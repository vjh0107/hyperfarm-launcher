use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt, stream};
use sha1::Sha1;
use sha2::{Digest, Sha512};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{LauncherError, Problem};

const PARALLEL_DOWNLOADS: usize = 16;
const ATTEMPTS: u32 = 3;
const FREE_SPACE_MARGIN: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hash {
    Sha1(String),
    Sha512(String),
}

impl Hash {
    pub fn sha1(hex: &str) -> Self {
        Hash::Sha1(hex.to_ascii_lowercase())
    }

    pub fn sha512(hex: &str) -> Self {
        Hash::Sha512(hex.to_ascii_lowercase())
    }

    fn digest(&self) -> Hasher {
        match self {
            Hash::Sha1(_) => Hasher::Sha1(Sha1::new()),
            Hash::Sha512(_) => Hasher::Sha512(Sha512::new()),
        }
    }

    fn expected(&self) -> &str {
        match self {
            Hash::Sha1(hex) | Hash::Sha512(hex) => hex,
        }
    }
}

enum Hasher {
    Sha1(Sha1),
    Sha512(Sha512),
}

impl Hasher {
    fn update(&mut self, bytes: &[u8]) {
        match self {
            Hasher::Sha1(hasher) => hasher.update(bytes),
            Hasher::Sha512(hasher) => hasher.update(bytes),
        }
    }

