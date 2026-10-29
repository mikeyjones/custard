//! `custard screenshot`: pictures of the app's pages, at the sizes of a
//! laptop and a phone, taken by Chrome or Chromium without a window. Uses the
//! one installed on this machine.

use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Duration,
};

use serde_json::json;

use crate::{
    add::find_app,
    common::{Failure, finish},
    run::{self, Server},
};

#[derive(clap::Args)]
pub struct ScreenshotArgs {
    /// The pages to picture, such as `/` or `/products/1`.
    #[arg(required = true)]
    paths: Vec<String>,
    /// Sizes, as `name=WIDTHxHEIGHT`. Defaults to desktop=1280x800 and mobile=390x844.
    #[arg(long = "size", value_name = "NAME=WxH")]
    sizes: Vec<String>,
    /// Where to write the pictures. Defaults to `target/custard/screenshots`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Picture an app already running here, such as http://localhost:3000,
    /// rather than building and starting one.
    #[arg(long)]
    url: Option<String>,
    /// The app's directory. Defaults to the current one, or the nearest above it.
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Report the result as one JSON object on standard output.
    #[arg(long)]
    json: bool,
}

pub fn run(args: ScreenshotArgs) -> ExitCode {
    let json = args.json;
    finish(json, screenshot(args), |report| {
        report["screenshots"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|shot| {
                format!(
                    "{} {:<8} {}",
                    shot["path"].as_str().unwrap_or_default(),
                    shot["size"].as_str().unwrap_or_default(),
                    shot["file"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Names and paths a Chrome-based browser goes by.
const BROWSERS: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
    "chrome",
    "brave",
    "brave-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
];

/// The browser to use: `CUSTARD_BROWSER`, or the first Chrome-based one found.
fn find_browser() -> Result<PathBuf, Failure> {
    if let Some(browser) = std::env::var_os("CUSTARD_BROWSER") {
        let browser = PathBuf::from(browser);
        return if browser.is_file() || on_path(&browser).is_some() {
            Ok(on_path(&browser).unwrap_or(browser))
        } else {
            Err(Failure::new(
                "browser_not_found",
                format!(
                    "CUSTARD_BROWSER is {}, which is not a program here",
                    browser.display()
                ),
            )
            .fix("point CUSTARD_BROWSER at Chrome, Chromium, Brave or Edge, or unset it"))
        };
    }
    BROWSERS
        .iter()
        .map(Path::new)
        .find_map(|candidate| {
            if candidate.is_absolute() {
                candidate.is_file().then(|| candidate.to_path_buf())
            } else {
                on_path(candidate)
            }
        })
        .ok_or_else(|| {
            Failure::new(
                "browser_not_found",
                "custard screenshot needs Chrome or Chromium, and none was found",
            )
            .fix(
                "install Chromium (`sudo pacman -S chromium`, `sudo apt install chromium`, \
                 `brew install --cask chromium`) or Chrome, or set CUSTARD_BROWSER to a Chrome-based browser",
            )
        })
}

fn on_path(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return None;
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

fn parse_size(text: &str) -> Result<(String, u32, u32), Failure> {
    let invalid = || {
        Failure::new("invalid_size", format!("`{text}` is not a size"))
            .fix("write sizes as name=WIDTHxHEIGHT, such as tablet=820x1180")
    };
    let (name, dimensions) = text.split_once('=').ok_or_else(invalid)?;
    let (width, height) = dimensions.split_once('x').ok_or_else(invalid)?;
    let width: u32 = width.parse().map_err(|_| invalid())?;
    let height: u32 = height.parse().map_err(|_| invalid())?;
    if name.is_empty() || !(200..=4000).contains(&width) || !(200..=10000).contains(&height) {
        return Err(invalid());
    }
    Ok((name.to_owned(), width, height))
}

/// `/products/1?tab=x` as `products-1-tab-x`; `/` as `home`.
fn file_stem(path: &str) -> String {
    let stem: String = path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if stem.is_empty() {
        "home".to_owned()
    } else {
        stem
    }
}

fn screenshot(args: ScreenshotArgs) -> Result<serde_json::Value, Failure> {
    let root = find_app(args.dir.as_deref())?;
    let browser = find_browser()?;
    for path in &args.paths {
        if !path.starts_with('/') {
            return Err(Failure::new(
                "invalid_path",
                format!("`{path}` does not start with /"),
            ));
        }
    }
    let sizes = if args.sizes.is_empty() {
        vec![
            ("desktop".to_owned(), 1280, 800),
            ("mobile".to_owned(), 390, 844),
        ]
    } else {
        args.sizes
            .iter()
            .map(|size| parse_size(size))
            .collect::<Result<_, _>>()?
    };
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| root.join("target/custard/screenshots"));
    std::fs::create_dir_all(&out)
        .map_err(|error| Failure::new("write_failed", format!("{}: {error}", out.display())))?;
    let out = std::fs::canonicalize(&out).unwrap_or(out);

    // An app to picture: the one running at --url, or this one, started now.
    let (base, server) = match &args.url {
        Some(url) => (url.trim_end_matches('/').to_owned(), None),
        None => {
            let package = run::package(&root)?;
            run::build(&root, &package)?;
            let server = Server::start(&root, &package)?;
            (server.url(""), Some(server))
        }
    };

    let profile = std::env::temp_dir().join(format!("custard-browser-{}", std::process::id()));
    let mut shots = Vec::new();
    for path in &args.paths {
        let status = server
            .as_ref()
            .and_then(|server| run::get(server.port, path).ok())
            .map(|answer| answer.status);
        for (size, width, height) in &sizes {
            let file = out.join(format!("{}-{size}.png", file_stem(path)));
            let _ = std::fs::remove_file(&file);
            let output = Command::new(&browser)
                .args([
                    "--headless=new",
                    "--disable-gpu",
                    "--hide-scrollbars",
                    "--no-first-run",
                    "--no-default-browser-check",
                    "--disable-extensions",
                    "--force-device-scale-factor=1",
                    "--virtual-time-budget=5000",
                ])
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg(format!("--window-size={width},{height}"))
                .arg(format!("--screenshot={}", file.display()))
                .arg(format!("{base}{path}"))
                .output()
                .map_err(|error| {
                    Failure::new(
                        "browser_failed",
                        format!("cannot run {}: {error}", browser.display()),
                    )
                })?;
            if !file.is_file() {
                return Err(Failure::new(
                    "browser_failed",
                    format!(
                        "{} made no picture of {path}:\n{}",
                        browser.display(),
                        String::from_utf8_lossy(&output.stderr)
                            .lines()
                            .take(10)
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                ));
            }
            shots.push(json!({
                "path": path,
                "size": size,
                "width": width,
                "height": height,
                "status": status,
                "file": file.display().to_string(),
            }));
        }
    }
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
    std::thread::sleep(Duration::from_millis(10));

    Ok(json!({
        "ok": true,
        "browser": browser.display().to_string(),
        "screenshots": shots,
        "next": ["look at each picture, and fix what does not look right"],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sizes() {
        assert_eq!(file_stem("/"), "home");
        assert_eq!(
            file_stem("/products/1?tab=Reviews"),
            "products-1-tab-reviews"
        );
        assert_eq!(
            parse_size("tablet=820x1180").unwrap(),
            ("tablet".into(), 820, 1180)
        );
        assert!(parse_size("820x1180").is_err());
        assert!(parse_size("huge=99999x10").is_err());
    }
}
