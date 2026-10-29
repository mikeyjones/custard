//! `custard add feature` and `custard add migration`.
//!
//! A feature is what one part of an app owns: its table (a migration), its
//! data and queries (`src/features/<name>.rs`), and, unless told otherwise,
//! the pages to list, add, change and delete its rows (`src/app/<name>.rs` and
//! `src/app/<name>/<item>_id.rs`).

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use serde_json::json;

use crate::{
    add::{KEYWORDS, find_app, pascal},
    common::{Failure, finish, topcoat},
    files::Files,
    new::words,
};

#[derive(clap::Args)]
pub struct FeatureArgs {
    /// The feature's name: plural, snake_case, such as `products` or
    /// `blog_posts`. It names the table, the module and the pages' URL.
    name: String,
    /// A field, as `name:type`, or `name:type?` for an optional one. Types:
    /// text, int, float, bool, date. Repeat for each field.
    #[arg(long = "field", value_name = "NAME:TYPE")]
    fields: Vec<String>,
    /// What one row is called, when it is not the name without its final
    /// `s`, such as `--singular person` for `people`.
    #[arg(long)]
    singular: Option<String>,
    /// Keep rows apart for each signed-in user, or each organisation.
    #[arg(long, value_enum)]
    owned_by: Option<Owner>,
    /// Only people with this role may use the pages.
    #[arg(long)]
    role: Option<String>,
    /// Only the table and the data, no pages.
    #[arg(long)]
    no_pages: bool,
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

#[derive(clap::Args)]
pub struct MigrationArgs {
    /// What the migration does, in a few words: `add a price to products`.
    #[arg(required = true, num_args = 1..)]
    description: Vec<String>,
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

#[derive(Clone, Copy, Debug, PartialEq, clap::ValueEnum)]
enum Owner {
    User,
    Org,
}

impl Owner {
    /// The column, and the `User` field it comes from.
    fn column(self) -> &'static str {
        match self {
            Owner::User => "user_id",
            Owner::Org => "org_id",
        }
    }
}

// -- add migration ----------------------------------------------------------------

pub fn run_migration(args: MigrationArgs) -> ExitCode {
    let json = args.json;
    finish(json, add_migration(args), |report| {
        let verb = if report["dry_run"] == true {
            "Would add"
        } else {
            "Added"
        };
        format!("{verb} {}", report["file"].as_str().unwrap_or_default())
    })
}

fn add_migration(args: MigrationArgs) -> Result<serde_json::Value, Failure> {
    let root = find_app(args.dir.as_deref())?;
    let description = args.description.join(" ");
    let slug = slug(&description);
    if slug.is_empty() {
        return Err(
            Failure::new("invalid_description", "the description has no words")
                .fix("describe the change, such as: custard add migration add a price to products"),
        );
    }
    let file = migration_file(&root, &slug)?;
    let mut files = Files::new(&root);
    files.create(&file, format!("-- {}\n\n", sentence(&description)));
    if !args.dry_run {
        files.write()?;
    }
    Ok(json!({
        "ok": true,
        "dry_run": args.dry_run,
        "file": file.display().to_string(),
        "next": [
            format!("write the SQL in {}", file.display()),
            "custard dev applies it as the app starts; deployed, the release command does",
        ],
    }))
}

/// `migrations/<version>_<slug>.sql`, with a version after every existing one.
fn migration_file(root: &Path, slug: &str) -> Result<PathBuf, Failure> {
    let now: u64 = chrono::Utc::now()
        .format("%Y%m%d%H%M%S")
        .to_string()
        .parse()
        .unwrap();
    let latest = fs::read_dir(root.join("migrations"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.split('_').next()?.parse::<u64>().ok()
        })
        .max()
        .unwrap_or(0);
    let version = now.max(latest + 1);
    Ok(PathBuf::from(format!("migrations/{version}_{slug}.sql")))
}

fn slug(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("_")
}

fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// -- add feature ------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Text,
    Int,
    Float,
    Bool,
    Date,
}

#[derive(Clone, Debug)]
struct Field {
    name: String,
    kind: Kind,
    optional: bool,
}

