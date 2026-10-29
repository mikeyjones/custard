//! `custard routes`: every page, route, layout and layer in the app, read
//! from its source the way module routing reads it: one URL segment per
//! module under `src/app`, following `mod` declarations.
//!
//! This reads the source rather than running the app, so it works while the
//! app does not compile. It understands what the generators write and what
//! Topcoat's guides show; inline `mod name { ... }` blocks are not followed.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use serde::Serialize;
use serde_json::json;

use crate::{
    add::{find_app, pascal},
    common::{Failure, finish},
};

#[derive(clap::Args)]
pub struct RoutesArgs {
    /// The app's directory. Defaults to the current one, or the nearest above it.
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Report the routes as one JSON object on standard output.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Route {
    /// `page`, `route`, `layout`, `layer`, or `custard` for Custard's own.
    pub kind: &'static str,
    pub methods: Vec<String>,
    pub url: String,
    /// The file, relative to the app.
    pub file: String,
    pub line: usize,
    /// The handler's Rust path.
    pub handler: String,
    /// How to link to it, for pages and routes.
    pub href: Option<String>,
    /// `none`, `user`, or `role:<name>`: what the handler asks of whoever calls it.
    pub auth: String,
    /// Path parameters, outermost first.
    pub params: Vec<String>,
    /// Absolute paths in attributes are only served when the router registers
    /// them with `.discover()` or by hand.
    pub absolute: bool,
}

impl Route {
    pub fn is_plain_get_page(&self) -> bool {
        self.kind == "page" && self.methods.iter().any(|m| m == "GET") && self.params.is_empty()
    }
}

/// What reading the module tree found.
pub struct Tree {
    pub routes: Vec<Route>,
    /// Every `.rs` file under `src/app` that the module tree reached.
    pub reached: BTreeSet<PathBuf>,
}

pub fn run(args: RoutesArgs) -> ExitCode {
    let json = args.json;
    let outcome = find_app(args.dir.as_deref()).and_then(|root| {
        let tree = read(&root)?;
        Ok(json!({ "ok": true, "routes": tree.routes, "next": [] }))
    });
    finish(json, outcome, |report| {
        let mut out = String::new();
        for route in report["routes"].as_array().into_iter().flatten() {
            let methods: Vec<&str> = route["methods"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m.as_str())
                .collect();
            let auth = route["auth"].as_str().unwrap_or("none");
            out.push_str(&format!(
                "{:<7} {:<10} {:<36} {}{}\n",
                route["kind"].as_str().unwrap_or_default(),
                methods.join(","),
                route["url"].as_str().unwrap_or_default(),
                route["file"].as_str().unwrap_or_default(),
                if auth == "none" {
                    String::new()
                } else {
                    format!("  [{auth}]")
                },
            ));
        }
        out.trim_end().to_owned()
    })
}

/// A URL segment, as a module gives it.
#[derive(Clone, Debug)]
enum Segment {
    Static(String),
    Param {
        name: String,
        catch_all: bool,
        module: Vec<String>,
    },
    /// `_group` modules scope layouts without adding to the URL.
    Group,
}

pub fn read(root: &Path) -> Result<Tree, Failure> {
    let mut tree = Tree {
        routes: Vec::new(),
        reached: BTreeSet::new(),
    };
    walk(root, Path::new("src/app.rs"), &[], &[], &mut tree)?;
    let app = fs::read_to_string(root.join("src/app.rs")).unwrap_or_default();
    if app.contains(".custard(") {
        let custard = |methods: &str, url: &str, what: &str| Route {
            kind: "custard",
            methods: vec![methods.to_owned()],
            url: url.to_owned(),
            file: "custard".to_owned(),
            line: 0,
            handler: what.to_owned(),
            href: None,
            auth: "none".to_owned(),
            params: Vec::new(),
            absolute: true,
        };
        tree.routes.push(custard("GET", "/up", "health check"));
        tree.routes.push(custard(
            "GET",
            "/_custard/auth/login",
            "sign in (custard::auth::LOGIN_PATH)",
        ));
        tree.routes.push(custard(
            "GET",
            "/_custard/auth/callback",
            "sign-in callback",
        ));
        tree.routes.push(custard(
            "GET",
            "/_custard/auth/logout",
            "sign out (custard::auth::LOGOUT_PATH)",
        ));
    }
    Ok(tree)
}

fn walk(
    root: &Path,
    file: &Path,
    modules: &[String],
    segments: &[Segment],
    tree: &mut Tree,
) -> Result<(), Failure> {
    let text = fs::read_to_string(root.join(file))
        .map_err(|error| Failure::new("read_failed", format!("{}: {error}", file.display())))?;
    tree.reached.insert(file.to_path_buf());

    // This module's own segment.
    let mut segments = segments.to_vec();
    if let Some(module) = modules.last() {
        segments.push(own_segment(&text, module, modules));
    }

    handlers(&text, file, modules, &segments, tree);

    // Its children.
    let dir = if file.file_name().is_some_and(|name| name == "mod.rs") {
        file.parent().unwrap().to_path_buf()
    } else {
        file.with_extension("")
    };
    for child in child_modules(&text) {
        let flat = dir.join(format!("{child}.rs"));
        let nested = dir.join(&child).join("mod.rs");
        let child_file = if root.join(&flat).is_file() {
            flat
        } else if root.join(&nested).is_file() {
            nested
        } else {
            continue;
        };
        let mut child_modules = modules.to_vec();
        child_modules.push(child);
        walk(root, &child_file, &child_modules, &segments, tree)?;
    }
    Ok(())
}

fn own_segment(text: &str, module: &str, modules: &[String]) -> Segment {
    if let Some(args) = macro_args(text, "module_param!") {
        let args = args
            .trim_start_matches("pub(crate)")
            .trim_start_matches("pub")
            .trim();
        let catch_all = args.starts_with('*');
        let name: String = args
            .trim_start_matches('*')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        return Segment::Param {
            name,
            catch_all,
            module: modules.to_vec(),
        };
    }
    if let Some(args) = macro_args(text, "segment!")
        && let Some(rename) = args.split("rename").nth(1).and_then(first_string)
    {
        return Segment::Static(rename);
    }
    if module.starts_with('_') {
        return Segment::Group;
    }
    Segment::Static(module.replace('_', "-"))
}

/// The text between the parentheses of the first `name(...)` at the start of a line.
fn macro_args(text: &str, name: &str) -> Option<String> {
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with(name))?;
    let start = line.find('(')? + 1;
    let end = line.rfind(')')?;
    (start <= end).then(|| line[start..end].to_owned())
}

