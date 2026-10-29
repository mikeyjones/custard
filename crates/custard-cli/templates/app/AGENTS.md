# {{title}}: notes for coding agents

A Topcoat web app (server-rendered Rust) that runs on The Custard Factory.
Custard (`custard` crate) connects it to the platform: PostgreSQL, sign-in,
file storage and telemetry.

Topcoat's own guide for agents, with its full API:
https://raw.githubusercontent.com/tokio-rs/topcoat/refs/tags/v0.11.0/llms.txt

## Commands

| Command | Does |
| --- | --- |
| `custard dev` | Build, bundle assets, serve on http://localhost:3000, reload on changes |
| `cargo check` | Type-check; run it after every change |
| `custard fmt --rustfmt` | Format Rust and `view!` bodies |
| `custard ui add <component>` | Copy a Topcoat UI component into `src/components/` |
| `custard ui list` | The components available |
| `cargo run -- migrate` | Apply the migrations (also done by `custard dev` as the app starts) |

## Where things go

- **Pages** are `#[page]` functions in `src/app/`. The module path is the URL:
  `src/app/notes.rs` serves `/notes`. Declare each new module with `mod` in its
  parent (`src/app.rs`). Link with `href!(module::page)`, never a literal path.
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
- **Schema changes** are new files in `migrations/`, named
  `<YYYYMMDDHHMMSS>_<description>.sql`. Never edit one that has been deployed.
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

## Rules

- Authorise inside every page, route, shard and procedure that needs it; there
  is no global guard.
- Run `cargo check` after editing, and fix every warning.
- Never commit `.env`.