impl Field {
    fn parse(spec: &str) -> Result<Field, Failure> {
        let invalid = |why: &str| {
            Failure::new("invalid_field", format!("`{spec}` is not a field: {why}")).fix(
                "write fields as name:type or name:type?, with types text, int, float, bool and date",
            )
        };
        let (name, kind) = spec
            .split_once(':')
            .ok_or_else(|| invalid("it has no type"))?;
        let (kind, optional) = match kind.strip_suffix('?') {
            Some(kind) => (kind, true),
            None => (kind, false),
        };
        let kind = match kind {
            "text" | "string" => Kind::Text,
            "int" | "integer" => Kind::Int,
            "float" | "number" => Kind::Float,
            "bool" | "boolean" => Kind::Bool,
            "date" => Kind::Date,
            _ => {
                return Err(invalid(
                    "its type is not one of text, int, float, bool or date",
                ));
            }
        };
        if kind == Kind::Bool && optional {
            return Err(invalid(
                "a bool is always true or false, so it cannot be optional",
            ));
        }
        check_identifier(name, "field").map_err(|why| invalid(&why))?;
        if ["id", "created_at", "updated_at", "user_id", "org_id"].contains(&name) {
            return Err(invalid("every feature has this column already"));
        }
        Ok(Field {
            name: name.to_owned(),
            kind,
            optional,
        })
    }

    fn sql(&self) -> String {
        let ty = match self.kind {
            Kind::Text => "TEXT",
            Kind::Int => "BIGINT",
            Kind::Float => "DOUBLE PRECISION",
            Kind::Bool => "BOOLEAN NOT NULL DEFAULT false",
            Kind::Date => "DATE",
        };
        if self.kind == Kind::Bool || self.optional {
            format!("{} {ty}", self.name)
        } else {
            format!("{} {ty} NOT NULL", self.name)
        }
    }

    fn rust(&self) -> String {
        let ty = match self.kind {
            Kind::Text => "String",
            Kind::Int => "i64",
            Kind::Float => "f64",
            Kind::Bool => "bool",
            Kind::Date => "NaiveDate",
        };
        if self.optional {
            format!("Option<{ty}>")
        } else {
            ty.to_owned()
        }
    }

    /// The serde attribute a form field needs, if any.
    fn serde(&self) -> Option<&'static str> {
        if self.kind == Kind::Bool {
            Some("#[serde(default, deserialize_with = \"custard::form::checkbox\")]")
        } else if self.optional {
            Some("#[serde(default, deserialize_with = \"custard::form::optional\")]")
        } else {
            None
        }
    }

    fn label(&self) -> String {
        words(&self.name)
    }

    /// The value as text, for showing; `row` is the variable holding it.
    fn shown(&self, row: &str) -> String {
        let value = format!("{row}.{}", self.name);
        match (self.kind, self.optional) {
            (Kind::Bool, _) => format!("if {value} {{ \"Yes\" }} else {{ \"No\" }}"),
            (Kind::Text, false) | (Kind::Int, false) | (Kind::Float, false) => value,
            (Kind::Text, true) => format!("{value}.unwrap_or_default()"),
            (Kind::Date, false) => format!("{value}.to_string()"),
            (_, true) => format!("{value}.map(|value| value.to_string()).unwrap_or_default()"),
        }
    }

    /// The form control for this field, filled from `row` when editing.
    fn control(&self, row: Option<&str>) -> String {
        let label = self.label();
        if self.kind == Kind::Bool {
            let checked = row
                .map(|row| format!(" checked=({row}.{})", self.name))
                .unwrap_or_default();
            return format!(
                "<label class=\"flex items-center gap-2 text-sm\">\n    <input type=\"checkbox\" name=\"{}\"{checked}>\n    {label:?}\n</label>",
                self.name
            );
        }
        let mut attrs = format!("name=\"{}\"", self.name);
        match self.kind {
            Kind::Int => attrs.push_str(" type=\"number\" step=\"1\""),
            Kind::Float => attrs.push_str(" type=\"number\" step=\"any\""),
            Kind::Date => attrs.push_str(" type=\"date\""),
            Kind::Text | Kind::Bool => {}
        }
        if !self.optional {
            attrs.push_str(" required=\"\"");
        }
        if let Some(row) = row {
            let _ = write!(attrs, " value=({})", self.shown(row));
        }
        format!(
            "<label class=\"grid gap-1 text-sm\">\n    {label:?}\n    input(attrs: attributes! {{ {attrs} }})\n</label>"
        )
    }
}

