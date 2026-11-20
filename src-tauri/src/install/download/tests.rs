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

#[test]
fn skips_optional_files_that_do_not_fit_instead_of_refusing_to_install() {
    let required = DownloadTask::new("https://a", PathBuf::from("a"), Some(100), None);
    let pack = DownloadTask::new("https://b", PathBuf::from("b"), Some(500), None).optional();
    let tasks = [required, pack];
    let urls = |usable| -> Vec<String> {
        fitting(&tasks, usable)
            .unwrap()
            .into_iter()
            .map(|task| task.url.clone())
            .collect()
    };

    assert_eq!(urls(100 + FREE_SPACE_MARGIN + 499), ["https://a"]);
    assert_eq!(
        urls(100 + FREE_SPACE_MARGIN + 500),
        ["https://a", "https://b"]
    );
    let error = fitting(&tasks, 99 + FREE_SPACE_MARGIN).unwrap_err();
    assert_eq!(error.problem, Problem::DiskFull);
}

#[cfg(windows)]
#[test]
fn finds_the_drive_of_a_canonical_windows_path() {
    let disks = [(Path::new(r"C:\"), 7), (Path::new(r"D:\"), 9)];

    let space = available_on(
        Path::new(r"\\?\C:\Users\player\AppData\Roaming\HyperFarm"),
        disks.into_iter(),
    );

    assert_eq!(space, 7);
}

#[cfg(unix)]
#[tokio::test]
async fn waits_for_a_target_that_is_locked_for_a_moment() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let from = root.path().join("a.jar.part");
    std::fs::write(&from, BODY).unwrap();
    let locked = root.path().join("mods");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let unlock = {
        let locked = locked.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        })
    };

    move_into_place(&from, &locked.join("a.jar")).await.unwrap();
    unlock.await.unwrap();

    assert_eq!(std::fs::read(locked.join("a.jar")).unwrap(), BODY);
}

#[cfg(unix)]
#[tokio::test]
async fn waits_to_remove_a_file_that_is_held_for_a_moment() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("mods");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("old.jar"), BODY).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let unlock = {
        let locked = locked.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        })
    };

    remove_when_free(&locked.join("old.jar")).await.unwrap();
    unlock.await.unwrap();

    assert!(!locked.join("old.jar").exists());
}

#[test]
fn blames_another_program_only_for_a_windows_file_that_stayed_held() {
    let jar = Path::new("mods/hyperfarm.jar");
    let denied = || std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let held = if cfg!(windows) { HELD } else { WRITE_FAILED };

    assert_eq!(held_error(jar)(denied()).message, held);
    assert_eq!(
        held_error(jar)(std::io::Error::from(std::io::ErrorKind::NotFound)).message,
        WRITE_FAILED
    );
    assert_eq!(io_error(jar)(denied()).message, WRITE_FAILED);
}

#[test]
fn treats_a_windows_sharing_violation_as_held() {
    let sharing = std::io::Error::from_raw_os_error(32);
    let lock = std::io::Error::from_raw_os_error(33);

    assert_eq!(is_held(&sharing), cfg!(windows));
    assert_eq!(is_held(&lock), cfg!(windows));
}
