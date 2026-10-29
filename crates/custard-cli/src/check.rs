//! `custard check`: everything wrong with the app, as one list. Rust's own
//! errors and warnings (from `cargo check`), files that need formatting, and
//! what Custard knows goes wrong in its apps, each with where it is and what
//! to do about it. Changes nothing.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use serde::Serialize;
use serde_json::json;

use crate::{
    add::find_app,
    common::{Failure, finish, topcoat},
    routes,
};

#[derive(clap::Args)]
pub struct CheckArgs {
    /// The app's directory. Defaults to the current one, or the nearest above it.
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Report the result as one JSON object on standard output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize)]
struct Problem {
    /// `rustc`, `fmt` or `custard`.
    source: &'static str,
    /// `error` or `warning`.
    level: String,
    /// Rust's error code, such as `E0308`, or Custard's, such as `sqlx_macro`.
    code: Option<String>,
    message: String,
    file: Option<String>,
    line: Option<u64>,
    column: Option<u64>,
    /// What to do about it, when known.
    fix: Option<String>,
    /// The compiler's full explanation, with the code it points at.
    #[serde(skip_serializing_if = "Option::is_none")]
    rendered: Option<String>,
}

impl Problem {
    fn custard(
        level: &str,
        code: &str,
        message: String,
        file: &Path,
        line: Option<u64>,
        fix: &str,
    ) -> Problem {
        Problem {
            source: "custard",
            level: level.to_owned(),
            code: Some(code.to_owned()),
            message,
            file: Some(file.display().to_string()),
            line,
            column: None,
            fix: Some(fix.to_owned()),
            rendered: None,
        }
    }
}

pub fn run(args: CheckArgs) -> ExitCode {
    let json = args.json;
    finish(json, check(args), |report| {
        let mut out = String::new();
        for problem in report["problems"].as_array().into_iter().flatten() {
            let place = match (problem["file"].as_str(), problem["line"].as_u64()) {
                (Some(file), Some(line)) => format!("{file}:{line}: "),
                (Some(file), None) => format!("{file}: "),
                _ => String::new(),
            };
            out.push_str(&format!(
                "{}: {place}{}\n",
                problem["level"].as_str().unwrap_or_default(),
                problem["message"].as_str().unwrap_or_default()
            ));
            if let Some(fix) = problem["fix"].as_str() {
                out.push_str(&format!("  fix: {fix}\n"));
            }
        }
        out.push_str(&format!(
            "{} errors, {} warnings",
            report["errors"], report["warnings"]
        ));
        out
    })
}

fn check(args: CheckArgs) -> Result<serde_json::Value, Failure> {
    let root = find_app(args.dir.as_deref())?;
    let mut problems = rustc(&root)?;
    problems.extend(formatting(&root));
    problems.extend(custard(&root)?);

    let errors = problems.iter().filter(|p| p.level == "error").count();
    let warnings = problems.len() - errors;
    let report = json!({
        "ok": errors == 0 && warnings == 0,
        "errors": errors,
        "warnings": warnings,
        "problems": problems,
        "next": if problems.is_empty() {
            vec!["custard smoke".to_owned()]
        } else {
            vec!["fix the problems, then run custard check again".to_owned()]
        },
    });
    if errors > 0 || warnings > 0 {
        let mut failure = Failure::new(
            if errors > 0 { "errors" } else { "warnings" },
            format!("{errors} errors, {warnings} warnings"),
        );
        failure.report = Some(report);
        return Err(failure);
    }
    Ok(report)
}

