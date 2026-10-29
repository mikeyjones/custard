//! Custard: [Topcoat](https://github.com/tokio-rs/topcoat) apps that run on
//! The Custard Factory.
//!
//! An app's `main` hands its router to [`App::run`]:
//!
//! ```rust,ignore
//! #[tokio::main]
//! async fn main() -> std::process::ExitCode {
//!     custard::App::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
//!         .migrations(sqlx::migrate!())
//!         .run(app::router)
//!         .await
//! }
//! ```
//!
//! and its router adds the platform with
//! [`.custard(platform)`](RouterBuilderCustardExt::custard). Pages then reach
//! what the platform gives the app:
//!
//! - [`db`]: the PostgreSQL database (`DATABASE_URL`)
//! - [`auth`]: the person signed in, through the environment's sign-in service
//!   (`CUSTARD_AUTH_*`)
//! - [`storage()`]: the environment's bucket (`S3_*`)
//! - [`telemetry`]: logs, traces and errors, sent to the platform
//!   (`OTEL_*`)

mod app;
pub mod auth;
pub mod db;
pub mod env;
pub mod form;
pub mod storage;
pub mod telemetry;

pub use app::{App, Platform, RouterBuilderCustardExt};
pub use auth::User;
pub use db::{db, try_db};
pub use env::Env;
pub use storage::{Storage, storage, try_storage};
