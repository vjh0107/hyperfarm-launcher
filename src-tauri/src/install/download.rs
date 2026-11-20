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
// 백신은 막 받은 파일을 검사하는 동안 그 파일을 붙잡는다. 40MB 가 넘는 하이퍼팜 모드 jar 는 그 검사가 2초를 넘겨, 예전의
// 다섯 번(모두 2초)으로는 모자랐다. 열두 번이면 모두 13초 남짓 기다린다
const HELD_ATTEMPTS: u32 = 12;
const HELD_WAIT_LIMIT: Duration = Duration::from_secs(2);

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

    pub fn expected(&self) -> &str {
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
    pub optional: bool,
}

impl DownloadTask {
    pub fn new(url: &str, target: PathBuf, size: Option<u64>, hash: Option<Hash>) -> Self {
        Self {
            url: url.into(),
            target,
            size,
            hash,
            content_addressed: false,
            optional: false,
        }
    }

    // 받지 못해도 게임은 켤 수 있는 파일이다. 실패하면 남기기만 하고 나머지 설치를 멈추지 않는다
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    // 에셋은 경로가 곧 해시라 크기만 맞으면 같은 파일이다. 매번 3천 개를 다시 해시하면 실행이 몇 초 늦어진다
    pub fn asset(url: &str, target: PathBuf, size: u64, hash: Hash) -> Self {
        Self {
            url: url.into(),
            target,
            size: Some(size),
            hash: Some(hash),
            content_addressed: true,
            optional: false,
        }
    }

    pub async fn is_satisfied(&self) -> bool {
        let Ok(metadata) = tokio::fs::metadata(&self.target).await else {
            return false;
        };
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
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };
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

    pub async fn fetch_all(
        &self,
        missing: &[DownloadTask],
        root: &Path,
        progress: Progress,
    ) -> Result<(), LauncherError> {
        tokio::fs::create_dir_all(root)
            .await
            .map_err(io_error(root))?;
        let tasks = fitting(missing, usable_space(root))?;
        let total: u64 = tasks.iter().map(|task| task.expected_bytes()).sum();
        let done = AtomicU64::new(0);
        stream::iter(tasks.into_iter().map(Ok::<_, LauncherError>))
            .try_for_each_concurrent(PARALLEL_DOWNLOADS, |task| {
                let done = &done;
                let progress = &progress;
                async move {
                    match fetch(&self.client, task, done, total, progress).await {
                        Err(error) if task.optional => {
                            log::warn!("Skipped {}: {}", task.url, error.message);
                            Ok(())
                        }
                        result => result,
                    }
                }
            })
            .await
    }
}

