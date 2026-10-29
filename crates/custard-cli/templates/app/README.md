# {{title}}

A [Topcoat](https://github.com/tokio-rs/topcoat) app built with Custard, ready
to deploy to The Custard Factory with its database, sign-in, file storage and
telemetry.

## Developing

```sh
docker compose up -d   # PostgreSQL, unless you use your workspace on The Custard Factory
custard dev            # builds, bundles assets, serves and reloads on changes
```

Open <http://localhost:3000>. Settings come from `.env` (see `.env.example`).
Without the sign-in service, `CUSTARD_DEV_USER` signs everyone in as one
person; take the `CUSTARD_AUTH_*` variables from the environment's
*My variables* page on The Custard Factory to sign in for real.

## Deploying to The Custard Factory

1. Push this repository and add it to a project as an app.
2. In the app's settings, set the **release command** to
   `./bin/{{name}} migrate`, so migrations run once per deploy before the new
   version serves.
3. Deploy. The platform sets `PORT`, `APP_ORIGIN`, `DATABASE_URL`,
   `CUSTARD_AUTH_*`, `S3_*` and `OTEL_*`; the app reads them itself.

Railpack builds the app with `cargo build --release`, then `railpack.json`
bundles its assets with `./bin/{{name}} assets bundle`.

## Project structure

| Path | Contents |
| --- | --- |
| `src/main.rs` | Starts the app with `custard::App` |
| `src/app.rs`, `src/app/` | The route tree: the router, layouts and pages |
| `src/features.rs`, `src/features/` | What each feature owns: its data and rules |
| `src/components.rs`, `src/components/` | Topcoat UI components, yours to edit |
| `migrations/` | SQL migrations, applied in order |
| `styles.css` | The theme and Tailwind input |
| `railpack.json` | The extra build step for the asset bundle |