/// Postgres's reserved words, which cannot name a table or column unquoted.
const RESERVED_SQL: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "constraint",
    "create",
    "current_catalog",
    "current_date",
    "current_role",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "from",
    "grant",
    "group",
    "having",
    "in",
    "initially",
    "intersect",
    "into",
    "lateral",
    "leading",
    "limit",
    "localtime",
    "localtimestamp",
    "not",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "placing",
    "primary",
    "references",
    "returning",
    "select",
    "session_user",
    "some",
    "symmetric",
    "system_user",
    "table",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "variadic",
    "when",
    "where",
    "window",
    "with",
];

fn check_identifier(name: &str, what: &str) -> Result<(), String> {
    check_rust_identifier(name, what)?;
    if RESERVED_SQL.contains(&name) {
        return Err(format!("`{name}` is reserved in SQL"));
    }
    Ok(())
}

fn check_rust_identifier(name: &str, what: &str) -> Result<(), String> {
    let valid = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.ends_with('_')
        && !name.contains("__");
    if !valid {
        return Err(format!(
            "a {what} name is snake_case, starting with a letter"
        ));
    }
    if KEYWORDS.contains(&name) {
        return Err(format!("`{name}` is a Rust keyword"));
    }
    Ok(())
}

/// `products` → `product`, `categories` → `category`, `boxes` → `box`.
fn singular(name: &str) -> String {
    let (head, last) = match name.rsplit_once('_') {
        Some((head, last)) => (format!("{head}_"), last),
        None => (String::new(), name),
    };
    let last = if let Some(stem) = last.strip_suffix("ies") {
        format!("{stem}y")
    } else if ["sses", "shes", "ches", "xes", "zes"]
        .iter()
        .any(|end| last.ends_with(end))
    {
        last[..last.len() - 2].to_owned()
    } else if let Some(stem) = last.strip_suffix('s').filter(|stem| !stem.ends_with('s')) {
        stem.to_owned()
    } else {
        last.to_owned()
    };
    format!("{head}{last}")
}

/// Everything generated for one feature.
struct Feature {
    /// `blog_posts`: the module, the table.
    name: String,
    /// `blog_post`.
    singular: String,
    /// `BlogPost`.
    ty: String,
    fields: Vec<Field>,
    owner: Option<Owner>,
    role: Option<String>,
    migration: PathBuf,
}

impl Feature {
    fn url(&self) -> String {
        format!("/{}", self.name.replace('_', "-"))
    }

    fn param(&self) -> String {
        format!("{}_id", self.singular)
    }

    fn columns(&self) -> String {
        let mut columns = vec!["id".to_owned()];
        columns.extend(self.fields.iter().map(|field| field.name.clone()));
        columns.extend(["created_at".to_owned(), "updated_at".to_owned()]);
        columns.join(", ")
    }

    fn uses_dates(&self) -> bool {
        self.fields.iter().any(|field| field.kind == Kind::Date)
    }

