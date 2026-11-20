pub mod download;
pub mod library;
pub mod metadata;
pub mod options;
pub mod pack;
pub mod rules;
mod server_pack;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{LauncherError, Problem};
use crate::http::Http;
use download::{
    DownloadTask, Downloader, Hash, Progress, io_error, move_into_place, remove_when_free,
};
use metadata::{
    AssetIndex, AssetIndexRef, FabricProfile, RuntimeIndex, RuntimeManifest, VersionInfo,
    VersionManifest,
};
use pack::{PackFile, PackManifest};
use rules::{Argument, Platform, parse_arguments};
use server_pack::ServerPack;

const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const JAVA_RUNTIMES: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions/loader";
const ASSET_OBJECTS: &str = "https://resources.download.minecraft.net";
const HOSTED: &str = "https://pack.hyperf.kr/";
const INSTALLED_MODS: &str = ".hyperfarm-installed.json";

#[derive(Clone, Debug)]
pub struct GameLayout {
    pub root: PathBuf,
}

impl GameLayout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn version(&self, id: &str) -> PathBuf {
        self.root.join("versions").join(id)
    }

    fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    fn runtime(&self, component: &str) -> PathBuf {
        self.root.join("runtime").join(component)
    }

    pub fn mods(&self) -> PathBuf {
        self.root.join("mods")
    }

    fn shaderpacks(&self) -> PathBuf {
        self.root.join("shaderpacks")
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn options(&self) -> PathBuf {
        self.root.join("options.txt")
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPlan {
    pub total_bytes: u64,
    pub update: bool,
    pub changed_mods: usize,
}

pub trait InstallListener: Send + Sync {
    fn downloading(&self, plan: DownloadPlan);
    fn progress(&self) -> Progress;
}

#[derive(Clone, Debug)]
pub struct Installation {
    pub java: PathBuf,
    pub classpath: Vec<PathBuf>,
    pub main_class: String,
    pub jvm_arguments: Vec<Argument>,
    pub game_arguments: Vec<Argument>,
    pub version_name: String,
    pub version_type: String,
    pub asset_index: String,
    pub assets_root: PathBuf,
    pub libraries_root: PathBuf,
    pub natives: PathBuf,
    pub game_directory: PathBuf,
    pub server: String,
}

pub struct GameInstaller {
    http: Http,
    downloader: Downloader,
    layout: GameLayout,
    platform: Platform,
    owns_folder: bool,
}

impl GameInstaller {
    pub fn new(http: Http, layout: GameLayout, platform: Platform) -> Self {
        let downloader = Downloader::new(http.client().clone());
        Self {
            http,
            downloader,
            layout,
            platform,
            owns_folder: false,
        }
    }

    pub fn owning_folder(mut self, owns: bool) -> Self {
        self.owns_folder = owns;
        self
    }

    pub async fn install(
        &self,
        pack: &PackManifest,
        listener: &dyn InstallListener,
    ) -> Result<Installation, LauncherError> {
        let update = self.layout.version(&pack.minecraft_version).is_dir();
        let version = self.version(&pack.minecraft_version).await?;
        let fabric_id = format!(
            "fabric-loader-{}-{}",
            pack.fabric_loader_version, pack.minecraft_version
        );
        let fabric = fabric_profile(
            &self.http,
            &format!(
                "{FABRIC_META}/{}/{}/profile/json",
                pack.minecraft_version, pack.fabric_loader_version
            ),
            &self
                .layout
                .version(&fabric_id)
                .join(format!("{fabric_id}.json")),
        )
        .await?;
        let assets = self.asset_index(&version.asset_index).await?;
        let component = version.java_version.component.clone();
        let runtime = self.runtime(&component).await?;

        let mut libraries = library::resolve(
            &fabric.libraries,
            &version.libraries,
            &self.platform,
            &self.layout.libraries(),
        )?;
        library::fill_missing_hashes(&self.http, &mut libraries).await;
        let client_jar = self
            .layout
            .version(&version.id)
            .join(format!("{}.jar", version.id));
        let client = &version.downloads.client;
        let mut tasks: Vec<DownloadTask> = libraries.clone();
        tasks.push(DownloadTask::new(
            &client.url,
            client_jar.clone(),
            Some(client.size),
            Some(Hash::sha1(&client.sha1)),
        ));
        tasks.extend(self.asset_tasks(&assets));
        tasks.extend(self.runtime_tasks(&runtime, &component));
        tasks.extend(pack_tasks(&pack.mods, &self.layout.mods())?);
        tasks.extend(pack_tasks(&pack.shaderpacks, &self.layout.shaderpacks())?);
        let logging = version
            .logging
            .as_ref()
            .and_then(|logging| logging.client.as_ref());
        let logging_config = logging.map(|entry| {
            self.layout
                .assets()
                .join("log_configs")
                .join(&entry.file.id)
        });
        if let (Some(entry), Some(path)) = (logging, &logging_config) {
            tasks.push(DownloadTask::new(
                &entry.file.url,
                path.clone(),
                Some(entry.file.size),
                Some(Hash::sha1(&entry.file.sha1)),
            ));
        }
        dedupe(&mut tasks);
        let server_pack = pack
            .server_resource_pack_url
            .as_deref()
            .map(|url| ServerPack::new(url, &self.layout.root));
        let pending_pack = match &server_pack {
            Some(server_pack) => server_pack.prepare(self.http.client()).await,
            None => None,
        };
        if let Some(pending) = &pending_pack {
            tasks.push(pending.task.clone());
        }

        let missing = self.downloader.missing(tasks).await;
        if !missing.is_empty() {
            let total_bytes = missing.iter().map(|task| task.size.unwrap_or(0)).sum();
            let changed_mods = missing
                .iter()
                .filter(|task| task.target.parent() == Some(self.layout.mods().as_path()))
                .count();
            listener.downloading(DownloadPlan {
                total_bytes,
                update,
                changed_mods,
            });
            self.downloader
                .fetch_all(&missing, &self.layout.root, listener.progress())
                .await?;
        }
        // 못 두어도 접속하면 서버가 보내 주므로 게임은 그대로 켠다
        if let (Some(server_pack), Some(pending)) = (&server_pack, pending_pack)
            && let Err(error) = server_pack.finish(pending).await
        {
            log::warn!("Could not keep the server resource pack: {}", error.message);
        }
        self.finish_runtime(&runtime, &component).await?;
        prune_mods(&pack.mods, &self.layout.mods(), self.owns_folder).await?;

        let java = self
            .layout
            .runtime(&component)
            .join(self.platform.java_executable());
        if !java.is_file() {
            return Err(LauncherError::new(
                Problem::CorruptDownload,
                "게임용 Java 를 설치하지 못했어요.",
            ));
        }
        let natives = self.layout.version(&version.id).join("natives");
        for sub in ["java", "jna", "lwjgl", "netty"] {
            let directory = natives.join(sub);
            tokio::fs::create_dir_all(&directory)
                .await
                .map_err(io_error(&directory))?;
        }

        let mut classpath: Vec<PathBuf> = libraries.into_iter().map(|task| task.target).collect();
        classpath.push(client_jar);
        let mut jvm_arguments: Vec<Argument> =
            parse_arguments(version.arguments.default_user_jvm.as_ref())
                .into_iter()
                .map(without_heap_size)
                .collect();
        jvm_arguments.extend(parse_arguments(version.arguments.jvm.as_ref()));
        if let (Some(entry), Some(path)) = (logging, logging_config) {
            jvm_arguments.push(Argument {
                rules: vec![],
                values: vec![entry.argument.replace("${path}", &path.to_string_lossy())],
            });
        }
        let mut game_arguments = parse_arguments(version.arguments.game.as_ref());
        if let Some(arguments) = &fabric.arguments {
            jvm_arguments.extend(parse_arguments(arguments.jvm.as_ref()));
            game_arguments.extend(parse_arguments(arguments.game.as_ref()));
        }
        Ok(Installation {
            java,
            classpath,
            main_class: fabric.main_class,
            jvm_arguments,
            game_arguments,
            version_name: fabric.id,
            version_type: version.kind,
            asset_index: version.asset_index.id,
            assets_root: self.layout.assets(),
            libraries_root: self.layout.libraries(),
            natives,
            game_directory: self.layout.root.clone(),
            server: pack.server.clone(),
        })
    }

    async fn version(&self, id: &str) -> Result<VersionInfo, LauncherError> {
        let file = self.layout.version(id).join(format!("{id}.json"));
        or_saved(&file, async {
            let manifest: VersionManifest = self.http.get_json(VERSION_MANIFEST).await?;
            let entry = manifest
                .versions
                .into_iter()
                .find(|entry| entry.id == id)
                .ok_or_else(|| {
                    LauncherError::new(
                        Problem::PackInvalid,
                        format!("마인크래프트 {id} 을 찾지 못했어요."),
                    )
                })?;
            self.downloader
                .fetch_one(&DownloadTask::new(
                    &entry.url,
                    file.clone(),
                    None,
                    Some(Hash::sha1(&entry.sha1)),
                ))
                .await?;
            read_json(&file).await
        })
        .await
    }

    async fn asset_index(&self, reference: &AssetIndexRef) -> Result<AssetIndex, LauncherError> {
        let file = self
            .layout
            .assets()
            .join("indexes")
            .join(format!("{}.json", reference.id));
        self.downloader
            .fetch_one(&DownloadTask::new(
                &reference.url,
                file.clone(),
                Some(reference.size),
                Some(Hash::sha1(&reference.sha1)),
            ))
            .await?;
        read_json(&file).await
    }

    async fn runtime(&self, component: &str) -> Result<RuntimeManifest, LauncherError> {
        let unsupported = || {
            LauncherError::new(
                Problem::UnsupportedPlatform,
                format!("이 운영체제용 게임 Java({component})가 없어요."),
            )
        };
        let key = self.platform.java_runtime_key().ok_or_else(unsupported)?;
        let file = self
            .layout
            .root
            .join("runtime")
            .join(format!("{component}.json"));
        or_saved(&file, async {
            let index: RuntimeIndex = self.http.get_json(JAVA_RUNTIMES).await?;
            let variant = index
                .get(key)
                .and_then(|components| components.get(component))
                .and_then(|v| v.first())
                .ok_or_else(unsupported)?;
            let manifest = &variant.manifest;
            self.downloader
                .fetch_one(&DownloadTask::new(
                    &manifest.url,
                    file.clone(),
                    Some(manifest.size),
                    Some(Hash::sha1(&manifest.sha1)),
                ))
                .await?;
            read_json(&file).await
        })
        .await
    }

    fn asset_tasks(&self, assets: &AssetIndex) -> Vec<DownloadTask> {
        let objects = self.layout.assets().join("objects");
        assets
            .objects
            .values()
            .map(|object| {
                let prefix = &object.hash[..2];
                DownloadTask::asset(
                    &format!("{ASSET_OBJECTS}/{prefix}/{}", object.hash),
                    objects.join(prefix).join(&object.hash),
                    object.size,
                    Hash::sha1(&object.hash),
                )
            })
            .collect()
    }

    fn runtime_tasks(&self, runtime: &RuntimeManifest, component: &str) -> Vec<DownloadTask> {
        let root = self.layout.runtime(component);
        runtime
            .files
            .iter()
            .filter(|(_, file)| file.kind == "file")
            .filter_map(|(path, file)| {
                let raw = &file.downloads.as_ref()?.raw;
                Some(DownloadTask::new(
                    &raw.url,
                    root.join(path),
                    Some(raw.size),
                    Some(Hash::sha1(&raw.sha1)),
                ))
            })
            .collect()
    }

    async fn finish_runtime(
        &self,
        runtime: &RuntimeManifest,
        component: &str,
    ) -> Result<(), LauncherError> {
        let root = self.layout.runtime(component);
        for (path, file) in &runtime.files {
            let target = root.join(path);
            match file.kind.as_str() {
                "directory" => tokio::fs::create_dir_all(&target)
                    .await
                    .map_err(io_error(&target))?,
                #[cfg(unix)]
                "link" => {
                    if let Some(link) = &file.target
                        && tokio::fs::symlink_metadata(&target).await.is_err()
                    {
                        if let Some(parent) = target.parent() {
                            tokio::fs::create_dir_all(parent)
                                .await
                                .map_err(io_error(parent))?;
                        }
                        tokio::fs::symlink(link, &target)
                            .await
                            .map_err(io_error(&target))?;
                    }
                }
                #[cfg(unix)]
                "file" if file.executable => {
                    use std::os::unix::fs::PermissionsExt;
                    let mut permissions = tokio::fs::metadata(&target)
                        .await
                        .map_err(io_error(&target))?
                        .permissions();
                    permissions.set_mode(permissions.mode() | 0o111);
                    tokio::fs::set_permissions(&target, permissions)
                        .await
                        .map_err(io_error(&target))?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn pack_tasks(files: &[PackFile], directory: &Path) -> Result<Vec<DownloadTask>, LauncherError> {
    files
        .iter()
        .map(|file| {
            let hash = Hash::sha512(&file.sha512);
            // Cloudflare 가 쿼리 없는 pack.hyperf.kr 주소를 엣지에 옛 판으로 담아 두어 해시가 맞지 않고 실행이 멈췄다
            // (2026-11-20). 판마다 다른 쿼리로 받는다
            let url = if file.url.starts_with(HOSTED) {
                let expected = hash.expected();
                with_query(&file.url, expected.get(..16).unwrap_or(expected))
            } else {
                file.url.clone()
            };
            Ok(DownloadTask::new(
                &url,
                directory.join(file.safe_file_name()?),
                Some(file.size),
                Some(hash),
            ))
        })
        .collect()
}

fn with_query(url: &str, version: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}v={version}")
}

async fn fabric_profile(
    http: &Http,
    url: &str,
    file: &Path,
) -> Result<FabricProfile, LauncherError> {
    or_saved(file, async {
        let text = http.get_text(url).await?;
        let profile = serde_json::from_str(&text).map_err(|error| {
            log::warn!("{url} sent an unreadable Fabric profile: {error}");
            LauncherError::network()
        })?;
        // 사본은 메타 서버가 멎었을 때만 쓰므로, 백신이 파일을 쥐어 남기지 못해도 받은 것으로 켠다
        if let Err(error) = save(file, &text).await {
            log::warn!("Could not keep {}: {error}", file.display());
        }
        Ok(profile)
    })
    .await
}

async fn save(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let partial = file.with_extension("json.part");
    tokio::fs::write(&partial, text).await?;
    move_into_place(&partial, file).await
}

// 메타 서버가 멎어도 지난번에 받아 둔 것으로 게임을 켠다
async fn or_saved<T: serde::de::DeserializeOwned>(
    file: &Path,
    fetch: impl Future<Output = Result<T, LauncherError>>,
) -> Result<T, LauncherError> {
    match fetch.await {
        Err(error)
            if error.problem == Problem::Network
                && tokio::fs::metadata(file)
                    .await
                    .is_ok_and(|metadata| metadata.is_file()) =>
        {
            log::warn!("Could not refresh {}; using the saved one", file.display());
            read_json(file).await
        }
        result => result,
    }
}

fn dedupe(tasks: &mut Vec<DownloadTask>) {
    let mut seen = std::collections::HashSet::new();
    tasks.retain(|task| seen.insert(task.target.clone()));
}

// 플레이어가 고른 폴더에는 남이 둔 모드가 있을 수 있다. 지우는 것은 launcher 가 깔았다고 적어 둔 jar 뿐이고,
// 적어 둔 것이 없는 첫 실행에서는 아무것도 지우지 않는다. 다만 기본 게임 폴더는 launcher 만 쓰고, 기록이 생기기 전
// 판이 깐 옛 jar 를 남기면 새 판과 겹쳐 Fabric 이 뜨지 않으므로 기록이 없을 때 그 폴더의 jar 를 전부 launcher 것으로 본다
pub async fn prune_mods(
    mods: &[PackFile],
    directory: &Path,
    owns_unrecorded: bool,
) -> Result<(), LauncherError> {
    let record = directory.join(INSTALLED_MODS);
    let installed = match read_installed(&record).await {
        Some(installed) => installed,
        None if owns_unrecorded => jars_in(directory).await,
        None => Vec::new(),
    };
    let wanted: std::collections::HashSet<&str> =
        mods.iter().map(|file| file.file_name.as_str()).collect();
    for name in installed
        .iter()
        .filter(|name| !wanted.contains(name.as_str()))
    {
        let path = directory.join(name);
        let is_jar = name.ends_with(".jar")
            && !name.contains(['/', '\\'])
            && tokio::fs::symlink_metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_file());
        if is_jar {
            remove_when_free(&path).await.map_err(io_error(&path))?;
        }
    }
    let names: Vec<&str> = mods.iter().map(|file| file.file_name.as_str()).collect();
    let json = serde_json::to_vec(&names).map_err(|error| io_error(&record)(error.into()))?;
    // 기록을 남기지 못하면 다음 실행이 지난 기록으로 지울 것을 고를 뿐이라 게임은 켠다
    if let Err(error) = write_record(directory, &record, &json).await {
        log::warn!("Could not keep {}: {error}", record.display());
    }
    Ok(())
}

async fn write_record(directory: &Path, record: &Path, json: &[u8]) -> std::io::Result<()> {
    tokio::fs::create_dir_all(directory).await?;
    tokio::fs::write(record, json).await
}

async fn read_installed(record: &Path) -> Option<Vec<String>> {
    let bytes = match tokio::fs::read(record).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!("Could not read {}: {error}", record.display());
            return Some(Vec::new());
        }
    };
    Some(serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        log::warn!("Ignoring unreadable {}: {error}", record.display());
        Vec::new()
    }))
}

async fn jars_in(directory: &Path) -> Vec<String> {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };
    let mut jars = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Some(name) = entry.file_name().to_str()
            && name.ends_with(".jar")
        {
            jars.push(name.to_string());
        }
    }
    jars
}

// 힙 크기는 런처 설정의 메모리가 정한다. Mojang 의 기본값(-Xmx4G)이 남으면 뒤에 붙인 설정과 겹친다
fn without_heap_size(argument: Argument) -> Argument {
    Argument {
        rules: argument.rules,
        values: argument
            .values
            .into_iter()
            .filter(|value| !value.starts_with("-Xmx") && !value.starts_with("-Xms"))
            .collect(),
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, LauncherError> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(io_error(path))?;
    serde_json::from_str(&text).map_err(|error| {
        log::warn!("Unreadable {}: {error}", path.display());
        LauncherError::new(Problem::CorruptDownload, "게임 정보 파일을 읽지 못했어요.")
    })
}

#[cfg(test)]
mod tests;
