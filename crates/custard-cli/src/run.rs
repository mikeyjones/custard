//! Building the app, running it on a free port, and asking it for pages:
//! what `custard smoke` and `custard screenshot` share.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::common::Failure;

/// What Cargo says about the app.
pub struct Package {
    /// The debug build of the app's binary.
    pub binary: PathBuf,
}

pub fn package(root: &Path) -> Result<Package, Failure> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|error| Failure::new("cargo_failed", format!("cannot run cargo: {error}")))?;
    if !output.status.success() {
        return Err(Failure::new(
            "cargo_failed",
            format!(
                "cargo metadata failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| Failure::new("cargo_failed", format!("cargo metadata: {error}")))?;
    let manifest = root.join("Cargo.toml");
    let package = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| {
            package["manifest_path"].as_str().map(Path::new) == Some(manifest.as_path())
        })
        .ok_or_else(|| Failure::new("cargo_failed", "cargo metadata does not list this app"))?;
    let name = package["name"].as_str().unwrap_or_default().to_owned();
    let bin = package["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|k| k == "bin"))
        })
        .and_then(|target| target["name"].as_str())
        .unwrap_or(&name)
        .to_owned();
    let target = PathBuf::from(metadata["target_directory"].as_str().unwrap_or("target"));
    Ok(Package {
        binary: target.join("debug").join(bin),
    })
}

/// Builds the app and its asset bundle, as `custard dev` does.
pub fn build(root: &Path, package: &Package) -> Result<(), Failure> {
    let output = Command::new("cargo")
        .args(["build", "--message-format=short"])
        .current_dir(root)
        .output()
        .map_err(|error| Failure::new("cargo_failed", format!("cannot run cargo: {error}")))?;
    if !output.status.success() {
        let errors: Vec<&str> = std::str::from_utf8(&output.stderr)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("error"))
            .take(20)
            .collect();
        return Err(Failure::new(
            "build_failed",
            format!("the app does not build:\n{}", errors.join("\n")),
        )
        .fix("run `custard check --json` for the problems, one by one"));
    }
    let bundle = Command::new(&package.binary)
        .args(["assets", "bundle"])
        .current_dir(root)
        .output()
        .map_err(|error| Failure::new("build_failed", format!("cannot run the app: {error}")))?;
    if !bundle.status.success() {
        return Err(Failure::new(
            "build_failed",
            format!(
                "the assets could not be bundled:\n{}",
                String::from_utf8_lossy(&bundle.stderr)
            ),
        ));
    }
    Ok(())
}

/// The app, running until dropped.
pub struct Server {
    child: Child,
    pub port: u16,
    logs: Arc<Mutex<Vec<(Instant, String)>>>,
}

impl Server {
    /// Starts the built app on a free port on this machine, with the settings
    /// in its `.env`, and waits until it answers.
    pub fn start(root: &Path, package: &Package) -> Result<Server, Failure> {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|address| address.port())
            .map_err(|error| Failure::new("internal", format!("no free port: {error}")))?;
        let mut child = Command::new(&package.binary)
            .current_dir(root)
            .env("HOST", "127.0.0.1")
            .env("PORT", port.to_string())
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                Failure::new("start_failed", format!("cannot start the app: {error}"))
            })?;
        let logs = Arc::new(Mutex::new(Vec::new()));
        for stream in [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let logs = logs.clone();
            thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    logs.lock().unwrap().push((Instant::now(), line));
                }
            });
        }
        let mut server = Server { child, port, logs };

        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(Some(status)) = server.child.try_wait() {
                return Err(Failure::new(
                    "start_failed",
                    format!("the app stopped as it started ({status}):\n{}", server.logs_since(None).join("\n")),
                )
                .fix("check DATABASE_URL in .env, and that the database is running (docker compose up -d)"));
            }
            if get(port, "/up").is_ok() {
                return Ok(server);
            }
            if Instant::now() > deadline {
                return Err(Failure::new(
                    "start_failed",
                    format!(
                        "the app did not answer within a minute:\n{}",
                        server.logs_since(None).join("\n")
                    ),
                ));
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// What the app logged since `since`, or all of it.
    pub fn logs_since(&self, since: Option<Instant>) -> Vec<String> {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|(at, _)| since.is_none_or(|since| *at >= since))
            .map(|(_, line)| line.clone())
            .collect()
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://localhost:{}{path}", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // Ask it to stop as the platform would, then make sure.
            let _ = Command::new("kill")
                .arg(self.child.id().to_string())
                .status();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = self.child.try_wait() {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An answer to a request: its status and, for a redirect, where to.
pub struct Answer {
    pub status: u16,
    pub location: Option<String>,
}

/// `GET path` from the app on this machine, as a browser asking for a page
/// would, without following redirects.
pub fn get(port: u16, path: &str) -> std::io::Result<Answer> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nAccept: text/html\r\nUser-Agent: custard\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let text = String::from_utf8_lossy(&response);
    let mut lines = text.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| std::io::Error::other("not an HTTP response"))?;
    let location = lines.take_while(|line| !line.is_empty()).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("location")
            .then(|| value.trim().to_owned())
    });
    Ok(Answer { status, location })
}
