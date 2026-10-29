//! `custard smoke`: builds the app, runs it, and asks it for every page that
//! needs no path parameters, reporting any that fail with what the app logged.

use std::{path::PathBuf, process::ExitCode, time::Instant};

use serde_json::json;

use crate::{
    add::find_app,
    common::{Failure, finish},
    routes,
    run::{self, Server},
};

#[derive(clap::Args)]
pub struct SmokeArgs {
    /// More paths to ask for, such as pages with parameters: `--path /products/1`.
    #[arg(long = "path", value_name = "PATH")]
    paths: Vec<String>,
    /// The app's directory. Defaults to the current one, or the nearest above it.
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Report the result as one JSON object on standard output.
    #[arg(long)]
    json: bool,
}

pub fn run(args: SmokeArgs) -> ExitCode {
    let json = args.json;
    finish(json, smoke(args), |report| {
        let mut out = String::new();
        for result in report["results"].as_array().into_iter().flatten() {
            let mark = if result["ok"] == true { "ok  " } else { "FAIL" };
            out.push_str(&format!(
                "{mark} {} {} ({} ms)\n",
                result["status"],
                result["path"].as_str().unwrap_or_default(),
                result["ms"],
            ));
            for line in result["logs"].as_array().into_iter().flatten() {
                out.push_str(&format!("       {}\n", line.as_str().unwrap_or_default()));
            }
        }
        for skipped in report["skipped"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "skip {} ({})\n",
                skipped["url"].as_str().unwrap_or_default(),
                skipped["reason"].as_str().unwrap_or_default()
            ));
        }
        out.push_str(&format!(
            "{} of {} pages failed",
            report["failed"], report["checked"]
        ));
        out
    })
}

fn smoke(args: SmokeArgs) -> Result<serde_json::Value, Failure> {
    let root = find_app(args.dir.as_deref())?;
    let tree = routes::read(&root)?;
    let mut paths: Vec<String> = vec!["/up".to_owned()];
    let mut skipped = Vec::new();
    for route in &tree.routes {
        if route.is_plain_get_page() {
            if !paths.contains(&route.url) {
                paths.push(route.url.clone());
            }
        } else if route.kind == "page" && !route.params.is_empty() {
            skipped.push(json!({
                "url": route.url,
                "reason": "it has path parameters: pass a real one with --path",
            }));
        }
    }
    for path in &args.paths {
        if !path.starts_with('/') {
            return Err(Failure::new(
                "invalid_path",
                format!("`{path}` does not start with /"),
            ));
        }
        if !paths.contains(path) {
            paths.push(path.clone());
        }
    }

    let package = run::package(&root)?;
    run::build(&root, &package)?;
    let server = Server::start(&root, &package)?;

    let mut results = Vec::new();
    for path in &paths {
        let started = Instant::now();
        let answer = run::get(server.port, path);
        let ms = started.elapsed().as_millis();
        // Give the app a moment to finish writing what it logged.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let (status, location, ok) = match &answer {
            Ok(answer) => (answer.status, answer.location.clone(), answer.status < 400),
            Err(_) => (0, None, false),
        };
        let logs = if ok {
            Vec::new()
        } else {
            server.logs_since(Some(started))
        };
        results.push(json!({
            "path": path,
            "status": status,
            "ok": ok,
            "ms": ms,
            "location": location,
            "error": answer.err().map(|error| error.to_string()),
            "logs": logs,
        }));
    }
    drop(server);

    let failed = results.iter().filter(|result| result["ok"] != true).count();
    let report = json!({
        "ok": failed == 0,
        "checked": results.len(),
        "failed": failed,
        "results": results,
        "skipped": skipped,
        "next": if failed == 0 { vec![] } else { vec!["fix each failing page, then run custard smoke again"] },
    });
    if failed > 0 {
        let mut failure = Failure::new(
            "pages_failed",
            format!("{failed} of {} pages failed", report["checked"]),
        );
        failure.report = Some(report);
        return Err(failure);
    }
    Ok(report)
}
