//! `custard add`: new parts of an app, ready to use.
//!
//! `custard add page /blog/{post_id}` writes `src/app/blog/post_id.rs` with a
//! `#[page]`, makes the modules above it, and declares each one in its parent,
//! so module routing serves it at once.

use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use serde_json::json;

use crate::{
    common::{Failure, finish, topcoat},
    files::Files,
    new::words,
};

#[derive(clap::Subcommand)]
pub enum AddCommand {
    /// Add a page at a URL path, such as /about, /blog/posts or /posts/{post_id}
    Page(PageArgs),
    /// Add a feature: its table, its data and queries, and pages to manage it
    Feature(crate::feature::FeatureArgs),
    /// Add an empty SQL migration to migrations/
    Migration(crate::feature::MigrationArgs),
}

#[derive(clap::Args)]
pub struct PageArgs {
    /// The page's URL path. Static segments are lowercase words joined by `-`;
    /// `{name}` is a path parameter, read in the page.
    path: String,
    /// The page's heading. Defaults to its last segment, in words.
    #[arg(long)]
    title: Option<String>,
    /// Only for people who are signed in; others are sent to sign in.
    #[arg(long)]
    auth: bool,
    /// Only for people with this role; implies --auth.
    #[arg(long)]
    role: Option<String>,
    /// The app's directory. Defaults to the current one, or the nearest above it.
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Say what would change without changing anything.
    #[arg(long)]
    dry_run: bool,
    /// Report the result as one JSON object on standard output.
    #[arg(long)]
    json: bool,
}

pub fn run(command: AddCommand) -> ExitCode {
    match command {
        AddCommand::Feature(args) => crate::feature::run_feature(args),
        AddCommand::Migration(args) => crate::feature::run_migration(args),
        AddCommand::Page(args) => {
            let json = args.json;
            finish(json, add_page(args), |report| {
                let verb = if report["dry_run"] == true {
                    "Would add"
                } else {
                    "Added"
                };
                format!(
                    "{verb} {} in {}\nLink to it with {}",
                    report["url"].as_str().unwrap_or_default(),
                    report["file"].as_str().unwrap_or_default(),
                    report["href"].as_str().unwrap_or_default(),
                )
            })
        }
    }
}

/// One segment of the page's URL, and the module that serves it.
#[derive(Clone, Debug, PartialEq)]
enum Segment {
    /// `blog-posts`, served by `mod blog_posts`.
    Static(String),
    /// `{post_id}`, served by `mod post_id` with `module_param!(post_id)`.
    Param(String),
}

impl Segment {
    fn module(&self) -> String {
        match self {
            Segment::Static(text) => text.replace('-', "_"),
            Segment::Param(name) => name.clone(),
        }
    }
}