async fn fetch(
    client: &reqwest::Client,
    task: &DownloadTask,
    done: &AtomicU64,
    total: u64,
    progress: &Progress,
) -> Result<(), LauncherError> {
    let parent = task
        .target
        .parent()
        .expect("download targets always live in a directory");
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(io_error(parent))?;
    let file_name = task
        .target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let partial = task.target.with_file_name(format!("{file_name}.part"));
    let mut attempt = 1;
    loop {
        let mut received = 0u64;
        let result = transfer(
            client,
            task,
            &partial,
            &mut received,
            done,
            total,
            progress,
            &file_name,
        )
        .await;
        match result {
            Ok(()) => {
                move_into_place(&partial, &task.target)
                    .await
                    .map_err(io_error(&task.target))?;
                return Ok(());
            }
            Err(error) => {
                done.fetch_sub(received, Ordering::Relaxed);
                let _ = tokio::fs::remove_file(&partial).await;
                if attempt == ATTEMPTS {
                    log::warn!("Giving up on {}: {error}", task.url);
                    return Err(match error {
                        TransferError::Corrupt(_) => LauncherError::new(
                            Problem::CorruptDownload,
                            format!("받은 파일이 손상됐어요. ({file_name})"),
                        ),
                        TransferError::Io(error) => io_error(&partial)(error),
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
    let response = client
        .get(&task.url)
        .send()
        .await
        .map_err(|e| TransferError::Network(e.to_string()))?;
    if !response.status().is_success() {
        return Err(TransferError::Network(format!(
            "{} answered {}",
            task.url,
            response.status()
        )));
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
        return Err(TransferError::Corrupt(format!(
            "{} sent {} bytes, expected {:?}",
            task.url, received, task.size
        )));
    }
    if let (Some(hash), Some(hasher)) = (&task.hash, hasher)
        && hasher.finish() != hash.expected()
    {
        return Err(TransferError::Corrupt(format!(
            "{} does not match its hash",
            task.url
        )));
    }
    Ok(())
}

// 받지 못해도 되는 파일은 필수 파일을 받고 남는 공간에 들어갈 때만 받는다
fn fitting(missing: &[DownloadTask], usable: u64) -> Result<Vec<&DownloadTask>, LauncherError> {
    let needed: u64 = missing
        .iter()
        .filter(|task| !task.optional)
        .map(DownloadTask::expected_bytes)
        .sum::<u64>()
        + FREE_SPACE_MARGIN;
    if usable < needed {
        return Err(LauncherError::disk_full(needed, usable));
    }
    let mut spare = usable - needed;
    Ok(missing
        .iter()
        .filter(|task| {
            if !task.optional {
                return true;
            }
            let size = task.expected_bytes();
            if size > spare {
                log::warn!("Skipped {}: not enough disk space", task.url);
                return false;
            }
            spare -= size;
            true
        })
        .collect())
}

pub(crate) async fn move_into_place(from: &Path, to: &Path) -> std::io::Result<()> {
    while_held(|| tokio::fs::rename(from, to)).await
}

pub(crate) async fn remove_when_free(path: &Path) -> std::io::Result<()> {
    while_held(|| tokio::fs::remove_file(path)).await
}

async fn while_held<F, Operation>(mut operation: F) -> std::io::Result<()>
where
    F: FnMut() -> Operation,
    Operation: Future<Output = std::io::Result<()>>,
{
    let mut attempt = 1;
    loop {
        match operation().await {
            Err(error) if is_held(&error) && attempt < HELD_ATTEMPTS => {
                let wait = Duration::from_millis(200 * u64::from(attempt));
                tokio::time::sleep(wait.min(HELD_WAIT_LIMIT)).await;
                attempt += 1;
            }
            result => return result,
        }
    }
}

// Windows 는 다른 프로그램이 붙잡은 파일을 바꾸거나 지우는 것을 PermissionDenied(5)나 공유 위반(32·33)으로 거절한다.
// 32·33 은 다른 운영체제에서는 다른 뜻이다
pub(crate) fn is_held(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
        || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)))
}

fn usable_space(path: &Path) -> u64 {
    let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() else {
        return u64::MAX;
    };
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    available_on(
        &canonical,
        disks
            .iter()
            .map(|disk| (disk.mount_point(), disk.available_space())),
    )
}

// Windows 의 canonicalize 는 \\?\C:\ 를 돌려주고 sysinfo 의 마운트 지점은 C:\ 라, 떼지 않으면 어느 디스크와도 맞지
// 않아 검사가 꺼진다
fn available_on<'a>(path: &Path, disks: impl Iterator<Item = (&'a Path, u64)>) -> u64 {
    let path = dunce::simplified(path);
    disks
        .filter(|(mount, _)| path.starts_with(mount))
        .max_by_key(|(mount, _)| mount.as_os_str().len())
        .map(|(_, space)| space)
        .unwrap_or(u64::MAX)
}

// 같은 문구가 여러 자리에서 나오므로 로그에는 어느 파일이었는지를 함께 남긴다
pub fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> LauncherError + '_ {
    move |error| {
        log::warn!("File system error at {}: {error}", path.display());
        let message = if is_held(&error) {
            "백신 같은 다른 프로그램이 게임 파일을 막고 있어요. 잠시 뒤 다시 시도해 주세요."
        } else {
            "게임 폴더에 파일을 쓰지 못했어요."
        };
        LauncherError::new(Problem::CorruptDownload, message)
    }
}

#[cfg(test)]
mod tests;
