//! `custard new`: a Topcoat app with the recommended setup (module routing,
//! Tailwind, Topcoat UI, the browser runtime), wired to The Custard Factory.
//!
//! Topcoat's own generator makes the base, so the theme, components and build
//! script are exactly what `topcoat new --recommended` makes; Custard's files
//! are written over it.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Output},
};

use serde_json::json;

/// Where generated apps get the `custard` crate from, unless told otherwise.
const DEFAULT_CUSTARD_GIT: &str = "https://github.com/mikeyjones/custard";

/// The Topcoat UI components the generated pages use, beside the `card` that
/// Topcoat's generator installs.
const COMPONENTS: &[&str] = &["badge", "button", "input"];

#[derive(clap::Args)]
pub struct NewArgs {
    /// The directory to create. It must not exist.
    path: PathBuf,
    /// The Cargo package name. Defaults to the directory's name.
    #[arg(long)]
    name: Option<String>,
    /// The site's title. Defaults to the name, in words.
    #[arg(long)]
    title: Option<String>,
    /// Use the custard crate at this path, for working on Custard itself.
    /// Such an app cannot be built by The Custard Factory.
    #[arg(long, conflicts_with = "custard_git")]
    custard_path: Option<PathBuf>,
    /// Use the custard crate from this git repository.
    #[arg(long, default_value = DEFAULT_CUSTARD_GIT)]
    custard_git: String,
    /// Do not initialise a git repository.
    #[arg(long)]
    no_git: bool,
    /// Report the result as one JSON object on standard output.
    #[arg(long)]
    json: bool,
}

struct Template {
    path: &'static str,
    contents: &'static str,
}

macro_rules! templates {
    ($($path:literal => $file:literal),* $(,)?) => {
        &[$(Template { path: $path, contents: include_str!(concat!("../templates/app/", $file)) }),*]
    };
}

const TEMPLATES: &[Template] = templates![
    "Cargo.toml" => "Cargo.toml",
    "rust-toolchain.toml" => "rust-toolchain.toml",
    "rustfmt.toml" => "rustfmt.toml",
    "railpack.json" => "railpack.json",
    "docker-compose.yml" => "docker-compose.yml",
    ".env.example" => "env.example",
    ".env" => "env.example",
    ".gitignore" => "gitignore",
    "README.md" => "README.md",
    "AGENTS.md" => "AGENTS.md",
    "CLAUDE.md" => "CLAUDE.md",
    "src/main.rs" => "src/main.rs",
    "src/app.rs" => "src/app.rs",
    "src/app/notes.rs" => "src/app/notes.rs",
    "src/app/account.rs" => "src/app/account.rs",
    "src/features.rs" => "src/features.rs",
    "src/features/notes.rs" => "src/features/notes.rs",
];

const NOTES_MIGRATION: &str = include_str!("../templates/app/migrations/create_notes.sql");

pub fn run(args: NewArgs) -> ExitCode {
    let json = args.json;
    match create(args) {
        Ok(report) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                println!(
                    "Created {} in {}",
                    report["name"].as_str().unwrap(),
                    report["path"].as_str().unwrap()
                );
                println!("\nNext:");
                for step in report["next"].as_array().unwrap() {
                    println!("  {}", step.as_str().unwrap());
                }
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "ok": false,
                        "error": { "code": failure.code, "message": failure.message, "fix": failure.fix },
                    }))
                    .unwrap()
                );
            } else {
                eprintln!("error: {}", failure.message);
                if let Some(fix) = failure.fix {
                    eprintln!("  fix: {fix}");
                }
            }
            ExitCode::FAILURE
        }
    }
}

struct Failure {
    code: &'static str,
    message: String,
    fix: Option<String>,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
            fix: None,
        }
    }

    fn fix(mut self, fix: impl Into<String>) -> Failure {
        self.fix = Some(fix.into());
        self
    }
}

