//! Editing an app's files: everything a command changes is kept in memory
//! until it is all written, so a command that fails changes nothing.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::common::Failure;

/// The files a command reads, creates and changes, kept in memory until
/// they are all written, so a failure changes nothing.
pub struct Files<'a> {
    root: &'a Path,
    contents: BTreeMap<PathBuf, (String, Change)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Change {
    None,
    Created,
    Modified,
}

impl<'a> Files<'a> {
    pub fn new(root: &'a Path) -> Files<'a> {
        Files {
            root,
            contents: BTreeMap::new(),
        }
    }

    pub fn exists(&self, path: &Path) -> bool {
        self.contents.contains_key(path) || self.root.join(path).exists()
    }

    pub fn read(&mut self, path: &Path) -> Result<String, Failure> {
        if let Some((text, _)) = self.contents.get(path) {
            return Ok(text.clone());
        }
        let text = fs::read_to_string(self.root.join(path))
            .map_err(|error| Failure::new("read_failed", format!("{}: {error}", path.display())))?;
        self.contents
            .insert(path.to_path_buf(), (text.clone(), Change::None));
        Ok(text)
    }

    pub fn create(&mut self, path: &Path, text: String) {
        self.contents
            .insert(path.to_path_buf(), (text, Change::Created));
    }

    pub fn set(&mut self, path: &Path, text: String) {
        let change = match self.contents.get(path) {
            Some((_, Change::Created)) => Change::Created,
            _ => Change::Modified,
        };
        self.contents.insert(path.to_path_buf(), (text, change));
    }

    /// Adds `imports` after a module's declarations and imports, and `body`
    /// at its end.
    pub fn extend_module(&mut self, path: &Path, imports: &str, body: &str) {
        let text = self
            .contents
            .get(path)
            .map(|(t, _)| t.clone())
            .unwrap_or_default();
        let mut lines: Vec<String> = text.trim_end().lines().map(str::to_owned).collect();
        let at = lines
            .iter()
            .rposition(|line| {
                ["mod ", "pub mod ", "pub(crate) mod ", "use "]
                    .iter()
                    .any(|start| line.starts_with(start))
            })
            .map(|last| last + 1)
            .unwrap_or(lines.len());
        lines.insert(at, format!("\n{}", imports.trim_end()));
        self.set(path, format!("{}\n{body}", lines.join("\n")));
    }

    /// Adds `mod module;` to `parent`, after its other `mod` declarations,
    /// unless it is already there.
    pub fn declare(&mut self, parent: &Path, module: &str) -> Result<(), Failure> {
        let text = self.read(parent)?;
        let declared = text.lines().any(|line| {
            let line = line.trim();
            let line = line
                .strip_prefix("pub(crate) ")
                .or_else(|| line.strip_prefix("pub "))
                .unwrap_or(line);
            line == format!("mod {module};")
        });
        if declared {
            return Ok(());
        }
        let lines: Vec<&str> = text.lines().collect();
        let is_mod = |line: &str| {
            let line = line.trim_start();
            (line.starts_with("mod ")
                || line.starts_with("pub mod ")
                || line.starts_with("pub(crate) mod "))
                && line.ends_with(';')
        };
        let declaration = format!("pub mod {module};");
        let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
        match lines.iter().rposition(|line| is_mod(line)) {
            Some(last) => out.insert(last + 1, declaration),
            None => {
                // After the module's doc comment and inner attributes.
                let at = lines
                    .iter()
                    .position(|line| {
                        let line = line.trim_start();
                        !(line.is_empty() || line.starts_with("//!") || line.starts_with("#!["))
                    })
                    .unwrap_or(lines.len());
                let mut insert = vec![declaration];
                if at > 0 && !lines[at - 1].trim().is_empty() {
                    insert.insert(0, String::new());
                }
                if at < lines.len() {
                    insert.push(String::new());
                }
                for (offset, line) in insert.into_iter().enumerate() {
                    out.insert(at + offset, line);
                }
            }
        }
        let mut text = out.join("\n");
        text.push('\n');
        self.set(parent, text);
        Ok(())
    }

    /// What would be created and changed.
    pub fn plan(&self) -> (Vec<String>, Vec<String>) {
        let list = |wanted: Change| {
            self.contents
                .iter()
                .filter(|(_, (_, change))| *change == wanted)
                .map(|(path, _)| path.display().to_string())
                .collect()
        };
        (list(Change::Created), list(Change::Modified))
    }

    /// Writes every created and changed file, returning their paths.
    pub fn write(&self) -> Result<Vec<PathBuf>, Failure> {
        let mut written = Vec::new();
        for (path, (text, change)) in &self.contents {
            if *change == Change::None {
                continue;
            }
            let target = self.root.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    Failure::new("write_failed", format!("{}: {error}", parent.display()))
                })?;
            }
            fs::write(&target, text).map_err(|error| {
                Failure::new("write_failed", format!("{}: {error}", target.display()))
            })?;
            written.push(path.clone());
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_modules_after_the_others() {
        let dir = std::env::temp_dir().join(format!("custard-add-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.rs"), "mod account;\nmod notes;\n\nuse x;\n").unwrap();
        fs::write(dir.join("b.rs"), "//! Docs.\n\nuse y;\n").unwrap();
        fs::write(dir.join("c.rs"), "//! Docs.\n").unwrap();
        let mut files = Files::new(&dir);
        files.declare(Path::new("a.rs"), "blog").unwrap();
        files.declare(Path::new("a.rs"), "notes").unwrap();
        files.declare(Path::new("b.rs"), "posts").unwrap();
        files.declare(Path::new("c.rs"), "posts").unwrap();
        assert_eq!(
            files.read(Path::new("a.rs")).unwrap(),
            "mod account;\nmod notes;\npub mod blog;\n\nuse x;\n"
        );
        assert_eq!(
            files.read(Path::new("b.rs")).unwrap(),
            "//! Docs.\n\npub mod posts;\n\nuse y;\n"
        );
        assert_eq!(
            files.read(Path::new("c.rs")).unwrap(),
            "//! Docs.\n\npub mod posts;\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