fn add_page(args: PageArgs) -> Result<serde_json::Value, Failure> {
    let root = find_app(args.dir.as_deref())?;
    let segments = parse_path(&args.path)?;
    if segments.is_empty() {
        return Err(
            Failure::new("page_exists", "the home page, /, is already in src/app.rs")
                .fix("edit the `home` page in src/app.rs, or give a path such as /about"),
        );
    }
    if let Some(role) = &args.role
        && (role.is_empty() || role.contains('"') || role.contains('\\'))
    {
        return Err(Failure::new(
            "invalid_role",
            format!("`{role}` is not a role name"),
        ));
    }

    let mut files = Files::new(&root);
    let modules: Vec<String> = segments.iter().map(Segment::module).collect();

    // Each module above the page, and the page's own, declared in its parent.
    let mut parent = PathBuf::from("src/app.rs");
    for depth in 0..segments.len() {
        let module = &modules[depth];
        // The directory a module's children live in: `src/app/` for
        // `src/app.rs`, `src/app/blog/` for `blog.rs` or `blog/mod.rs`.
        let dir = if parent.file_name().is_some_and(|name| name == "mod.rs") {
            parent.parent().unwrap().to_path_buf()
        } else {
            parent.with_extension("")
        };
        let file = module_file(&root, &dir, module);
        let is_page = depth == segments.len() - 1;
        if !files.exists(&file) && !is_page {
            files.create(&file, module_contents(&segments[..=depth]));
        }
        files.declare(&parent, module)?;
        parent = file;
    }

    // The page itself.
    let file = parent;
    let url = format!(
        "/{}",
        segments
            .iter()
            .map(|segment| match segment {
                Segment::Static(text) => text.clone(),
                Segment::Param(name) => format!("{{{name}}}"),
            })
            .collect::<Vec<_>>()
            .join("/")
    );
    let title = args
        .title
        .clone()
        .unwrap_or_else(|| match segments.last().unwrap() {
            Segment::Static(text) => words(text),
            Segment::Param(name) => words(name),
        });
    let page = PageSpec {
        url: &url,
        file: &file,
        title: &title,
        segments: &segments,
        signed_in: args.auth || args.role.is_some(),
        role: args.role.as_deref(),
    };
    if files.exists(&file) {
        let existing = files.read(&file)?;
        if existing.contains("#[page") {
            return Err(Failure::new(
                "page_exists",
                format!("{} already has a page", file.display()),
            )
            .fix(format!(
                "edit {} instead, or choose another path",
                file.display()
            )));
        }
        let foreign_use = existing
            .lines()
            .any(|line| line.starts_with("use ") && !line.contains("module_param"));
        if foreign_use {
            return Err(Failure::new(
                "module_not_empty",
                format!("{} already has code of its own", file.display()),
            )
            .fix(format!("add a #[page] to {} by hand", file.display())));
        }
        files.extend_module(&file, &page.imports(false), &page.body());
    } else {
        files.create(&file, page.module());
    }

    let href = {
        let path = modules.join("::");
        let params: Vec<String> = segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Param(name) => {
                    let depth = modules.iter().position(|m| m == name).unwrap();
                    Some(format!(
                        "crate::app::{}::{}(..)",
                        modules[..=depth].join("::"),
                        pascal(name)
                    ))
                }
                Segment::Static(_) => None,
            })
            .collect();
        if params.is_empty() {
            format!("href!(crate::app::{path}::page)")
        } else {
            format!("href!(crate::app::{path}::page, {})", params.join(", "))
        }
    };

    let (created, modified) = if args.dry_run {
        files.plan()
    } else {
        let changed = files.write()?;
        // Formatting needs rustfmt on PATH; an unformatted page still works.
        let paths: Vec<String> = changed
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        let mut fmt = vec!["fmt", "--rustfmt"];
        fmt.extend(paths.iter().map(String::as_str));
        let _ = topcoat(Some(&root), &fmt);
        files.plan()
    };

    let mut next = vec!["cargo check".to_owned()];
    if !url.contains('{') {
        next.push(format!("open http://localhost:3000{url}"));
    }
    Ok(json!({
        "ok": true,
        "dry_run": args.dry_run,
        "url": url,
        "file": file.display().to_string(),
        "href": href,
        "title": title,
        "signed_in": page.signed_in,
        "role": args.role,
        "params": segments.iter().filter_map(|s| match s { Segment::Param(n) => Some(n), _ => None }).collect::<Vec<_>>(),
        "created": created,
        "modified": modified,
        "next": next,
    }))
}

/// The app's directory: the nearest with `Cargo.toml` and `src/app.rs`.
pub(crate) fn find_app(start: Option<&Path>) -> Result<PathBuf, Failure> {
    let start = match start {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|error| Failure::new("internal", format!("no current directory: {error}")))?,
    };
    let start = fs::canonicalize(&start)
        .map_err(|error| Failure::new("not_an_app", format!("{}: {error}", start.display())))?;
    start
        .ancestors()
        .find(|dir| dir.join("Cargo.toml").is_file() && dir.join("src/app.rs").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            Failure::new(
                "not_an_app",
                format!("{} is not in an app made by `custard new`", start.display()),
            )
            .fix("run this in the app's directory, or pass --dir")
        })
}

