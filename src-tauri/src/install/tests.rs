use super::pack::PackFile;
use super::*;

fn pack_file(file_name: &str) -> PackFile {
    PackFile {
        file_name: file_name.into(),
        url: "https://x".into(),
        sha512: "00".into(),
        size: 4,
    }
}

fn names_in(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != INSTALLED_MODS)
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn prunes_only_jars_it_installed_that_left_the_manifest() {
    let mods = tempfile::tempdir().unwrap();
    std::fs::write(mods.path().join("sodium-old.jar"), "ours").unwrap();
    std::fs::write(mods.path().join("lithium.jar"), "ours").unwrap();
    std::fs::write(mods.path().join("players-own.jar"), "theirs").unwrap();
    prune_mods(
        &[pack_file("sodium-old.jar"), pack_file("lithium.jar")],
        mods.path(),
        false,
    )
    .await
    .unwrap();
    std::fs::write(mods.path().join("sodium-new.jar"), "ours").unwrap();

    prune_mods(
        &[pack_file("sodium-new.jar"), pack_file("lithium.jar")],
        mods.path(),
        false,
    )
    .await
    .unwrap();

    assert_eq!(
        names_in(mods.path()),
        ["lithium.jar", "players-own.jar", "sodium-new.jar"]
    );
}

#[tokio::test]
async fn keeps_every_jar_in_a_folder_it_has_not_installed_into() {
    let mods = tempfile::tempdir().unwrap();
    std::fs::write(mods.path().join("sodium.jar"), "ours").unwrap();
    std::fs::write(mods.path().join("players-own.jar"), "theirs").unwrap();

    prune_mods(&[pack_file("sodium.jar")], mods.path(), false)
        .await
        .unwrap();

    assert_eq!(names_in(mods.path()), ["players-own.jar", "sodium.jar"]);
}

#[tokio::test]
async fn clears_old_jars_from_its_own_folder_before_it_kept_a_record() {
    let mods = tempfile::tempdir().unwrap();
    std::fs::write(mods.path().join("sodium-0.9.2.jar"), "older launcher").unwrap();
    std::fs::write(mods.path().join("sodium-0.9.3.jar"), "ours").unwrap();
    std::fs::write(mods.path().join("sodium.toml"), "config").unwrap();

    prune_mods(&[pack_file("sodium-0.9.3.jar")], mods.path(), true)
        .await
        .unwrap();

    let mut left = names_in(mods.path());
    left.retain(|name| !name.starts_with('.'));
    assert_eq!(left, ["sodium-0.9.3.jar", "sodium.toml"]);
}

#[test]
fn fetches_our_jars_past_the_edge_cache_and_modrinth_as_published() {
    let sha512 = format!("{}{}", "AB".repeat(8), "cd".repeat(56));
    let file = |url: &str| PackFile {
        file_name: "a.jar".into(),
        url: url.into(),
        sha512: sha512.clone(),
        size: 4,
    };
    let modrinth = "https://cdn.modrinth.com/data/x/versions/y/sodium.jar";

    let tasks = pack_tasks(
        &[
            file("https://pack.hyperf.kr/mods/hyperfarm-0.1.0.jar"),
            file(modrinth),
        ],
        Path::new("/mods"),
    )
    .unwrap();

    assert_eq!(
        tasks[0].url,
        "https://pack.hyperf.kr/mods/hyperfarm-0.1.0.jar?v=abababababababab"
    );
    assert_eq!(tasks[1].url, modrinth);
}

#[test]
fn reads_the_server_pack_only_under_the_key_old_launchers_ignore() {
    let manifest = |extra: &str| -> PackManifest {
        serde_json::from_str(&format!(
            r#"{{"minecraftVersion": "26.3", "fabricLoaderVersion": "0.20.1", "server": "hyperf.kr",
                "mods": [], "shaderpacks": [], {extra}}}"#
        ))
        .unwrap()
    };

    assert_eq!(
        manifest(r#""serverResourcePackUrl": "https://pack-prod.hyperf.kr/resource_pack.zip""#)
            .server_resource_pack_url
            .as_deref(),
        Some("https://pack-prod.hyperf.kr/resource_pack.zip")
    );
    assert!(
        manifest(r#""serverResourcePack": "https://pack-prod.hyperf.kr/resource_pack.zip""#)
            .server_resource_pack_url
            .is_none()
    );
}

#[tokio::test]
async fn starts_from_the_saved_fabric_profile_while_the_meta_server_is_down() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"id": "fabric-loader-0.20.1-26.3", "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient", "libraries": []}"#,
        ))
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    let saved = root.path().join("fabric-loader-0.20.1-26.3.json");
    let url = format!("{}/profile/json", server.uri());
    let http = Http::new();
    fabric_profile(&http, &url, &saved).await.unwrap();

    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let profile = fabric_profile(&http, &url, &saved).await.unwrap();
    assert_eq!(profile.id, "fabric-loader-0.20.1-26.3");
    let unsaved = fabric_profile(&http, &url, &root.path().join("other.json"))
        .await
        .err()
        .unwrap();
    assert_eq!(unsaved.problem, Problem::Network);
}