    fn migration_sql(&self) -> String {
        let mut columns = vec!["id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY".to_owned()];
        if let Some(owner) = self.owner {
            let whose = match owner {
                Owner::User => {
                    "-- The user it belongs to: their `user_id` from the sign-in service."
                }
                Owner::Org => {
                    "-- The organisation it belongs to: its `org_id` from the sign-in service."
                }
            };
            columns.push(format!("{whose}\n    {} TEXT NOT NULL", owner.column()));
        }
        columns.extend(self.fields.iter().map(Field::sql));
        columns.push("created_at TIMESTAMPTZ NOT NULL DEFAULT now()".into());
        columns.push("updated_at TIMESTAMPTZ NOT NULL DEFAULT now()".into());
        let mut sql = format!(
            "CREATE TABLE {} (\n    {}\n);\n",
            self.name,
            columns.join(",\n    ")
        );
        match self.owner {
            Some(owner) => {
                let column = owner.column();
                let _ = write!(
                    sql,
                    "\nCREATE INDEX {name}_by_{column} ON {name} ({column}, created_at DESC);\n",
                    name = self.name
                );
            }
            None => {
                let _ = write!(
                    sql,
                    "\nCREATE INDEX {name}_by_created_at ON {name} (created_at DESC);\n",
                    name = self.name
                );
            }
        }
        sql
    }

    /// `src/features/<name>.rs`.
    fn module(&self, with_pages: bool) -> String {
        let name = &self.name;
        let ty = &self.ty;
        let columns = self.columns();
        let words_plural = words(name).to_lowercase();
        let words_one = words(&self.singular).to_lowercase();
        let (owner_arg, owner_doc, owner_where, owner_and) = match self.owner {
            Some(owner) => (
                format!(", {}: &str", owner.column()),
                match owner {
                    Owner::User => {
                        "\n//! Each belongs to one user, by their `user_id` from the sign-in service."
                    }
                    Owner::Org => {
                        "\n//! Each belongs to one organisation, by its `org_id` from the sign-in service."
                    }
                },
                format!(" WHERE {} = $1", owner.column()),
                format!(" AND {} = $2", owner.column()),
            ),
            None => (String::new(), "", String::new(), String::new()),
        };
        let bind_owner = self
            .owner
            .map(|owner| format!("\n    .bind({})", owner.column()))
            .unwrap_or_default();

        let mut out = format!(
            "//! {}: kept in the `{name}` table, made by\n//! `{}`.{owner_doc}\n",
            sentence(&words(name)),
            self.migration.display()
        );
        if !with_pages {
            out.push_str("#![allow(dead_code, reason = \"pages and routes call these as they are written\")]\n");
        }
        let chrono = if self.uses_dates() {
            "DateTime, NaiveDate, Utc"
        } else {
            "DateTime, Utc"
        };
        let _ = write!(
            out,
            "\nuse chrono::{{{chrono}}};\nuse serde::Deserialize;\nuse sqlx::{{FromRow, PgPool}};\n"
        );

        // The row.
        let _ = write!(
            out,
            "\n/// A {words_one}, as it is stored.\n#[derive(Clone, Debug, FromRow)]\npub struct {ty} {{\n    pub id: i64,\n"
        );
        for field in &self.fields {
            let _ = writeln!(out, "    pub {}: {},", field.name, field.rust());
        }
        out.push_str("    pub created_at: DateTime<Utc>,\n    pub updated_at: DateTime<Utc>,\n}\n");

        // The input.
        let _ = write!(
            out,
            "\n/// What a {words_one} is made or changed from: a form's fields, or JSON.\n#[derive(Clone, Debug, Deserialize)]\npub struct {ty}Input {{\n"
        );
        for field in &self.fields {
            if let Some(attr) = field.serde() {
                let _ = writeln!(out, "    {attr}");
            }
            let _ = writeln!(out, "    pub {}: {},", field.name, field.rust());
        }
        out.push_str("}\n");

        let offset = usize::from(self.owner.is_some());
        let binds: String = self
            .fields
            .iter()
            .map(|field| format!("\n    .bind(&input.{})", field.name))
            .collect();

        // list
        let _ = write!(
            out,
            "\n/// The {words_plural}, newest first.\npub async fn list(db: &PgPool{owner_arg}) -> sqlx::Result<Vec<{ty}>> {{\n    sqlx::query_as(\n        \"SELECT {columns} FROM {name}{owner_where} ORDER BY created_at DESC\",\n    ){bind_owner}\n    .fetch_all(db)\n    .await\n}}\n"
        );
        // find
        let _ = write!(
            out,
            "\n/// One {words_one}, if there is one with this id.\npub async fn find(db: &PgPool{owner_arg}, id: i64) -> sqlx::Result<Option<{ty}>> {{\n    sqlx::query_as(\"SELECT {columns} FROM {name} WHERE id = ${}{}\")\n        .bind(id){}\n        .fetch_optional(db)\n        .await\n}}\n",
            1,
            owner_and,
            bind_owner.replace("\n    ", "\n        ")
        );
        // create
        let mut insert_columns: Vec<String> = Vec::new();
        if let Some(owner) = self.owner {
            insert_columns.push(owner.column().into());
        }
        insert_columns.extend(self.fields.iter().map(|field| field.name.clone()));
        let placeholders: Vec<String> = (1..=insert_columns.len())
            .map(|n| format!("${n}"))
            .collect();
        let _ = write!(
            out,
            "\n/// Adds a {words_one}.\npub async fn create(db: &PgPool{owner_arg}, input: &{ty}Input) -> sqlx::Result<{ty}> {{\n    sqlx::query_as(\n        \"INSERT INTO {name} ({}) VALUES ({}) RETURNING {columns}\",\n    ){bind_owner}{binds}\n    .fetch_one(db)\n    .await\n}}\n",
            insert_columns.join(", "),
            placeholders.join(", ")
        );
        // update: $1 id, $2 owner, then the fields.
        let sets: Vec<String> = self
            .fields
            .iter()
            .enumerate()
            .map(|(i, field)| format!("{} = ${}", field.name, i + 2 + offset))
            .collect();
        let _ = write!(
            out,
            "\n/// Changes a {words_one}; `None` if there is none with this id.\npub async fn update(\n    db: &PgPool{owner_arg},\n    id: i64,\n    input: &{ty}Input,\n) -> sqlx::Result<Option<{ty}>> {{\n    sqlx::query_as(\n        \"UPDATE {name} SET {}, updated_at = now() WHERE id = $1{owner_and} RETURNING {columns}\",\n    )\n    .bind(id){bind_owner}{binds}\n    .fetch_optional(db)\n    .await\n}}\n",
            sets.join(", ")
        );
        // delete
        let _ = write!(
            out,
            "\n/// Deletes a {words_one}; `false` if there was none with this id.\npub async fn delete(db: &PgPool{owner_arg}, id: i64) -> sqlx::Result<bool> {{\n    let result = sqlx::query(\"DELETE FROM {name} WHERE id = $1{owner_and}\")\n        .bind(id){}\n        .execute(db)\n        .await?;\n    Ok(result.rows_affected() > 0)\n}}\n",
            bind_owner.replace("\n    ", "\n        ")
        );
        out
    }

