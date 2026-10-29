mod app;
// Components are copied whole from Topcoat UI, so not every function is used.
#[allow(dead_code)]
mod components;
mod features;

/// `./{{name}}` serves the app, `./{{name}} migrate` applies the migrations in
/// `migrations/`, and `./{{name}} assets bundle` writes the asset bundle.
#[tokio::main]
async fn main() -> std::process::ExitCode {
    custard::App::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        .migrations(sqlx::migrate!())
        .run(app::router)
        .await
}
