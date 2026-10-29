//! The app's PostgreSQL database.
//!
//! `DATABASE_URL` reaches it through the platform's pooler (PgBouncer, in
//! transaction mode), so nothing that outlives a transaction works on it:
//! session settings, advisory locks, `LISTEN`. Migrations use
//! `DIRECT_DATABASE_URL` instead, and run once per deploy as the app's
//! release command (`./bin/<app> migrate`), never as each server starts.

use std::{str::FromStr, time::Duration};

use sqlx::{
    PgPool,
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use topcoat::context::{Cx, try_app_context};

/// The contract version this library implements (`_custard.info`).
pub const CONTRACT_VERSION: i32 = 1;

/// A handle to the database, shared by every request.
#[derive(Clone, Debug)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// A pool that connects when first used, so the app starts while the
    /// database is still coming up; `/up` reports whether it can be reached.
    pub fn connect_lazy(url: &str) -> Result<Db, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy_with(options(url)?);
        Ok(Db { pool })
    }

    /// Connects now, for one-off work such as migrating.
    pub async fn connect(url: &str) -> Result<Db, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(options(url)?)
            .await?;
        Ok(Db { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Whether a query gets through.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    /// Applies the migrations not yet applied.
    pub async fn migrate(&self, migrator: &Migrator) -> Result<(), sqlx::migrate::MigrateError> {
        migrator.run(&self.pool).await
    }

    /// The contract version the platform set this database up for, if it says.
    pub async fn contract_version(&self) -> Option<i32> {
        sqlx::query_scalar::<_, i32>("SELECT contract_version FROM _custard.info LIMIT 1")
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }
}

/// The database pool, for queries in pages and routes:
///
/// ```rust,ignore
/// let notes = sqlx::query_as::<_, Note>("SELECT * FROM notes")
///     .fetch_all(custard::db(cx))
///     .await?;
/// ```
///
/// # Panics
///
/// When the app has no database: `DATABASE_URL` is not set.
#[track_caller]
pub fn db(cx: &Cx) -> &PgPool {
    match try_app_context::<Db>(cx) {
        Some(db) => db.pool(),
        None => panic!(
            "this app has no database: set DATABASE_URL (see .env.example), \
             or turn the database on in the project's settings on The Custard Factory"
        ),
    }
}

/// The database, if the app has one.
pub fn try_db(cx: &Cx) -> Option<&PgPool> {
    try_app_context::<Db>(cx).map(Db::pool)
}

// Options from a URL, accepting what the platform gives developers:
// `sslrootcert=system` asks for the system's authorities, which is what
// sqlx does anyway when no file is named.
fn options(url: &str) -> Result<PgConnectOptions, sqlx::Error> {
    PgConnectOptions::from_str(&without_system_root(url))
}

fn without_system_root(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_owned();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, value)| !(key == "sslrootcert" && value == "system"))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    parsed.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_only_the_system_root() {
        assert_eq!(
            without_system_root("postgres://u:p@h:5432/db?sslmode=verify-full&sslrootcert=system"),
            "postgres://u:p@h:5432/db?sslmode=verify-full"
        );
        assert_eq!(
            without_system_root("postgres://u:p@h/db?sslrootcert=system"),
            "postgres://u:p@h/db"
        );
        assert_eq!(
            without_system_root("postgres://u:p@h/db?sslrootcert=custard-gateway-ca.pem"),
            "postgres://u:p@h/db?sslrootcert=custard-gateway-ca.pem"
        );
    }
}
