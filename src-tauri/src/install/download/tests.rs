use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sha1::{Digest, Sha1};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const BODY: &[u8] = b"hyperfarm";

async fn serve() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path("/file"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
        .mount(&server)
        .await;
    server
}

fn sha1(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}

fn ignore() -> Progress {
    Arc::new(|_, _, _| {})
}

#[tokio::test]
async fn rejects_content_that_does_not_match_the_hash_after_retrying() {
    let server = serve().await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("a.jar");
    let task = DownloadTask::new(
        &format!("{}/file", server.uri()),
        target.clone(),
        Some(BODY.len() as u64),
        Some(Hash::sha1(&"00".repeat(20))),
    );

    let error = Downloader::new(reqwest::Client::new())
        .fetch_all(&[task], root.path(), ignore())
        .await
        .unwrap_err();

    assert_eq!(error.problem, Problem::CorruptDownload);
    assert!(!target.exists());
    assert!(!root.path().join("a.jar.part").exists());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        ATTEMPTS as usize
    );
}

#[tokio::test]
async fn skips_satisfied_files_and_reports_only_missing_bytes() {
    let server = serve().await;
    let root = tempfile::tempdir().unwrap();
    let present = root.path().join("present.jar");
    std::fs::write(&present, BODY).unwrap();
    let missing = root.path().join("missing.jar");
    let url = format!("{}/file", server.uri());
    let size = Some(BODY.len() as u64);
    let downloader = Downloader::new(reqwest::Client::new());

    let needed = downloader
        .missing(vec![
            DownloadTask::new(&url, present, size, Some(Hash::sha1(&sha1(BODY)))),
            DownloadTask::new(&url, missing.clone(), size, Some(Hash::sha1(&sha1(BODY)))),
        ])
        .await;
    let done = Arc::new(AtomicU64::new(0));
    let seen = done.clone();
    downloader
        .fetch_all(
            &needed,
            root.path(),
            Arc::new(move |bytes, _, _| seen.store(bytes, Ordering::Relaxed)),
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(&missing).unwrap(), BODY);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(done.load(Ordering::Relaxed), BODY.len() as u64);
}

#[tokio::test]
async fn trusts_content_addressed_files_by_size() {
    let root = tempfile::tempdir().unwrap();
    let asset = root.path().join("asset");
    std::fs::write(&asset, b"different").unwrap();

    let needed = Downloader::new(reqwest::Client::new())
        .missing(vec![DownloadTask::asset(
            "http://127.0.0.1:1/file",
            asset,
            9,
            Hash::sha1(&sha1(BODY)),
        )])
        .await;

    assert!(needed.is_empty());
}

#[tokio::test]
async fn refetches_a_file_whose_content_changed_but_kept_its_size() {
    let root = tempfile::tempdir().unwrap();
    let library = root.path().join("library.jar");
    std::fs::write(&library, b"hyperfaRm").unwrap();

    let needed = Downloader::new(reqwest::Client::new())
        .missing(vec![DownloadTask::new(
            "http://127.0.0.1:1/file",
            library,
            Some(9),
            Some(Hash::sha1(&sha1(BODY))),
        )])
        .await;

    assert_eq!(needed.len(), 1);
}

#[tokio::test]
async fn keeps_going_when_an_optional_file_cannot_be_fetched() {
    let server = serve().await;
    let root = tempfile::tempdir().unwrap();
    let wanted = root.path().join("a.jar");
    let tasks = [
        DownloadTask::new(
            &format!("{}/file", server.uri()),
            wanted.clone(),
            Some(BODY.len() as u64),
            Some(Hash::sha1(&sha1(BODY))),
        ),
        DownloadTask::new(
            &format!("{}/file", server.uri()),
            root.path().join("pack.zip"),
            Some(BODY.len() as u64 + 1),
            None,
        )
        .optional(),
    ];

    Downloader::new(reqwest::Client::new())
        .fetch_all(&tasks, root.path(), ignore())
        .await
        .unwrap();

    assert_eq!(std::fs::read(wanted).unwrap(), BODY);
}