fn parse_path(path: &str) -> Result<Vec<Segment>, Failure> {
    let invalid = |why: String| {
        Failure::new(
            "invalid_path",
            format!("`{path}` cannot be a page's path: {why}"),
        )
        .fix("use lowercase words joined by `-`, and `{name}` for parameters, e.g. /blog/{post_id}")
    };
    if !path.starts_with('/') {
        return Err(invalid("it must start with `/`".into()));
    }
    let mut segments = Vec::new();
    let mut params = Vec::new();
    for text in path.split('/').filter(|text| !text.is_empty()) {
        let segment = if let Some(name) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
            if name.starts_with('*') {
                return Err(invalid(
                    "catch-all parameters are not supported here".into(),
                ));
            }
            let ident = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !ident {
                return Err(invalid(format!("`{name}` must be snake_case")));
            }
            if params.contains(&name) {
                return Err(invalid(format!("`{name}` appears twice")));
            }
            params.push(name);
            Segment::Param(name.to_owned())
        } else {
            let valid = text.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && !text.ends_with('-')
                && !text.contains("--")
                && text
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !valid {
                return Err(invalid(format!(
                    "`{text}` must be lowercase words joined by `-`"
                )));
            }
            Segment::Static(text.to_owned())
        };
        if KEYWORDS.contains(&segment.module().as_str()) {
            return Err(invalid(format!(
                "`{}` is a Rust keyword, so no module can be named it",
                segment.module()
            )));
        }
        segments.push(segment);
    }
    Ok(segments)
}

pub(crate) const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "try", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield",
];

/// `src/app/blog/posts.rs`, or `src/app/blog/posts/mod.rs` where that is the
/// one that exists.
fn module_file(root: &Path, dir: &Path, module: &str) -> PathBuf {
    let flat = dir.join(format!("{module}.rs"));
    let nested = dir.join(module).join("mod.rs");
    if !root.join(&flat).exists() && root.join(&nested).exists() {
        nested
    } else {
        flat
    }
}

/// A module above the page, which only holds its children and, for a
/// parameter, declares it.
fn module_contents(path: &[Segment]) -> String {
    let url = url_of(path);
    match path.last().unwrap() {
        Segment::Static(_) => format!("//! Pages under `{url}`.\n"),
        Segment::Param(name) => format!(
            "//! Pages under `{url}`.\n\nuse topcoat::router::module_param;\n\nmodule_param!(pub {name});\n"
        ),
    }
}

fn url_of(path: &[Segment]) -> String {
    let parts: Vec<String> = path
        .iter()
        .map(|segment| match segment {
            Segment::Static(text) => text.clone(),
            Segment::Param(name) => format!("{{{name}}}"),
        })
        .collect();
    format!("/{}", parts.join("/"))
}

/// The path from the page's module to the module declaring `param`:
/// `super::super` for a grandparent, `self` for the page's own.
fn ancestor_path(modules: &[String], param: &Segment) -> String {
    let depth = modules
        .iter()
        .position(|module| *module == param.module())
        .unwrap();
    let up = modules.len() - 1 - depth;
    if up == 0 {
        "self".to_owned()
    } else {
        vec!["super"; up].join("::")
    }
}

pub(crate) fn pascal(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

struct PageSpec<'a> {
    url: &'a str,
    file: &'a Path,
    title: &'a str,
    segments: &'a [Segment],
    signed_in: bool,
    role: Option<&'a str>,
}