fn first_string(text: &str) -> Option<String> {
    let start = text.find('"')? + 1;
    let end = start + text[start..].find('"')?;
    Some(text[start..end].to_owned())
}

fn child_modules(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line
                .strip_prefix("pub(crate) ")
                .or_else(|| line.strip_prefix("pub "))
                .unwrap_or(line);
            let name = line.strip_prefix("mod ")?.strip_suffix(';')?.trim();
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                .then(|| name.to_owned())
        })
        .collect()
}

fn url(segments: &[Segment]) -> String {
    let parts: Vec<String> = segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Static(text) => Some(text.clone()),
            Segment::Param {
                name,
                catch_all: false,
                ..
            } => Some(format!("{{{name}}}")),
            Segment::Param {
                name,
                catch_all: true,
                ..
            } => Some(format!("{{*{name}}}")),
            Segment::Group => None,
        })
        .collect();
    format!("/{}", parts.join("/"))
}

fn join(base: &str, relative: &str) -> String {
    let relative = relative.trim_start_matches("./");
    if base == "/" {
        format!("/{relative}")
    } else if relative.is_empty() {
        format!("{base}/")
    } else {
        format!("{base}/{relative}")
    }
}

fn handlers(text: &str, file: &Path, modules: &[String], segments: &[Segment], tree: &mut Tree) {
    let lines: Vec<&str> = text.lines().collect();
    let attribute = |line: &str| {
        let line = line.trim_start();
        ["page", "route", "layout", "layer"]
            .into_iter()
            .find(|kind| {
                line.starts_with(&format!("#[{kind}]")) || line.starts_with(&format!("#[{kind}("))
            })
    };
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| attribute(lines[i]).is_some())
        .collect();
    let module_url = url(segments);
    let params: Vec<(String, Vec<String>)> = segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Param { name, module, .. } => Some((name.clone(), module.clone())),
            _ => None,
        })
        .collect();

    for (n, &at) in starts.iter().enumerate() {
        let kind = attribute(lines[at]).unwrap();
        let line = lines[at].trim();
        let args = line
            .find('(')
            .and_then(|start| line.rfind(')').map(|end| line[start + 1..end].to_owned()))
            .unwrap_or_default();
        // Drop `with(...)` so its strings are not taken for the path.
        let args = args.split(", with(").next().unwrap_or("").to_owned();
        let path = first_string(&args);
        let before_path = match args.find('"') {
            Some(quote) => &args[..quote],
            None => args.as_str(),
        };
        let mut methods: Vec<String> = before_path
            .split(|c: char| !(c.is_ascii_alphabetic() || c == '*'))
            .filter(|word| !word.is_empty() && *word != "with")
            .map(str::to_owned)
            .collect();
        if methods.is_empty() {
            methods = match kind {
                "page" => vec!["GET".into()],
                "route" => vec!["?".into()],
                _ => Vec::new(),
            };
        }
        let (url, absolute) = match &path {
            Some(path) if path.starts_with('/') => (path.clone(), true),
            Some(path) => (join(&module_url, path), false),
            None => (module_url.clone(), false),
        };

        // The function, and what its body asks of the caller.
        let end = starts.get(n + 1).copied().unwrap_or(lines.len());
        let Some((fn_offset, name)) = lines[at..end].iter().enumerate().find_map(|(i, line)| {
            let after = line.split("fn ").nth(1)?;
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some((i, name))
        }) else {
            continue;
        };
        let body = lines[at + fn_offset..end].join("\n");
        let auth = if let Some(role) = body.split("require_role(cx,").nth(1).and_then(first_string)
        {
            format!("role:{role}")
        } else if body.contains("require_user(") {
            "user".to_owned()
        } else {
            "none".to_owned()
        };

        let module_path = if modules.is_empty() {
            "crate::app".to_owned()
        } else {
            format!("crate::app::{}", modules.join("::"))
        };
        let handler = format!("{module_path}::{name}");
        let href = matches!(kind, "page" | "route").then(|| {
            let values: Vec<String> = params
                .iter()
                .map(|(param, module)| {
                    format!("crate::app::{}::{}(..)", module.join("::"), pascal(param))
                })
                .collect();
            if values.is_empty() {
                format!("href!({handler})")
            } else {
                format!("href!({handler}, {})", values.join(", "))
            }
        });
        tree.routes.push(Route {
            kind,
            methods,
            url,
            file: file.display().to_string(),
            line: at + 1,
            handler,
            href,
            auth,
            params: params.iter().map(|(name, _)| name.clone()).collect(),
            absolute,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_module_tree() {
        let root = std::env::temp_dir().join(format!("custard-routes-test-{}", std::process::id()));
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "src/app.rs",
            "mod blog_posts;\npub mod _marketing;\n\n#[layout]\nasync fn root(slot: Slot<'_>) {}\n\n#[page]\nasync fn home() {}\n\nfn router() { module_router!().custard(platform) }\n",
        );
        write(
            "src/app/blog_posts.rs",
            "pub mod post_id;\n\n#[page]\npub async fn page(cx: &Cx) {\n    custard::auth::require_user(cx).await?;\n}\n\n#[route(POST)]\npub async fn create(cx: &Cx) {}\n",
        );
        write(
            "src/app/blog_posts/post_id.rs",
            "module_param!(pub post_id: i64, error = not_found);\n\n#[page]\npub async fn page(cx: &Cx) {\n    custard::auth::require_role(cx, \"editor\").await?;\n}\n\n#[route(POST \"./delete\")]\npub async fn delete(cx: &Cx) {}\n",
        );
        write("src/app/_marketing.rs", "pub mod pricing;\n");
        write(
            "src/app/_marketing/pricing.rs",
            "#[page]\nasync fn pricing() {}\n",
        );

        let tree = read(&root).unwrap();
        let found: Vec<(String, String, String)> = tree
            .routes
            .iter()
            .map(|r| (r.kind.to_owned(), r.url.clone(), r.auth.clone()))
            .collect();
        let expect =
            |kind: &str, url: &str, auth: &str| (kind.to_owned(), url.to_owned(), auth.to_owned());
        assert!(found.contains(&expect("layout", "/", "none")));
        assert!(found.contains(&expect("page", "/", "none")));
        assert!(found.contains(&expect("page", "/blog-posts", "user")));
        assert!(found.contains(&expect("route", "/blog-posts", "none")));
        assert!(found.contains(&expect("page", "/blog-posts/{post_id}", "role:editor")));
        assert!(found.contains(&expect("route", "/blog-posts/{post_id}/delete", "none")));
        assert!(found.contains(&expect("page", "/pricing", "none")));
        assert!(found.contains(&expect("custard", "/up", "none")));
        let item = tree
            .routes
            .iter()
            .find(|r| r.url == "/blog-posts/{post_id}" && r.kind == "page")
            .unwrap();
        assert_eq!(
            item.href.as_deref(),
            Some(
                "href!(crate::app::blog_posts::post_id::page, crate::app::blog_posts::post_id::PostId(..))"
            )
        );
        assert_eq!(tree.reached.len(), 5);
        fs::remove_dir_all(root).unwrap();
    }
}