    /// Who may use the pages, as the first lines of each handler, and the
    /// argument that scopes the queries to them.
    fn guard(&self) -> (String, String) {
        let check = match &self.role {
            Some(role) => format!("custard::auth::require_role(cx, {role:?}).await?"),
            None => "custard::auth::require_user(cx).await?".to_owned(),
        };
        match self.owner {
            Some(owner) => (
                format!("    let user = {check};\n"),
                format!(", &user.{}", owner.column()),
            ),
            None => (format!("    {check};\n"), String::new()),
        }
    }

    /// `src/app/<name>.rs`: the list, and the form to add one.
    fn list_page(&self) -> String {
        let name = &self.name;
        let ty = &self.ty;
        let param = self.param();
        let param_ty = pascal(&param);
        let title = sentence(&words(name));
        let one = words(&self.singular).to_lowercase();
        let (guard, owner) = self.guard();

        let headers: String = self
            .fields
            .iter()
            .map(|field| {
                format!(
                    "                            <th class=\"py-2 font-medium\">{:?}</th>\n",
                    field.label()
                )
            })
            .collect();
        let cells: String = self
            .fields
            .iter()
            .map(|field| {
                format!(
                    "                                <td>({})</td>\n",
                    field.shown("row")
                )
            })
            .collect();
        let controls: String = self
            .fields
            .iter()
            .map(|field| indent(&field.control(None), 16))
            .collect();

        format!(
            r##"//! `{url}`: the {plural}, and adding one.

pub mod {param};

use topcoat::{{
    Result,
    context::Cx,
    router::{{
        content::Form,
        error::{{SeeOther, see_other}},
        href, page, route,
    }},
    view::{{View, attributes, view}},
}};

use crate::{{
    components::{{button::button, input::input}},
    features::{name}::{{self, {ty}Input}},
}};

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {{
{guard}    let rows = {name}::list(custard::db(cx){owner}).await?;

    Ok(view! {{
        <main class="mx-auto max-w-3xl space-y-8 px-4 py-12">
            <h1 class="text-2xl font-semibold">{title:?}</h1>
            if rows.is_empty() {{
                <p class="text-muted-foreground">{empty:?}</p>
            }} else {{
                <table class="w-full text-sm">
                    <thead class="text-left text-muted-foreground">
                        <tr>
                            <th class="py-2 font-medium">"#"</th>
{headers}                        </tr>
                    </thead>
                    <tbody class="divide-y">
                        for row in rows {{
                            <tr>
                                <td class="py-2">
                                    <a
                                        class="font-medium hover:underline"
                                        href=(href!({param}::page, {param}::{param_ty}(row.id)))
                                    >
                                        (row.id)
                                    </a>
                                </td>
{cells}                            </tr>
                        }}
                    </tbody>
                </table>
            }}
            <form method="post" action=(href!(create)) class="grid gap-4 rounded-lg border p-4">
                <h2 class="font-semibold">{add_heading:?}</h2>
{controls}                <div>button(attrs: attributes! {{ type="submit" }}, {add_button:?})</div>
            </form>
        </main>
    }})
}}

#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<{ty}Input>) -> Result<SeeOther> {{
{guard}    let row = {name}::create(custard::db(cx){owner}, &form).await?;
    Ok(see_other(href!({param}::page, {param}::{param_ty}(row.id)).resolve(cx)))
}}
"##,
            url = self.url(),
            plural = words(name).to_lowercase(),
            empty = format!("No {} yet.", words(name).to_lowercase()),
            add_heading = format!("Add a {one}"),
            add_button = format!("Add {one}"),
        )
    }

    /// `src/app/<name>/<singular>_id.rs`: one row, changing it, deleting it.
    fn item_page(&self) -> String {
        let name = &self.name;
        let ty = &self.ty;
        let param = self.param();
        let param_ty = pascal(&param);
        let one = words(&self.singular);
        let (guard, owner) = self.guard();
        let controls: String = self
            .fields
            .iter()
            .map(|field| indent(&field.control(Some("row")), 16))
            .collect();

        format!(
            r#"//! `{url}/{{{param}}}`: one {one_lower}, changing it, and deleting it.

use topcoat::{{
    Result,
    context::Cx,
    router::{{
        content::Form,
        error::{{RouterErrorExt, SeeOther, not_found, see_other}},
        href, module_param, page, path_param, route,
    }},
    view::{{View, attributes, view}},
}};

use crate::{{
    components::{{
        button::{{ButtonVariant, button}},
        input::input,
    }},
    features::{name}::{{self, {ty}Input}},
}};

module_param!(pub {param}: i64, error = not_found);

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {{
{guard}    let id = *path_param::<{param_ty}>(cx)?;
    let row = {name}::find(custard::db(cx){owner}, id)
        .await?
        .ok_or_not_found()?;
    let added = row.created_at.format("%-d %b %Y").to_string();
    let changed = row.updated_at.format("%-d %b %Y, %H:%M").to_string();

    Ok(view! {{
        <main class="mx-auto max-w-3xl space-y-8 px-4 py-12">
            <a class="text-sm text-muted-foreground hover:underline" href=(href!(super::page))>
                {back:?}
            </a>
            <div>
                <h1 class="text-2xl font-semibold">{heading:?} (row.id)</h1>
                <p class="text-sm text-muted-foreground">"Added " (added) ", changed " (changed)</p>
            </div>
            <form method="post" action=(href!(update, {param_ty}(id))) class="grid gap-4 rounded-lg border p-4">
{controls}                <div>button(attrs: attributes! {{ type="submit" }}, "Save")</div>
            </form>
            <form method="post" action=(href!(delete, {param_ty}(id)))>
                button(
                    variant: ButtonVariant::Destructive,
                    attrs: attributes! {{ type="submit" }},
                    {delete_button:?}
                )
            </form>
        </main>
    }})
}}

#[route(POST)]
pub async fn update(cx: &Cx, Form(form): Form<{ty}Input>) -> Result<SeeOther> {{
{guard}    let id = *path_param::<{param_ty}>(cx)?;
    {name}::update(custard::db(cx){owner}, id, &form)
        .await?
        .ok_or_not_found()?;
    Ok(see_other(href!(page, {param_ty}(id)).resolve(cx)))
}}

#[route(POST "./delete")]
pub async fn delete(cx: &Cx) -> Result<SeeOther> {{
{guard}    let id = *path_param::<{param_ty}>(cx)?;
    if !{name}::delete(custard::db(cx){owner}, id).await? {{
        return Err(not_found().into());
    }}
    Ok(see_other(href!(super::page).resolve(cx)))
}}
"#,
            url = self.url(),
            one_lower = one.to_lowercase(),
            back = format!("← {}", sentence(&words(name))),
            heading = format!("{one} #"),
            delete_button = format!("Delete {}", one.to_lowercase()),
        )
    }
}