impl PageSpec<'_> {
    fn params(&self) -> Vec<&str> {
        self.segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Param(name) => Some(name.as_str()),
                Segment::Static(_) => None,
            })
            .collect()
    }

    fn needs_cx(&self) -> bool {
        self.signed_in || !self.params().is_empty()
    }

    /// The `use` the page needs; `module_param` too when the file declares
    /// its own parameter and has not imported it already.
    fn imports(&self, module_param: bool) -> String {
        let mut router = Vec::new();
        if module_param {
            router.push("module_param");
        }
        router.push("page");
        if !self.params().is_empty() {
            router.push("path_param");
        }
        let mut uses = vec!["Result".to_owned()];
        if self.needs_cx() {
            uses.push("context::Cx".to_owned());
        }
        uses.push(format!("router::{{{}}}", router.join(", ")));
        uses.push("view::{View, view}".to_owned());
        format!("use topcoat::{{{}}};\n", uses.join(", "))
    }

    /// A new file holding just the page.
    fn module(&self) -> String {
        let own_param = matches!(self.segments.last(), Some(Segment::Param(_)));
        let mut out = format!(
            "//! `{}`: {}.\n\n{}",
            self.url,
            self.title,
            self.imports(own_param)
        );
        if let Some(Segment::Param(name)) = self.segments.last() {
            out.push_str(&format!("\nmodule_param!(pub {name});\n"));
        }
        out.push_str(&self.body());
        out
    }

    /// The page function.
    fn body(&self) -> String {
        let mut lets = Vec::new();
        let mut shown = Vec::new();
        if let Some(role) = self.role {
            lets.push(format!(
                "    let user = custard::auth::require_role(cx, {role:?}).await?;\n    let signed_in_as = user.email.clone();"
            ));
            shown.push(format!(
                "<p class=\"text-sm text-muted-foreground\">\"Signed in as \" (signed_in_as) \", with the {role} role.\"</p>"
            ));
        } else if self.signed_in {
            lets.push(
                "    let user = custard::auth::require_user(cx).await?;\n    let signed_in_as = user.email.clone();"
                    .to_owned(),
            );
            shown.push(
                "<p class=\"text-sm text-muted-foreground\">\"Signed in as \" (signed_in_as)</p>"
                    .to_owned(),
            );
        }
        let modules: Vec<String> = self.segments.iter().map(Segment::module).collect();
        for segment in self.segments {
            if let Segment::Param(name) = segment {
                let path = ancestor_path(&modules, segment);
                let ty = if path == "self" {
                    pascal(name)
                } else {
                    format!("{path}::{}", pascal(name))
                };
                lets.push(format!(
                    "    let {name} = path_param::<{ty}>(cx).to_owned();"
                ));
                shown.push(format!(
                    "<p><span class=\"text-muted-foreground\">\"{name}: \"</span> ({name})</p>"
                ));
            }
        }
        let args = if self.needs_cx() { "cx: &Cx" } else { "" };
        let mut body = format!("\n#[page]\npub async fn page({args}) -> Result<impl View> {{\n");
        for line in &lets {
            body.push_str(line);
            body.push('\n');
        }
        if !lets.is_empty() {
            body.push('\n');
        }
        body.push_str(&format!(
            "    Ok(view! {{\n        <main class=\"mx-auto max-w-3xl space-y-6 px-4 py-12\">\n            <h1 class=\"text-2xl font-semibold\">{title:?}</h1>\n",
            title = self.title
        ));
        for line in &shown {
            body.push_str("            ");
            body.push_str(line);
            body.push('\n');
        }
        body.push_str(&format!(
            "            <p class=\"text-muted-foreground\">\"Edit \" <code>{file:?}</code> \" to change this page.\"</p>\n        </main>\n    }})\n}}\n",
            file = self.file.display().to_string()
        ));
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paths() {
        assert_eq!(
            parse_path("/blog-posts/{post_id}").unwrap(),
            [
                Segment::Static("blog-posts".into()),
                Segment::Param("post_id".into())
            ]
        );
        assert!(parse_path("/").unwrap().is_empty());
        for bad in [
            "about",
            "/About",
            "/blog_posts",
            "/a--b",
            "/{Id}",
            "/{*rest}",
            "/type",
            "/{id}/{id}",
        ] {
            assert!(parse_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_params() {
        assert_eq!(pascal("post_id"), "PostId");
        let modules: Vec<String> = ["blog", "post_id", "edit"].map(String::from).to_vec();
        assert_eq!(
            ancestor_path(&modules, &Segment::Param("post_id".into())),
            "super"
        );
    }
}