/// Rust's errors and warnings in the app's own code.
fn rustc(root: &Path) -> Result<Vec<Problem>, Failure> {
    let output = Command::new("cargo")
        .args(["check", "--all-targets", "--message-format=json"])
        .current_dir(root)
        .output()
        .map_err(|error| Failure::new("cargo_failed", format!("cannot run cargo: {error}")))?;
    let mut problems = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in output.stdout.split(|b| *b == b'\n') {
        let Ok(message) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-message" {
            continue;
        }
        let diagnostic = &message["message"];
        let level = diagnostic["level"].as_str().unwrap_or_default();
        if !matches!(level, "error" | "warning") {
            continue;
        }
        let text = diagnostic["message"].as_str().unwrap_or_default();
        // The count at the end repeats what came before.
        if text.starts_with("aborting due to")
            || text.contains("warning emitted")
            || text.contains("warnings emitted")
        {
            continue;
        }
        let span = diagnostic["spans"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|span| span["is_primary"] == true);
        let file = span.and_then(|span| span["file_name"].as_str());
        // Only the app's own files: not its dependencies'.
        if file
            .is_some_and(|file| Path::new(file).is_absolute() && !Path::new(file).starts_with(root))
        {
            continue;
        }
        let rendered = diagnostic["rendered"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if !seen.insert(rendered.clone()) {
            continue;
        }
        let fix = diagnostic["children"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|child| {
                if child["level"] != "help" {
                    return None;
                }
                let replacement = child["spans"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find_map(|span| span["suggested_replacement"].as_str());
                let help = child["message"].as_str().unwrap_or_default();
                Some(match replacement {
                    Some(replacement) if !replacement.is_empty() => {
                        format!("{help}: `{replacement}`")
                    }
                    _ => help.to_owned(),
                })
            });
        problems.push(Problem {
            source: "rustc",
            level: level.to_owned(),
            code: diagnostic["code"]["code"].as_str().map(str::to_owned),
            message: text.to_owned(),
            file: file.map(str::to_owned),
            line: span.and_then(|span| span["line_start"].as_u64()),
            column: span.and_then(|span| span["column_start"].as_u64()),
            fix,
            rendered: Some(rendered),
        });
    }
    if !output.status.success() && problems.iter().all(|p| p.level != "error") {
        // Cargo failed before compiling: a broken Cargo.toml, a missing dependency.
        let stderr = String::from_utf8_lossy(&output.stderr);
        problems.push(Problem {
            source: "rustc",
            level: "error".into(),
            code: None,
            message: stderr
                .lines()
                .filter(|line| line.contains("error"))
                .collect::<Vec<_>>()
                .join("\n"),
            file: Some("Cargo.toml".into()),
            line: None,
            column: None,
            fix: None,
            rendered: Some(stderr.into_owned()),
        });
    }
    Ok(problems)
}

/// Files `custard fmt` would change.
fn formatting(root: &Path) -> Vec<Problem> {
    let output = match topcoat(Some(root), &["fmt", "--check", "--rustfmt"]) {
        Ok(_) => return Vec::new(),
        Err(failure) => failure.message,
    };
    output
        .lines()
        .filter_map(|line| line.strip_suffix(": needs formatting"))
        .map(|file| Problem {
            source: "fmt",
            level: "warning".into(),
            code: Some("needs_formatting".into()),
            message: "not formatted".into(),
            file: Some(file.to_owned()),
            line: None,
            column: None,
            fix: Some("run `custard fmt --rustfmt`".into()),
            rendered: None,
        })
        .collect()
}

/// What goes wrong in Custard apps that Rust cannot see.
fn custard(root: &Path) -> Result<Vec<Problem>, Failure> {
    let mut problems = Vec::new();

    // Files under src/app that no `mod` reaches are never compiled or served.
    let tree = routes::read(root)?;
    for file in rust_files(&root.join("src/app")) {
        let relative = file.strip_prefix(root).unwrap().to_path_buf();
        if !tree.reached.contains(&relative) {
            problems.push(Problem::custard(
                "warning",
                "unreachable_module",
                "no `mod` declaration reaches this file, so it is not compiled and its pages are not served".into(),
                &relative,
                None,
                "declare it with `pub mod <name>;` in its parent module, or make pages with `custard add page`",
            ));
        }
    }

    for file in rust_files(&root.join("src")) {
        let relative = file.strip_prefix(root).unwrap().to_path_buf();
        let text = fs::read_to_string(&file).unwrap_or_default();
        for (number, line) in text.lines().enumerate() {
            let line_number = Some(number as u64 + 1);
            for macro_name in [
                "sqlx::query!(",
                "sqlx::query_as!(",
                "sqlx::query_scalar!(",
                "query!(",
                "query_as!(",
            ] {
                if line.contains(macro_name) && !line.trim_start().starts_with("//") {
                    problems.push(Problem::custard(
                        "error",
                        "sqlx_macro",
                        format!("`{}` checks SQL against a database while compiling, and The Custard Factory builds without one", macro_name.trim_end_matches('(')),
                        &relative,
                        line_number,
                        "use `sqlx::query_as::<_, T>(\"...\")` with `#[derive(FromRow)]` instead",
                    ));
                    break;
                }
            }
            if relative.starts_with("src/app")
                && let Some(at) = line.find("href=\"/")
                && !line[at..].starts_with("href=\"/_custard")
            {
                problems.push(Problem::custard(
                    "warning",
                    "literal_link",
                    "a link to a page here is written as a literal path, which breaks when the page moves".into(),
                    &relative,
                    line_number,
                    "link with `href=(href!(module::page))`; `custard routes` lists each page's href!",
                ));
            }
        }
    }

    // Migrations: named so they run in order, and each version once.
    let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in fs::read_dir(root.join("migrations"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = PathBuf::from("migrations").join(&name);
        let Some(stem) = name.strip_suffix(".sql") else {
            continue;
        };
        let (version, description) = stem.split_once('_').unwrap_or((stem, ""));
        let named = version.chars().all(|c| c.is_ascii_digit())
            && !version.is_empty()
            && !description.is_empty()
            && !description.ends_with(".down")
            || stem.ends_with(".up")
            || stem.ends_with(".down");
        if !named {
            problems.push(Problem::custard(
                "error",
                "migration_name",
                "a migration's name must be `<version>_<description>.sql`, with a number for the version".into(),
                &path,
                None,
                "make migrations with `custard add migration <description>`",
            ));
            continue;
        }
        versions
            .entry(version.to_owned())
            .or_default()
            .push(name.clone());
        if fs::metadata(entry.path()).is_ok_and(|meta| meta.len() == 0)
            || fs::read_to_string(entry.path()).is_ok_and(|sql| {
                sql.lines()
                    .all(|line| line.trim().is_empty() || line.trim_start().starts_with("--"))
            })
        {
            problems.push(Problem::custard(
                "warning",
                "empty_migration",
                "this migration has no SQL in it yet".into(),
                &path,
                None,
                "write its SQL, or delete it if it is not needed",
            ));
        }
    }
    for (version, names) in versions {
        if names.len() > 1 {
            problems.push(Problem::custard(
                "error",
                "migration_version_twice",
                format!("version {version} is used by {}", names.join(" and ")),
                Path::new("migrations"),
                None,
                "give one of them a later version: rename it, or make it again with `custard add migration`",
            ));
        }
    }
    Ok(problems)
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}
