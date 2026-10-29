//! Running an app: `main` hands its router to [`App::run`], which reads the
//! platform's settings, connects what the app has, and answers the commands
//! The Custard Factory and its developers run.
//!
//! | Command | Does |
//! | --- | --- |
//! | `./bin/<app>` | Serves the app on `HOST` and `PORT` |
//! | `./bin/<app> migrate` | Applies the database migrations: the app's release command |
//! | `./bin/<app> assets bundle` | Writes the asset bundle beside the binary, once it is built |

use std::{process::ExitCode, sync::Arc, time::Duration};

use sqlx::migrate::Migrator;
use topcoat::{
    context::Cx,
    cookie::RouterBuilderCookieExt as _,
    router::{
        Body, Method, RouteFn, RouteFuture, RouterBuilder, StatusCode, response::IntoResponse as _,
    },
};
use topcoat_asset::{Bundler, BundlerConfig};

use crate::{
    auth::{Auth, User},
    db::{CONTRACT_VERSION, Db},
    env::Env,
    storage::Storage,
    telemetry::{self, RequestTelemetry},
};

/// What the platform gives this app, connected. Passed to the app's router
/// function, which adds it with [`RouterBuilderCustardExt::custard`].
#[derive(Clone)]
pub struct Platform {
    pub env: Env,
    pub db: Option<Db>,
    pub auth: Option<Arc<Auth>>,
    pub storage: Option<Storage>,
}

/// The app, as `main` describes it.
pub struct App {
    name: &'static str,
    version: &'static str,
    migrator: Option<Migrator>,
}

impl App {
    /// Use the package's own name and version:
    /// `App::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))`.
    pub fn new(name: &'static str, version: &'static str) -> App {
        App {
            name,
            version,
            migrator: None,
        }
    }

    /// The app's migrations: `sqlx::migrate!()`, from `migrations/`.
    pub fn migrations(mut self, migrator: Migrator) -> App {
        self.migrator = Some(migrator);
        self
    }

    /// Runs the command the process was started with, serving by default.
    pub async fn run<F>(self, router: F) -> ExitCode
    where
        F: FnOnce(&Platform) -> RouterBuilder,
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match args.as_slice() {
            [] | ["serve"] => self.serve(router).await,
            ["migrate"] => self.migrate().await,
            ["assets", "bundle"] => bundle_assets(),
            ["help" | "--help" | "-h"] => {
                println!("{}", HELP.replace("<app>", self.name));
                ExitCode::SUCCESS
            }
            _ => {
                eprintln!("{}", HELP.replace("<app>", self.name));
                ExitCode::from(2)
            }
        }
    }

    async fn serve<F>(self, router: F) -> ExitCode
    where
        F: FnOnce(&Platform) -> RouterBuilder,
    {
        let env = Env::load();
        let _telemetry = telemetry::init(self.name, self.version, env.telemetry);

        let db = match &env.database_url {
            Some(url) => match Db::connect_lazy(url) {
                Ok(db) => Some(db),
                Err(error) => {
                    tracing::error!(%error, "DATABASE_URL cannot be used");
                    return ExitCode::FAILURE;
                }
            },
            None => {
                tracing::info!("no DATABASE_URL: this app has no database");
                None
            }
        };

        // While developing, apply migrations as the app starts. Deployed, the
        // release command does it once, before any server starts.
        if cfg!(debug_assertions)
            && let (Some(migrator), Some(url)) = (&self.migrator, &env.direct_database_url)
        {
            match Db::connect(url).await {
                Ok(direct) => {
                    if let Err(error) = direct.migrate(migrator).await {
                        tracing::error!(%error, "the migrations failed");
                        return ExitCode::FAILURE;
                    }
                    check_contract(&direct).await;
                }
                Err(error) => tracing::warn!(%error, "could not reach the database to migrate it"),
            }
        }

        let auth = match &env.auth {
            Some(settings) => Some(Arc::new(Auth::new(settings.clone(), &env.app_origin))),
            None => dev_user().map(|user| {
                tracing::warn!(email = %user.email, "no sign-in service: everyone is signed in as CUSTARD_DEV_USER");
                Arc::new(Auth::dev(user, &env.app_origin))
            }),
        };
        if auth.is_none() {
            tracing::info!("no CUSTARD_AUTH_*: signing in is off");
        }

        let storage = match &env.storage {
            Some(settings) => match Storage::new(settings) {
                Ok(storage) => Some(storage),
                Err(error) => {
                    tracing::error!(%error, "the S3_* variables cannot be used");
                    return ExitCode::FAILURE;
                }
            },
            None => None,
        };

        let platform = Platform {
            env,
            db,
            auth,
            storage,
        };
        let router = router(&platform).build();
        tracing::info!(origin = %platform.env.app_origin, "{} {} is starting", self.name, self.version);
        match topcoat::start(router).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "the server stopped");
                ExitCode::FAILURE
            }
        }
    }

    async fn migrate(self) -> ExitCode {
        let env = Env::load();
        let _telemetry = telemetry::init(self.name, self.version, env.telemetry);
        let Some(migrator) = &self.migrator else {
            tracing::info!("this app has no migrations");
            return ExitCode::SUCCESS;
        };
        let Some(url) = &env.direct_database_url else {
            tracing::error!("DIRECT_DATABASE_URL (or DATABASE_URL) is not set");
            return ExitCode::FAILURE;
        };
        let db = match Db::connect(url).await {
            Ok(db) => db,
            Err(error) => {
                tracing::error!(%error, "could not reach the database");
                return ExitCode::FAILURE;
            }
        };
        check_contract(&db).await;
        match db.migrate(migrator).await {
            Ok(()) => {
                tracing::info!("the database is up to date");
                ExitCode::SUCCESS
            }
            Err(error) => {
                tracing::error!(%error, "the migrations failed");
                ExitCode::FAILURE
            }
        }
    }
}

