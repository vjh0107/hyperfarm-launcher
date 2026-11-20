use std::path::{Path, PathBuf};

use md5::Md5;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio::io::AsyncReadExt;

use super::download::{DownloadTask, io_error};
use crate::error::LauncherError;

const STAGING: &str = "hyperfarm-server-pack.zip";
const RECORD: &str = "hyperfarm-server-pack.json";

// 서버가 접속 때 보내는 리소스팩을 클라이언트의 내려받기 캐시에 미리 둔다. 클라이언트는 downloads/<팩 UUID>/<SHA-1> 에
// 해시가 맞는 파일이 있으면 받지 않고, CraftEngine 은 팩 UUID 를 SHA-1 16진 문자열의 UUID.nameUUIDFromBytes 로 정한다
pub struct ServerPack {
    url: String,
    downloads: PathBuf,
}

pub struct PendingServerPack {
    pub task: DownloadTask,
    version: String,
}

#[derive(Serialize, Deserialize)]
struct Record {
    url: String,
    version: String,
    sha1: String,
}

impl ServerPack {
    pub fn new(url: &str, game_directory: &Path) -> Self {
        Self {
            url: url.into(),
            downloads: game_directory.join("downloads"),
        }
    }

    pub async fn prepare(&self, client: &reqwest::Client) -> Option<PendingServerPack> {
        let probe = with_query(&self.url, &format!("{:x}", rand::random::<u64>()));
        let response = match client.head(probe).send().await {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                log::warn!("The server resource pack answered {}", response.status());
                return None;
            }
            Err(error) => {
                log::warn!("Could not reach the server resource pack: {error}");
                return None;
            }
        };
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        // HEAD 응답은 본문이 없어 content_length() 가 0 을 줄 때가 있다
        let size = header(reqwest::header::CONTENT_LENGTH).parse::<u64>().ok();
        let version = format!(
            "{}|{}|{}",
            header(reqwest::header::ETAG),
            header(reqwest::header::LAST_MODIFIED),
            size.unwrap_or_default()
        );
        if self.current(&version).await {
            return None;
        }
        let staging = self.downloads.join(STAGING);
        let _ = tokio::fs::remove_file(&staging).await;
        // 게임서버는 팩을 한 경로에 덮어쓰고 플레이어에게는 서명 쿼리가 붙은 주소를 준다. 쿼리 없는 주소는 Cloudflare 가
        // 엣지에 담아 두어 덮어쓴 뒤에도 옛 팩을 내주므로(2026-11-20 운영), 판마다 다른 쿼리를 붙여 받는다
        let fetch = with_query(
            &self.url,
            &hex::encode(&Sha1::digest(version.as_bytes())[..8]),
        );
        Some(PendingServerPack {
            task: DownloadTask::new(&fetch, staging, size, None).optional(),
            version,
        })
    }

    pub async fn finish(&self, pending: PendingServerPack) -> Result<(), LauncherError> {
        if !tokio::fs::try_exists(&pending.task.target)
            .await
            .unwrap_or(false)
        {
            return Ok(());
        }
        let sha1 = file_sha1(&pending.task.target).await.map_err(io_error)?;
        let target = cached_path(&self.downloads, &sha1);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
        }
        tokio::fs::rename(&pending.task.target, &target)
            .await
            .map_err(io_error)?;
        let record = Record {
            url: self.url.clone(),
            version: pending.version,
            sha1,
        };
        let json = serde_json::to_vec(&record).map_err(|error| io_error(error.into()))?;
        tokio::fs::write(self.downloads.join(RECORD), json)
            .await
            .map_err(io_error)
    }

    async fn current(&self, version: &str) -> bool {
        let Ok(bytes) = tokio::fs::read(self.downloads.join(RECORD)).await else {
            return false;
        };
        let Ok(record) = serde_json::from_slice::<Record>(&bytes) else {
            return false;
        };
        record.url == self.url
            && record.version == version
            && tokio::fs::try_exists(cached_path(&self.downloads, &record.sha1))
                .await
                .unwrap_or(false)
    }
}

fn with_query(url: &str, version: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}v={version}")
}

fn cached_path(downloads: &Path, sha1: &str) -> PathBuf {
    downloads.join(pack_uuid(sha1)).join(sha1)
}

