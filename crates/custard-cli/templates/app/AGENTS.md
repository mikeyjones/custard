# {{title}}: notes for coding agents

A Topcoat web app (server-rendered Rust) that runs on The Custard Factory.
Custard (`custard` crate) connects it to the platform: PostgreSQL, sign-in,
file storage and telemetry.

Topcoat's own guide for agents, with its full API:
https://raw.githubusercontent.com/tokio-rs/topcoat/refs/tags/v0.11.0/llms.txt

## Commands

| Command | Does |
| --- | --- |
| `custard add page /path [--auth] [--role r] [--title T]` | Add a page, its modules and their `mod` declarations; `{name}` segments are path parameters |
| `custard add feature <plural> --field name:type ... [--owned-by user\|org] [--role r] [--no-pages]` | A table, its migration, `src/features/<name>.rs` with `list`/`find`/`create`/`update`/`delete`, and pages to manage it. Types: `text`, `int`, `float`, `bool`, `date`; add `?` for optional |
| `custard add migration <description>` | An empty, correctly numbered SQL file in `migrations/` |
| `custard dev` | Build, bundle assets, serve on http://localhost:3000, reload on changes |
| `custard check --json` | Everything wrong: Rust errors and warnings, formatting, unreachable modules, literal links, migrations. Each problem has a file, line and fix. Run it after every change |
| `custard routes --json` | Every page and route: URL, file, the `href!` to link to it, who may use it |
| `custard smoke --json [--path /products/1]` | Build, run the app, request every page; failures come with the app's log lines |
| `custard screenshot / /products [--size name=WxH]` | PNGs of pages at desktop and phone sizes, in `target/custard/screenshots/`. Look at them |
| `custard fmt --rustfmt` | Format Rust and `view!` bodies |
| `custard ui add <component>` | Copy a Topcoat UI component into `src/components/` |
| `custard ui list` | The components available |
| `cargo run -- migrate` | Apply the migrations (also done by `custard dev` as the app starts) |

## Where things go

- **Pages** are `#[page]` functions in `src/app/`. The module path is the URL:
  `src/app/notes.rs` serves `/notes`. Start each one with `custard add page`,
  which declares its modules and prints the `href!` to link to it; never write
  a literal path in a link. `custard add page /posts/{post_id}` makes a page
  that reads `path_param::<PostId>(cx)`; pages below it read
  `path_param::<super::PostId>(cx)`.
- **Form posts and API routes** are `#[route(POST)]` functions beside the page,
  taking `Form(form): Form<T>` or `Json(body): Json<T>`, and answering
  `Ok(see_other(href!(page).resolve(cx)))` after a form.
- **Layouts** are `#[layout]` functions; the root one is in `src/app.rs`.
- **Components** are `#[component] async fn`s. Shared ones go in
  `src/components/`; prefer `custard ui add` to writing them from scratch.
  Each component is also a unit struct, so a variable named like an imported
  component (`input`, `button`, `card`) does not compile: name it `new_note`.
- **Data and rules** go in `src/features/<feature>.rs`: plain async functions
  taking `&PgPool`, using `sqlx::query_as` with `#[derive(FromRow)]` structs.
  Pages call them with `custard::db(cx)`. Do not use `sqlx::query!` macros:
  builds have no database to check them against.
- **A new kind of data** starts with `custard add feature`: it writes the
  migration, the queries and the pages together, and they compile. Change them
  from there.
- **Schema changes** are new files from `custard add migration`. Never edit one
  that has been deployed. After changing a table, update its feature's struct,
  input and queries to match.
- **Form fields** that may be blank use
  `#[serde(default, deserialize_with = "custard::form::optional")]`, and
  checkboxes `#[serde(default, deserialize_with = "custard::form::checkbox")]`.
- **Styling** is Tailwind classes in `view!`; the theme's colours are CSS
  variables in `styles.css` (`bg-primary`, `text-muted-foreground`, ...).

## The platform

- `custard::auth::require_user(cx).await?` gives the signed-in `User`
  (`user_id`, `email`, `name`, `org_id`, `org_name`, `roles`), sending a browser
  to sign in first. `custard::auth::user(cx).await` is `Option<&User>`;
  `custard::auth::require_role(cx, "admin").await?` answers 403 to others.
  Store `user.user_id` to refer to someone; the sign-in service owns users, so
  there is no users table.
- `custard::db(cx)` is the `PgPool`. It goes through PgBouncer in transaction
  mode: no `LISTEN`, advisory locks or session `SET`s.
- `custard::storage(cx)` is the bucket: `put`, `get`, `delete`, `list`, and
  `signed_url(key, expires_in)` for browsers to read a file.
- Log with `tracing::info!` and friends; requests are traced already, and errors
  and panics reach the platform's issue tracker.
- Paths under `/_custard` belong to the platform; `/up` is the health check.

## Working loop

1. Make the change, starting from `custard add ...` where one fits.
2. `custard check --json` until it reports no problems.
3. `custard smoke --json` until every page answers.
4. `custard screenshot` the pages you changed, look at the pictures, and fix
   what does not look right: spacing, alignment, contrast, the phone layout.

## Rules

- Authorise inside every page, route, shard and procedure that needs it; there
  is no global guard.
- Run `custard check` after editing, and fix every error and warning.
- Never commit `.env`.