const HELP: &str = "Usage: <app> [command]

Commands:
  serve           Serve the app on HOST and PORT (the default)
  migrate         Apply the database migrations, from DIRECT_DATABASE_URL
  assets bundle   Write the asset bundle beside this binary";

async fn check_contract(db: &Db) {
    if let Some(version) = db.contract_version().await
        && version > CONTRACT_VERSION
    {
        tracing::warn!(
            version,
            supported = CONTRACT_VERSION,
            "the platform set this database up for a newer contract than this version of custard knows"
        );
    }
}

/// `CUSTARD_DEV_USER=email` signs everyone in as that user while developing
/// without the sign-in service, with the roles in `CUSTARD_DEV_ROLES`.
fn dev_user() -> Option<User> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let email = std::env::var("CUSTARD_DEV_USER")
        .ok()
        .filter(|email| !email.is_empty())?;
    let mut roles: Vec<String> = std::env::var("CUSTARD_DEV_ROLES")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .map(str::to_owned)
        .collect();
    roles.sort();
    Some(User {
        user_id: format!("dev:{email}"),
        name: email.split('@').next().unwrap_or("dev").to_owned(),
        email,
        email_verified: true,
        org_id: "dev".into(),
        org_name: "Development".into(),
        roles,
    })
}

/// Writes the asset bundle beside this binary, as `AssetBundle::load` expects.
/// Run once the binary is built, where its sources still are.
fn bundle_assets() -> ExitCode {
    let result = std::env::current_exe().and_then(|exe| {
        let out = exe
            .parent()
            .expect("a binary is in a directory")
            .join("assets");
        let binary = std::fs::read(&exe)?;
        let config =
            BundlerConfig::new().cache_dir(std::env::temp_dir().join("custard-asset-cache"));
        Bundler::new(&config)
            .bundle(&binary, &out)
            .map_err(std::io::Error::other)?;
        Ok(out)
    });
    match result {
        Ok(out) => {
            println!("bundled the assets into {}", out.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("could not bundle the assets: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Adds the platform to a router.
pub trait RouterBuilderCustardExt {
    /// Cookies, a span for every request, `/up` for health checks, signing in
    /// under `/_custard/auth`, and the database, sign-in service and bucket
    /// for pages to reach with [`crate::db`], [`crate::auth`] and
    /// [`crate::storage`]. Add it before `.runtime()`.
    fn custard(self, platform: &Platform) -> Self;
}

impl RouterBuilderCustardExt for RouterBuilder {
    fn custard(self, platform: &Platform) -> Self {
        let mut router = self.cookies().layer(RequestTelemetry).route(health_route());
        for route in crate::auth::routes() {
            router = router.route(route);
        }
        if let Some(db) = &platform.db {
            router = router.app_context(db.clone());
        }
        if let Some(auth) = &platform.auth {
            router = router.app_context(auth.clone());
        }
        if let Some(storage) = &platform.storage {
            router = router.app_context(storage.clone());
        }
        router.app_context(platform.env.clone())
    }
}

/// `GET /up`: 200 while the app can reach its database, 503 when it cannot.
fn health_route() -> RouteFn {
    fn up(cx: &Cx, _body: Body) -> RouteFuture<'_> {
        Box::pin(async move {
            let healthy = match topcoat::context::try_app_context::<Db>(cx) {
                Some(db) => matches!(
                    tokio::time::timeout(Duration::from_secs(3), db.ping()).await,
                    Ok(Ok(()))
                ),
                None => true,
            };
            if healthy {
                (StatusCode::OK, "ok").into_response(cx)
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the database cannot be reached",
                )
                    .into_response(cx)
            }
        })
    }
    RouteFn::new(Method::GET, "/up", up)
}
