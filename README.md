# Custard

Rust web apps on [Topcoat](https://github.com/tokio-rs/topcoat) that run on
The Custard Factory, with its PostgreSQL database, sign-in service (Zitadel),
telemetry and file storage connected from the start.

```sh
cargo install --path crates/custard-cli
custard new my-shop
cd my-shop
docker compose up -d
custard dev
```

## Crates

| Crate | What it is |
| --- | --- |
| [`custard`](crates/custard) | The library apps use: `custard::App` runs the app; `.custard(platform)` adds the platform to a Topcoat router |
| [`custard-cli`](crates/custard-cli) | The `custard` command: `new`, and Topcoat's `dev`, `fmt`, `ui`, `asset` and `l10n` built in |

## What the library covers

It implements The Custard Factory's contract (`docs/contract.md` in the
platform's repository) for each service:

- **Config**: `APP_ORIGIN`, `HOST`, `PORT`, read once; `.env` while developing.
- **Database**: `custard::db(cx)` is a `sqlx::PgPool` on `DATABASE_URL` (the
  pooler). `./bin/<app> migrate` applies `migrations/` on
  `DIRECT_DATABASE_URL`, as the app's release command; debug builds migrate as
  they start. Accepts the platform's `sslrootcert=system`.
- **Sign-in**: OpenID Connect with PKCE against `CUSTARD_AUTH_*`, under
  `/_custard/auth/{login,callback,logout}`. Tokens are checked locally against
  the service's keys; the session is an encrypted cookie keyed from the client
  secret; expired sessions are renewed with the refresh token. Bearer tokens
  for API clients. `require_user`, `require_role`, `user`. Zitadel's claims are
  read into the contract's names. `CUSTARD_DEV_USER` signs everyone in while
  developing without the service (debug builds only).
- **Telemetry**: `tracing` to standard output always; with
  `OTEL_EXPORTER_OTLP_ENDPOINT`, traces and logs over OTLP/HTTP with the
  platform's token. Every request is a span with `http.route`, `url.path`,
  `http.response.status_code` and `user_agent.original`; 500s and panics are
  recorded as `exception` events for the issue tracker.
- **File storage**: `custard::storage(cx)` on `S3_*` (Garage, path-style): `put`,
  `get`, `delete`, `list`, signed read and upload URLs, `public_url`.
- **Health**: `GET /up`, 503 while the database cannot be reached.
- **Assets**: `./bin/<app> assets bundle` writes Topcoat's asset bundle beside
  the binary, which `railpack.json` runs after Railpack's build.

## Tests

```sh
cargo test --workspace
```

`tests/sign_in.rs` signs in end to end against a fake Zitadel. The storage
test runs against a real S3 server when `CUSTARD_TEST_S3_ENDPOINT`,
`_BUCKET`, `_ACCESS_KEY_ID` and `_SECRET_ACCESS_KEY` are set (a Garage
container works).