    fn finish(self) -> String {
        match self {
            Hasher::Sha1(hasher) => hex::encode(hasher.finalize()),
            Hasher::Sha512(hasher) => hex::encode(hasher.finalize()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DownloadTask {
    pub url: String,
    pub target: PathBuf,
    pub size: Option<u64>,
    pub hash: Option<Hash>,
    pub content_addressed: bool,
}

impl DownloadTask {
    pub fn new(url: &str, target: PathBuf, size: Option<u64>, hash: Option<Hash>) -> Self {
        Self { url: url.into(), target, size, hash, content_addressed: false }
    }

    // 에셋은 경로가 곧 해시라 크기만 맞으면 같은 파일이다. 매번 3천 개를 다시 해시하면 실행이 몇 초 늦어진다
    pub fn asset(url: &str, target: PathBuf, size: u64, hash: Hash) -> Self {
        Self { url: url.into(), target, size: Some(size), hash: Some(hash), content_addressed: true }
    }

    pub async fn is_satisfied(&self) -> bool {
        let Ok(metadata) = tokio::fs::metadata(&self.target).await else { return false };
        if !metadata.is_file() || self.size.is_some_and(|size| metadata.len() != size) {
            return false;
        }
        if self.content_addressed {
            return true;
        }
        match &self.hash {
            None => true,
            Some(hash) => file_matches(&self.target, hash).await,
        }
    }

    fn expected_bytes(&self) -> u64 {
        self.size.unwrap_or(0)
    }
}

async fn file_matches(path: &Path, hash: &Hash) -> bool {
    let Ok(mut file) = tokio::fs::File::open(path).await else { return false };
    let mut hasher = hash.digest();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(_) => return false,
        }
    }
    hasher.finish() == hash.expected()
}

pub type Progress = Arc<dyn Fn(u64, u64, &str) + Send + Sync>;

#[derive(Clone)]
pub struct Downloader {
    client: reqwest::Client,
}

impl Downloader {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub async fn missing(&self, tasks: Vec<DownloadTask>) -> Vec<DownloadTask> {
        stream::iter(tasks)
            .map(|task| async move { (!task.is_satisfied().await).then_some(task) })
            .buffered(PARALLEL_DOWNLOADS)
            .filter_map(|task| async move { task })
            .collect()
            .await
    }

    pub async fn fetch_one(&self, task: &DownloadTask) -> Result<(), LauncherError> {
        if task.is_satisfied().await {
            return Ok(());
        }
        let ignore: Progress = Arc::new(|_, _, _| {});
        fetch(&self.client, task, &AtomicU64::new(0), 0, &ignore).await
    }

    pub async fn fetch_all(&self, missing: &[DownloadTask], root: &Path, progress: Progress) -> Result<(), LauncherError> {
        let total: u64 = missing.iter().map(DownloadTask::expected_bytes).sum();
        tokio::fs::create_dir_all(root).await.map_err(io_error)?;
        let usable = usable_space(root);
        if usable < total + FREE_SPACE_MARGIN {
            return Err(LauncherError::disk_full(total + FREE_SPACE_MARGIN, usable));
        }
        let done = AtomicU64::new(0);
        stream::iter(missing.iter().map(Ok::<_, LauncherError>))
            .try_for_each_concurrent(PARALLEL_DOWNLOADS, |task| {
                let done = &done;
                let progress = &progress;
                async move { fetch(&self.client, task, done, total, progress).await }
            })
            .await
    }
}

async fn fetch(client: &reqwest::Client, task: &DownloadTask, done: &AtomicU64, total: u64, progress: &Progress) -> Result<(), LauncherError> {
    let parent = task.target.parent().expect("download targets always live in a directory");
    tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
    let file_name = task.target.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string();
    let partial = task.target.with_file_name(format!("{file_name}.part"));
    let mut attempt = 1;
    loop {
        let mut received = 0u64;
        let result = transfer(client, task, &partial, &mut received, done, total, progress, &file_name).await;
        match result {
            Ok(()) => {
                tokio::fs::rename(&partial, &task.target).await.map_err(io_error)?;
                return Ok(());
            }
            Err(error) => {
                done.fetch_sub(received, Ordering::Relaxed);
                let _ = tokio::fs::remove_file(&partial).await;
                if attempt == ATTEMPTS {
                    log::warn!("Giving up on {}: {error}", task.url);
                    return Err(match error {
                        TransferError::Corrupt(_) => {
                            LauncherError::new(Problem::CorruptDownload, format!("받은 파일이 손상됐어요. ({file_name})"))
                        }
                        TransferError::Io(_) => io_error(std::io::Error::other("write failed")),
                        TransferError::Network(_) => LauncherError::network(),
                    });
                }
                tokio::time::sleep(Duration::from_secs(attempt.into())).await;
                attempt += 1;
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum TransferError {
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[allow(clippy::too_many_arguments)]
async fn transfer(
    client: &reqwest::Client,
    task: &DownloadTask,
    partial: &Path,
    received: &mut u64,
    done: &AtomicU64,
    total: u64,
    progress: &Progress,
    file_name: &str,
) -> Result<(), TransferError> {
    let response = client.get(&task.url).send().await.map_err(|e| TransferError::Network(e.to_string()))?;
    if !response.status().is_success() {
        return Err(TransferError::Network(format!("{} answered {}", task.url, response.status())));
    }
    let mut hasher = task.hash.as_ref().map(Hash::digest);
    let mut file = tokio::fs::File::create(partial).await?;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| TransferError::Network(e.to_string()))?;
        file.write_all(&chunk).await?;
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&chunk);
        }
        *received += chunk.len() as u64;
        let now = done.fetch_add(chunk.len() as u64, Ordering::Relaxed) + chunk.len() as u64;
        progress(now, total, file_name);
    }
    file.flush().await?;
    if task.size.is_some_and(|size| *received != size) {
        return Err(TransferError::Corrupt(format!("{} sent {} bytes, expected {:?}", task.url, received, task.size)));
    }
    if let (Some(hash), Some(hasher)) = (&task.hash, hasher) {
        if hasher.finish() != hash.expected() {
            return Err(TransferError::Corrupt(format!("{} does not match its hash", task.url)));
        }
    }
    Ok(())
}

fn usable_space(path: &Path) -> u64 {
    let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() else { return u64::MAX };
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    disks
        .iter()
        .filter(|disk| canonical.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
        .unwrap_or(u64::MAX)
}

pub fn io_error(error: std::io::Error) -> LauncherError {
    log::warn!("File system error: {error}");
    LauncherError::new(Problem::CorruptDownload, "게임 폴더에 파일을 쓰지 못했어요.")
}

#[cfg(test)]
mod tests;