#[cfg(unix)]
struct ReadOnly(PathBuf);

#[cfg(unix)]
impl ReadOnly {
    fn new(directory: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o555)).unwrap();
        Self(directory.to_path_buf())
    }
}

#[cfg(unix)]
impl Drop for ReadOnly {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn starts_from_a_fresh_fabric_profile_it_cannot_keep() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"id": "fabric-loader-0.20.1-26.3", "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient", "libraries": []}"#,
        ))
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    let _read_only = ReadOnly::new(root.path());

    let profile = fabric_profile(
        &Http::new(),
        &format!("{}/profile/json", server.uri()),
        &root.path().join("versions/fabric/fabric.json"),
    )
    .await
    .unwrap();

    assert_eq!(profile.id, "fabric-loader-0.20.1-26.3");
}

#[cfg(unix)]
#[tokio::test]
async fn finishes_pruning_when_it_cannot_write_the_record() {
    let mods = tempfile::tempdir().unwrap();
    std::fs::write(mods.path().join("sodium.jar"), "ours").unwrap();
    let _read_only = ReadOnly::new(mods.path());

    prune_mods(&[pack_file("sodium.jar")], mods.path(), true)
        .await
        .unwrap();

    assert!(!mods.path().join(INSTALLED_MODS).exists());
}

#[test]
fn refuses_pack_file_names_that_leave_the_mods_directory() {
    for name in ["../evil.jar", "a/b.jar", "a\\b.jar", ".hidden.jar"] {
        assert!(pack_file(name).safe_file_name().is_err(), "{name}");
    }
    assert!(pack_file("sodium.jar").safe_file_name().is_ok());
}

#[test]
fn drops_only_heap_size_from_default_user_arguments() {
    let argument = Argument {
        rules: vec![],
        values: vec!["-Xmx2G".into(), "-Xms512M".into(), "-XX:+UseG1GC".into()],
    };
    assert_eq!(without_heap_size(argument).values, ["-XX:+UseG1GC"]);
}

#[tokio::test]
#[ignore = "HYPERFARM_TEST_LIVE_HOME 에 실제 게임을 받아 창이 뜨는지 본다"]
async fn installs_and_opens_the_game_window() {
    use crate::auth::MinecraftSession;
    use crate::launch::{self, LaunchOptions};

    struct Silent;
    impl InstallListener for Silent {
        fn downloading(&self, _: DownloadPlan) {}
        fn progress(&self) -> Progress {
            std::sync::Arc::new(|_, _, _| {})
        }
    }

    let home =
        PathBuf::from(std::env::var("HYPERFARM_TEST_LIVE_HOME").expect("HYPERFARM_TEST_LIVE_HOME"));
    let http = Http::new();
    let manifest = std::fs::canonicalize("../pack/manifest.json").unwrap();
    let pack = pack::load(&http, &format!("file://{}", manifest.display()))
        .await
        .unwrap();
    let platform = Platform::current();
    let layout = GameLayout::new(home);
    let mut installation = GameInstaller::new(http, layout.clone(), platform.clone())
        .install(&pack, &Silent)
        .await
        .unwrap();
    installation.server = "127.0.0.1:1".into();
    let session = MinecraftSession {
        access_token: "offline".into(),
        player_id: uuid_like(),
        name: "qa_launcher".into(),
        xuid: "0".into(),
    };
    let options = LaunchOptions {
        memory_megabytes: 2048,
        client_id: "test",
        launcher_version: "test",
    };

    let command = launch::command(&installation, &session, &options, &platform);
    let mut game = launch::start(
        &command,
        &installation.game_directory,
        layout.logs().join("live-test.log"),
        std::time::Duration::from_secs(180),
    )
    .await
    .unwrap();
    assert!(game.try_wait().unwrap().is_none());
    game.kill().await.unwrap();
}

fn uuid_like() -> String {
    let bytes: [u8; 16] = rand::random();
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