fn pack_uuid(sha1: &str) -> String {
    let mut bytes: [u8; 16] = Md5::digest(sha1.as_bytes()).into();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

async fn file_sha1(path: &Path) -> std::io::Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const PACK: &[u8] = b"hyperfarm pack";
    const PACK_SHA1: &str = "1f6faacd7258e2ae66e83548508f253dc727c3b8";
    const PACK_DIRECTORY: &str = "72d639c0-26a5-33d1-8d38-6185d72b0087";

    async fn serve(etag: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200).insert_header("ETag", etag))
            .mount(&server)
            .await;
        server
    }

    async fn download(pending: &PendingServerPack) {
        tokio::fs::create_dir_all(pending.task.target.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&pending.task.target, PACK).await.unwrap();
    }

    #[tokio::test]
    async fn fetches_past_an_edge_that_still_holds_the_old_pack() {
        use super::super::download::Downloader;
        use wiremock::matchers::query_param_is_missing;

        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"new\"")
                    .set_body_bytes(PACK),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param_is_missing("v"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(b"old hyperfarm pack, larger".as_slice()),
            )
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(PACK))
            .mount(&server)
            .await;
        let game = tempfile::tempdir().unwrap();
        let pack = ServerPack::new(&format!("{}/pack.zip", server.uri()), game.path());

        let pending = pack.prepare(&reqwest::Client::new()).await.unwrap();
        Downloader::new(reqwest::Client::new())
            .fetch_all(
                std::slice::from_ref(&pending.task),
                game.path(),
                std::sync::Arc::new(|_, _, _| {}),
            )
            .await
            .unwrap();
        pack.finish(pending).await.unwrap();

        let placed = game
            .path()
            .join("downloads")
            .join(PACK_DIRECTORY)
            .join(PACK_SHA1);
        assert_eq!(std::fs::read(placed).unwrap(), PACK);
    }

    #[tokio::test]
    async fn places_the_pack_where_the_client_looks_for_the_server_pack() {
        let server = serve("\"a\"").await;
        let game = tempfile::tempdir().unwrap();
        let pack = ServerPack::new(&format!("{}/pack.zip", server.uri()), game.path());

        let pending = pack.prepare(&reqwest::Client::new()).await.unwrap();
        download(&pending).await;
        pack.finish(pending).await.unwrap();

        let placed = game
            .path()
            .join("downloads")
            .join(PACK_DIRECTORY)
            .join(PACK_SHA1);
        assert_eq!(std::fs::read(placed).unwrap(), PACK);
    }

    #[tokio::test]
    async fn skips_the_download_until_the_pack_changes() {
        let game = tempfile::tempdir().unwrap();
        let client = reqwest::Client::new();
        let first = serve("\"a\"").await;
        let url = format!("{}/pack.zip", first.uri());
        let pack = ServerPack::new(&url, game.path());
        let pending = pack.prepare(&client).await.unwrap();
        download(&pending).await;
        pack.finish(pending).await.unwrap();

        assert!(pack.prepare(&client).await.is_none());

        first.reset().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200).insert_header("ETag", "\"b\""))
            .mount(&first)
            .await;
        assert!(pack.prepare(&client).await.is_some());
    }

    #[tokio::test]
    async fn downloads_again_when_the_client_removed_the_cached_pack() {
        let server = serve("\"a\"").await;
        let game = tempfile::tempdir().unwrap();
        let client = reqwest::Client::new();
        let pack = ServerPack::new(&format!("{}/pack.zip", server.uri()), game.path());
        let pending = pack.prepare(&client).await.unwrap();
        download(&pending).await;
        pack.finish(pending).await.unwrap();

        std::fs::remove_dir_all(game.path().join("downloads").join(PACK_DIRECTORY)).unwrap();

        assert!(pack.prepare(&client).await.is_some());
    }

    #[tokio::test]
    async fn leaves_the_pack_to_the_server_when_it_is_not_published() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let game = tempfile::tempdir().unwrap();
        let pack = ServerPack::new(&format!("{}/pack.zip", server.uri()), game.path());

        assert!(pack.prepare(&reqwest::Client::new()).await.is_none());
    }
}
