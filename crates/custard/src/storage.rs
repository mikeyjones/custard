//! The environment's bucket: S3-compatible storage (Garage on The Custard
//! Factory), path-style, with its address and key in `S3_*`.
//!
//! ```rust,ignore
//! let files = custard::storage(cx);
//! files.put("avatars/ana.png", bytes, "image/png").await?;
//! let url = files.signed_url("avatars/ana.png", Duration::from_secs(3600));
//! ```
//!
//! The bucket is private: browsers read files through signed URLs that
//! expire, so files never pass through the app on their way out.

use std::time::Duration;

use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle, actions::ListObjectsV2};
use topcoat::context::{Cx, try_app_context};

use crate::env::StorageEnv;

/// How long a request to the bucket may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the URLs this signs for its own requests last.
const SIGN_FOR: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("the bucket could not be reached: {0}")]
    Unreachable(#[from] reqwest::Error),
    #[error("the bucket answered {status}: {body}")]
    Refused { status: u16, body: String },
    #[error("no file at {0}")]
    NotFound(String),
    #[error("the bucket's answer could not be read: {0}")]
    Unreadable(String),
}

/// The bucket, shared by every request.
#[derive(Clone, Debug)]
pub struct Storage {
    bucket: Bucket,
    credentials: Credentials,
    public_url: Option<String>,
    http: reqwest::Client,
}

impl Storage {
    pub fn new(settings: &StorageEnv) -> Result<Storage, String> {
        let endpoint = settings
            .endpoint
            .parse()
            .map_err(|error| format!("S3_ENDPOINT is not a URL: {error}"))?;
        let bucket = Bucket::new(
            endpoint,
            UrlStyle::Path,
            settings.bucket.clone(),
            settings.region.clone(),
        )
        .map_err(|error| format!("S3_BUCKET cannot be used: {error}"))?;
        Ok(Storage {
            bucket,
            credentials: Credentials::new(&settings.access_key_id, &settings.secret_access_key),
            public_url: settings.public_url.clone(),
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("an HTTP client"),
        })
    }

    /// Keeps `bytes` at `key`, replacing what was there.
    pub async fn put(
        &self,
        key: &str,
        bytes: impl Into<reqwest::Body>,
        content_type: &str,
    ) -> Result<(), StorageError> {
        let url = self
            .bucket
            .put_object(Some(&self.credentials), key)
            .sign(SIGN_FOR);
        let response = self
            .http
            .put(url)
            .header("content-type", content_type)
            .body(bytes)
            .send()
            .await?;
        check(response, key).await.map(|_| ())
    }

    /// The file at `key`.
    pub async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        let url = self
            .bucket
            .get_object(Some(&self.credentials), key)
            .sign(SIGN_FOR);
        let response = check(self.http.get(url).send().await?, key).await?;
        Ok(response.bytes().await?.to_vec())
    }

    /// Removes the file at `key`. Removing one that is not there is not an error.
    pub async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let url = self
            .bucket
            .delete_object(Some(&self.credentials), key)
            .sign(SIGN_FOR);
        match check(self.http.delete(url).send().await?, key).await {
            Ok(_) | Err(StorageError::NotFound(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The keys that start with `prefix`, up to a thousand.
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
        action.with_prefix(prefix);
        let url = action.sign(SIGN_FOR);
        let body = check(self.http.get(url).send().await?, prefix)
            .await?
            .text()
            .await?;
        let parsed = ListObjectsV2::parse_response(&body)
            .map_err(|error| StorageError::Unreadable(error.to_string()))?;
        Ok(parsed
            .contents
            .into_iter()
            .map(|object| object.key)
            .collect())
    }

    /// A URL a browser can read the file at `key` from, for `expires_in`.
    pub fn signed_url(&self, key: &str, expires_in: Duration) -> String {
        self.bucket
            .get_object(Some(&self.credentials), key)
            .sign(expires_in)
            .into()
    }

    /// A URL a browser can upload a file to `key` at with `PUT`, for
    /// `expires_in`. The bucket needs CORS rules allowing the app's origin.
    pub fn signed_upload_url(&self, key: &str, expires_in: Duration) -> String {
        self.bucket
            .put_object(Some(&self.credentials), key)
            .sign(expires_in)
            .into()
    }

    /// Where the file at `key` can be read without signing, once the bucket
    /// is public (`S3_PUBLIC_URL`).
    pub fn public_url(&self, key: &str) -> Option<String> {
        self.public_url
            .as_ref()
            .map(|base| format!("{base}/{}", key.trim_start_matches('/')))
    }
}

async fn check(response: reqwest::Response, key: &str) -> Result<reqwest::Response, StorageError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status.as_u16() == 404 {
        return Err(StorageError::NotFound(key.to_owned()));
    }
    let body = response.text().await.unwrap_or_default();
    Err(StorageError::Refused {
        status: status.as_u16(),
        body: body.chars().take(300).collect(),
    })
}

/// The bucket, for pages and routes.
///
/// # Panics
///
/// When the app has no file storage: the `S3_*` variables are not set.
#[track_caller]
pub fn storage(cx: &Cx) -> &Storage {
    match try_app_context::<Storage>(cx) {
        Some(storage) => storage,
        None => panic!(
            "this app has no file storage: set the S3_* variables (see .env.example), \
             or turn file storage on in the project's settings on The Custard Factory"
        ),
    }
}

/// The bucket, if the app has one.
pub fn try_storage(cx: &Cx) -> Option<&Storage> {
    try_app_context::<Storage>(cx)
}
