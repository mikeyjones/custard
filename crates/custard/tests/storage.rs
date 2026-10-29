//! The bucket, against a real S3-compatible server. Runs when
//! `CUSTARD_TEST_S3_ENDPOINT` and the other `CUSTARD_TEST_S3_*` variables
//! name one, such as a Garage container; otherwise it passes without checking.

use std::time::Duration;

use custard::{Storage, env::StorageEnv, storage::StorageError};

fn settings() -> Option<StorageEnv> {
    let var = |name: &str| std::env::var(format!("CUSTARD_TEST_S3_{name}")).ok();
    Some(StorageEnv {
        endpoint: var("ENDPOINT")?,
        region: var("REGION").unwrap_or_else(|| "garage".into()),
        bucket: var("BUCKET")?,
        access_key_id: var("ACCESS_KEY_ID")?,
        secret_access_key: var("SECRET_ACCESS_KEY")?,
        public_url: None,
    })
}

#[tokio::test]
async fn keeps_reads_signs_lists_and_removes_files() {
    let Some(settings) = settings() else {
        eprintln!("CUSTARD_TEST_S3_* not set: skipping");
        return;
    };
    let storage = Storage::new(&settings).unwrap();
    let key = format!("tests/{}/hello world.txt", std::process::id());

    storage
        .put(&key, "Howdy, ünïcode!", "text/plain; charset=utf-8")
        .await
        .unwrap();
    assert_eq!(
        storage.get(&key).await.unwrap(),
        "Howdy, ünïcode!".as_bytes()
    );

    let listed = storage
        .list(&format!("tests/{}/", std::process::id()))
        .await
        .unwrap();
    assert_eq!(listed, std::slice::from_ref(&key));

    // A browser reads it from the signed URL, without the app.
    let url = storage.signed_url(&key, Duration::from_secs(60));
    let response = reqwest::get(&url).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(response.text().await.unwrap(), "Howdy, ünïcode!");

    // And can upload to a signed upload URL.
    let upload = storage.signed_upload_url(&format!("{key}.copy"), Duration::from_secs(60));
    let response = reqwest::Client::new()
        .put(&upload)
        .body("copied")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    assert_eq!(
        storage.get(&format!("{key}.copy")).await.unwrap(),
        b"copied"
    );

    storage.delete(&key).await.unwrap();
    storage.delete(&format!("{key}.copy")).await.unwrap();
    assert!(matches!(
        storage.get(&key).await,
        Err(StorageError::NotFound(_))
    ));
    // Removing what is not there is not an error.
    storage.delete(&key).await.unwrap();
}