fn indent(text: &str, by: usize) -> String {
    let pad = " ".repeat(by);
    text.lines().map(|line| format!("{pad}{line}\n")).collect()
}

pub fn run_feature(args: FeatureArgs) -> ExitCode {
    let json = args.json;
    finish(json, add_feature(args), |report| {
        let verb = if report["dry_run"] == true {
            "Would add"
        } else {
            "Added"
        };
        let mut summary = format!(
            "{verb} the {} feature",
            report["name"].as_str().unwrap_or_default()
        );
        for file in report["created"].as_array().into_iter().flatten() {
            let _ = write!(
                summary,
                "\n  created  {}",
                file.as_str().unwrap_or_default()
            );
        }
        for file in report["modified"].as_array().into_iter().flatten() {
            let _ = write!(
                summary,
                "\n  changed  {}",
                file.as_str().unwrap_or_default()
            );
        }
        summary
    })
}

fn add_feature(args: FeatureArgs) -> Result<serde_json::Value, Failure> {
    let root = find_app(args.dir.as_deref())?;
    let invalid_name = |why: String| {
        Failure::new(
            "invalid_name",
            format!("`{}` cannot name a feature: {why}", args.name),
        )
        .fix("use a plural snake_case name, such as products or blog_posts")
    };
    check_identifier(&args.name, "feature").map_err(invalid_name)?;
    let singular_name = args
        .singular
        .clone()
        .unwrap_or_else(|| singular(&args.name));
    check_rust_identifier(&singular_name, "singular").map_err(|why| {
        Failure::new("invalid_singular", format!("`{singular_name}`: {why}")).fix("pass --singular")
    })?;
    if singular_name == args.name {
        return Err(Failure::new(
            "invalid_singular",
            format!(
                "one of `{}` would also be called `{singular_name}`",
                args.name
            ),
        )
        .fix("pass --singular with a different name for one row"));
    }
    if args.fields.is_empty() {
        return Err(
            Failure::new("no_fields", "a feature needs at least one field")
                .fix("add fields such as --field name:text --field price:int"),
        );
    }
    let fields = args
        .fields
        .iter()
        .map(|spec| Field::parse(spec))
        .collect::<Result<Vec<_>, _>>()?;
    for (i, field) in fields.iter().enumerate() {
        if fields[..i].iter().any(|other| other.name == field.name) {
            return Err(Failure::new(
                "invalid_field",
                format!("`{}` is given twice", field.name),
            ));
        }
    }
    if let Some(role) = &args.role
        && (role.is_empty() || role.contains('"') || role.contains('\\'))
    {
        return Err(Failure::new(
            "invalid_role",
            format!("`{role}` is not a role name"),
        ));
    }

    let feature_file = PathBuf::from(format!("src/features/{}.rs", args.name));
    if root.join(&feature_file).exists() {
        return Err(Failure::new(
            "feature_exists",
            format!("{} already exists", feature_file.display()),
        )
        .fix("choose another name, or change the feature by hand"));
    }
    let pages = !args.no_pages;
    let list_file = PathBuf::from(format!("src/app/{}.rs", args.name));
    let item_file = PathBuf::from(format!("src/app/{}/{}_id.rs", args.name, singular_name));
    if pages && (root.join(&list_file).exists() || root.join(list_file.with_extension("")).exists())
    {
        return Err(Failure::new(
            "page_exists",
            format!("{} already exists", list_file.display()),
        )
        .fix("pass --no-pages, or choose another name"));
    }

    let migration = migration_file(&root, &format!("create_{}", args.name))?;
    let feature = Feature {
        ty: pascal(&singular_name),
        singular: singular_name,
        name: args.name.clone(),
        fields,
        owner: args.owned_by,
        role: args.role.clone(),
        migration: migration.clone(),
    };

    let mut files = Files::new(&root);
    files.create(&migration, feature.migration_sql());
    files.create(&feature_file, feature.module(pages));
    files.declare(Path::new("src/features.rs"), &feature.name)?;
    if pages {
        files.create(&list_file, feature.list_page());
        files.create(&item_file, feature.item_page());
        files.declare(Path::new("src/app.rs"), &feature.name)?;
    }

    let missing_components: Vec<&str> = if pages {
        ["button", "input"]
            .into_iter()
            .filter(|component| !root.join(format!("src/components/{component}.rs")).exists())
            .collect()
    } else {
        Vec::new()
    };

    if !args.dry_run {
        let written = files.write()?;
        if !missing_components.is_empty() {
            let mut add = vec!["ui", "add"];
            add.extend(&missing_components);
            topcoat(Some(&root), &add)?;
        }
        let rust: Vec<String> = written
            .iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
            .map(|path| path.display().to_string())
            .collect();
        let mut fmt = vec!["fmt", "--rustfmt"];
        fmt.extend(rust.iter().map(String::as_str));
        let _ = topcoat(Some(&root), &fmt);
    }
    let (created, modified) = files.plan();

    let mut next = vec!["cargo check".to_owned()];
    if pages {
        next.push(format!("open http://localhost:3000{}", feature.url()));
    }
    Ok(json!({
        "ok": true,
        "dry_run": args.dry_run,
        "name": feature.name,
        "type": feature.ty,
        "table": feature.name,
        "migration": migration.display().to_string(),
        "module": format!("crate::features::{}", feature.name),
        "functions": ["list", "find", "create", "update", "delete"],
        "owned_by": feature.owner.map(Owner::column),
        "url": pages.then(|| feature.url()),
        "href": pages.then(|| format!("href!(crate::app::{}::page)", feature.name)),
        "components_added": missing_components,
        "created": created,
        "modified": modified,
        "next": next,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singulars() {
        assert_eq!(singular("products"), "product");
        assert_eq!(singular("categories"), "category");
        assert_eq!(singular("boxes"), "box");
        assert_eq!(singular("addresses"), "address");
        assert_eq!(singular("blog_posts"), "blog_post");
        assert_eq!(singular("news"), "new");
        assert_eq!(singular("glass"), "glass");
    }

    #[test]
    fn fields() {
        let field = Field::parse("price:int?").unwrap();
        assert_eq!((field.kind, field.optional), (Kind::Int, true));
        assert_eq!(field.sql(), "price BIGINT");
        assert_eq!(
            Field::parse("name:text").unwrap().sql(),
            "name TEXT NOT NULL"
        );
        assert_eq!(
            Field::parse("done:bool").unwrap().sql(),
            "done BOOLEAN NOT NULL DEFAULT false"
        );
        for bad in [
            "name",
            "name:blob",
            "done:bool?",
            "order:text",
            "type:text",
            "id:int",
            "Name:text",
        ] {
            assert!(Field::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Add a price to products!"), "add_a_price_to_products");
        assert_eq!(sentence("add a price"), "Add a price");
    }
}