fn create(args: NewArgs) -> Result<serde_json::Value, Failure> {
    if args.path.exists() {
        return Err(Failure::new(
            "destination_exists",
            format!("{} already exists", args.path.display()),
        )
        .fix("choose a directory that does not exist yet"));
    }
    let name = match &args.name {
        Some(name) => name.clone(),
        None => args
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                Failure::new("invalid_name", "the directory has no usable name").fix("pass --name")
            })?,
    };
    check_name(&name)?;
    let title = args.title.clone().unwrap_or_else(|| words(&name));
    let custard_dep = match &args.custard_path {
        Some(path) => {
            let path = fs::canonicalize(path).map_err(|error| {
                Failure::new(
                    "invalid_custard_path",
                    format!("{}: {error}", path.display()),
                )
            })?;
            format!("custard = {{ path = {:?} }}", path.display().to_string())
        }
        None => format!("custard = {{ git = {:?} }}", args.custard_git),
    };

    // The base: Topcoat's recommended app, without its example or database.
    let path = args.path.to_string_lossy().into_owned();
    topcoat(
        None,
        &[
            "new",
            &path,
            "--name",
            &name,
            "--recommended",
            "--database",
            "none",
            "--no-example",
            "--no-git",
        ],
    )?;
    let root = fs::canonicalize(&args.path).map_err(|error| {
        Failure::new(
            "generate_failed",
            format!("the app was not created: {error}"),
        )
    })?;
    let mut ui_add = vec!["ui", "add"];
    ui_add.extend(COMPONENTS);
    topcoat(Some(&root), &ui_add)?;

    // Custard's files, over the base.
    let fill = |text: &str| {
        text.replace("{{name}}", &name)
            .replace("{{name_snake}}", &name.replace('-', "_"))
            .replace("{{title}}", &title)
            .replace("{{custard_dep}}", &custard_dep)
    };
    let mut written = Vec::new();
    for template in TEMPLATES {
        write(&root, template.path, &fill(template.contents))?;
        written.push(template.path.to_owned());
    }
    let migration = format!(
        "migrations/{}_create_notes.sql",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    );
    write(&root, &migration, NOTES_MIGRATION)?;
    written.push(migration);

    // Start formatted, whatever order Topcoat UI declared its modules in.
    // Formatting needs rustfmt on PATH; an app that is not formatted still works.
    let formatted = topcoat(Some(&root), &["fmt", "--rustfmt"]).is_ok();

    let components: Vec<&str> = ["card"]
        .into_iter()
        .chain(COMPONENTS.iter().copied())
        .collect();
    let git = !args.no_git && !inside_git_repository(&root) && git_init(&root);

    Ok(json!({
        "ok": true,
        "name": name,
        "title": title,
        "path": root.display().to_string(),
        "custard": custard_dep,
        "git": git,
        "formatted": formatted,
        "files": written,
        "components": components,
        "next": [
            format!("cd {}", args.path.display()),
            "docker compose up -d".to_owned(),
            "custard dev".to_owned(),
            "open http://localhost:3000".to_owned(),
        ],
        "deploy": {
            "release_command": format!("./bin/{name} migrate"),
            "notes": "Set the release command in the app's settings on The Custard Factory.",
        },
    }))
}

/// Runs a Topcoat CLI command through this binary, keeping its output
/// unless it fails.
fn topcoat(dir: Option<&Path>, args: &[&str]) -> Result<Output, Failure> {
    let exe = std::env::current_exe()
        .map_err(|error| Failure::new("internal", format!("cannot find this program: {error}")))?;
    let mut command = Command::new(exe);
    command.arg("topcoat").args(args);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let output = command
        .output()
        .map_err(|error| Failure::new("internal", format!("cannot run topcoat: {error}")))?;
    if !output.status.success() {
        return Err(Failure::new(
            "topcoat_failed",
            format!(
                "topcoat {} failed:\n{}{}",
                args.join(" "),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    Ok(output)
}

fn write(root: &Path, path: &str, contents: &str) -> Result<(), Failure> {
    let target = root.join(path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            Failure::new("write_failed", format!("{}: {error}", parent.display()))
        })?;
    }
    fs::write(&target, contents)
        .map_err(|error| Failure::new("write_failed", format!("{}: {error}", target.display())))
}

/// A Cargo package name that is also a fine directory and database name.
fn check_name(name: &str) -> Result<(), Failure> {
    let valid = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(Failure::new("invalid_name", format!("`{name}` is not a valid app name"))
            .fix("use lowercase letters, digits, `-` and `_`, starting with a letter, e.g. --name my-shop"))
    }
}

/// `my-shop` as `My shop`.
fn words(name: &str) -> String {
    let spaced = name.replace(['-', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => spaced,
    }
}

fn inside_git_repository(dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn git_init(dir: &Path) -> bool {
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(dir)
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(check_name("my-shop").is_ok());
        assert!(check_name("shop2").is_ok());
        assert!(check_name("My-Shop").is_err());
        assert!(check_name("2shop").is_err());
        assert!(check_name("my shop").is_err());
        assert_eq!(words("my-shop"), "My shop");
    }
}
