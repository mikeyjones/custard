//! What every command shares: how it reports success and failure, for people
//! and, with `--json`, for programs.

use std::{
    path::Path,
    process::{Command, ExitCode, Output},
};

use serde_json::{Value, json};

/// Why a command failed: a stable `code` for programs, a `message` for
/// people, and what to do about it.
#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub fix: Option<String>,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
            fix: None,
        }
    }

    pub fn fix(mut self, fix: impl Into<String>) -> Failure {
        self.fix = Some(fix.into());
        self
    }
}

/// Prints the outcome: the report as JSON, or `summary` of it and its `next`
/// steps for people.
pub fn finish(
    json: bool,
    outcome: Result<Value, Failure>,
    summary: impl FnOnce(&Value) -> String,
) -> ExitCode {
    match outcome {
        Ok(report) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                println!("{}", summary(&report));
                if let Some(next) = report["next"].as_array().filter(|next| !next.is_empty()) {
                    println!("\nNext:");
                    for step in next {
                        println!("  {}", step.as_str().unwrap_or_default());
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            if json {
                let report = json!({
                    "ok": false,
                    "error": { "code": failure.code, "message": failure.message, "fix": failure.fix },
                });
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
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

/// Runs a Topcoat CLI command through this binary, keeping its output
/// unless it fails.
pub fn topcoat(dir: Option<&Path>, args: &[&str]) -> Result<Output, Failure> {
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
