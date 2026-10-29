//! What The Custard Factory tells an app, read once at startup from the
//! environment variables of its contract (`docs/contract.md` in The Custard
//! Factory). Nothing reads the environment per request.

use std::env;

/// The platform's settings for this app.
#[derive(Clone, Debug)]
pub struct Env {
    /// Where people reach the app: scheme, host and port, without a trailing
    /// slash. `APP_ORIGIN`, or `http://localhost:<PORT>` while developing.
    pub app_origin: String,
    /// The app's role in its database, through the pooler: `DATABASE_URL`.
    pub database_url: Option<String>,
    /// The same database, never pooled, for migrations and `LISTEN`:
    /// `DIRECT_DATABASE_URL`, or `DATABASE_URL` when it is not set.
    pub direct_database_url: Option<String>,
    /// The environment's sign-in service, when the project has one.
    pub auth: Option<AuthEnv>,
    /// The environment's bucket, when the project has file storage.
    pub storage: Option<StorageEnv>,
    /// Whether the platform asks for telemetry: `OTEL_EXPORTER_OTLP_ENDPOINT`.
    pub telemetry: bool,
}

/// `CUSTARD_AUTH_*`.
#[derive(Clone, Debug)]
pub struct AuthEnv {
    /// The sign-in service as browsers reach it, and its tokens' `iss`.
    pub issuer: String,
    /// Where this server reaches the same service. Usually the issuer.
    pub url: String,
    pub client_id: String,
    /// Not given to internal apps, which only check bearer tokens.
    pub client_secret: Option<String>,
}

/// `S3_*`.
#[derive(Clone, Debug)]
pub struct StorageEnv {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Where the bucket's files can be read without signing, once it is public.
    pub public_url: Option<String>,
}

impl Env {
    /// Reads the variables, after loading `.env` if there is one. Variables
    /// already set win over the file.
    pub fn load() -> Env {
        let _ = dotenvy::dotenv();
        Env::from_vars(|name| env::var(name).ok())
    }

    /// From any source of variables, such as a test's map. Empty values count
    /// as unset.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Env {
        let var = |name: &str| var(name).filter(|value| !value.trim().is_empty());
        let port = var("PORT").unwrap_or_else(|| "3000".into());
        let app_origin = var("APP_ORIGIN")
            .map(|origin| trim_slash(&origin))
            .unwrap_or_else(|| format!("http://localhost:{port}"));

        let database_url = var("DATABASE_URL");
        let direct_database_url = var("DIRECT_DATABASE_URL").or_else(|| database_url.clone());

        let auth = match (var("CUSTARD_AUTH_ISSUER"), var("CUSTARD_AUTH_CLIENT_ID")) {
            (Some(issuer), Some(client_id)) => {
                let issuer = trim_slash(&issuer);
                Some(AuthEnv {
                    url: var("CUSTARD_AUTH_URL")
                        .map(|url| trim_slash(&url))
                        .unwrap_or_else(|| issuer.clone()),
                    issuer,
                    client_id,
                    client_secret: var("CUSTARD_AUTH_CLIENT_SECRET"),
                })
            }
            _ => None,
        };

        let storage = match (
            var("S3_ENDPOINT"),
            var("S3_BUCKET"),
            var("S3_ACCESS_KEY_ID"),
            var("S3_SECRET_ACCESS_KEY"),
        ) {
            (Some(endpoint), Some(bucket), Some(access_key_id), Some(secret_access_key)) => {
                Some(StorageEnv {
                    endpoint: trim_slash(&endpoint),
                    region: var("S3_REGION").unwrap_or_else(|| "garage".into()),
                    bucket,
                    access_key_id,
                    secret_access_key,
                    public_url: var("S3_PUBLIC_URL").map(|url| trim_slash(&url)),
                })
            }
            _ => None,
        };

        Env {
            app_origin,
            database_url,
            direct_database_url,
            auth,
            storage,
            telemetry: var("OTEL_EXPORTER_OTLP_ENDPOINT").is_some(),
        }
    }

    /// Whether people reach the app over HTTPS, so cookies must be `Secure`.
    pub fn is_https(&self) -> bool {
        self.app_origin.starts_with("https://")
    }
}

fn trim_slash(value: &str) -> String {
    value.trim().trim_end_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(vars: &[(&str, &str)]) -> Env {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Env::from_vars(|name| vars.get(name).cloned())
    }

    #[test]
    fn develops_on_localhost_without_app_origin() {
        assert_eq!(env(&[]).app_origin, "http://localhost:3000");
        assert_eq!(env(&[("PORT", "8080")]).app_origin, "http://localhost:8080");
    }

    #[test]
    fn reads_the_contract() {
        let found = env(&[
            ("APP_ORIGIN", "https://shop.example.com/"),
            ("DATABASE_URL", "postgres://app@pooler/db"),
            ("CUSTARD_AUTH_ISSUER", "https://auth--shop.example.com/"),
            ("CUSTARD_AUTH_CLIENT_ID", "client"),
            ("CUSTARD_AUTH_CLIENT_SECRET", "secret"),
            ("S3_ENDPOINT", "https://s3.example.com"),
            ("S3_BUCKET", "tcf-1-files"),
            ("S3_ACCESS_KEY_ID", "key"),
            ("S3_SECRET_ACCESS_KEY", "shh"),
            ("S3_PUBLIC_URL", ""),
        ]);
        assert_eq!(found.app_origin, "https://shop.example.com");
        assert_eq!(
            found.direct_database_url.as_deref(),
            Some("postgres://app@pooler/db")
        );
        let auth = found.auth.as_ref().unwrap();
        assert_eq!(auth.issuer, "https://auth--shop.example.com");
        assert_eq!(auth.url, auth.issuer);
        let storage = found.storage.as_ref().unwrap();
        assert_eq!(storage.region, "garage");
        assert_eq!(storage.public_url, None);
        assert!(found.is_https());
        assert!(!found.telemetry);
    }
}
