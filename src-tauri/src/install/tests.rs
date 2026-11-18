use std::collections::BTreeMap;

use super::pack::PackFile;
use super::*;

fn pack_file(file_name: &str) -> PackFile {
    PackFile { file_name: file_name.into(), url: "https://x".into(), sha512: "00".into(), size: 4 }
}

#[tokio::test]
async fn prunes_only_jars_missing_from_the_manifest() {
    let mods = tempfile::tempdir().unwrap();
    std::fs::write(mods.path().join("sodium.jar"), "kept").unwrap();
    std::fs::write(mods.path().join("old-sodium.jar"), "stale").unwrap();
    std::fs::write(mods.path().join("sodium.toml"), "config").unwrap();

    prune_mods(&[pack_file("sodium.jar")], mods.path()).await.unwrap();

    let mut left: Vec<String> =
        std::fs::read_dir(mods.path()).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["sodium.jar", "sodium.toml"]);
}

#[tokio::test]
async fn writes_default_options_only_once() {
    let root = tempfile::tempdir().unwrap();
    let options = root.path().join("options.txt");
    let defaults = BTreeMap::from([("lang".to_string(), "ko_kr".to_string())]);

    write_default_options(&defaults, &options).await.unwrap();
    assert_eq!(std::fs::read_to_string(&options).unwrap(), "lang:ko_kr\n");

    std::fs::write(&options, "lang:en_us\n").unwrap();
    write_default_options(&defaults, &options).await.unwrap();
    assert_eq!(std::fs::read_to_string(&options).unwrap(), "lang:en_us\n");
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
    let argument = Argument { rules: vec![], values: vec!["-Xmx2G".into(), "-Xms512M".into(), "-XX:+UseG1GC".into()] };
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

    let home = PathBuf::from(std::env::var("HYPERFARM_TEST_LIVE_HOME").expect("HYPERFARM_TEST_LIVE_HOME"));
    let http = Http::new();
    let manifest = std::fs::canonicalize("../pack/manifest.json").unwrap();
    let pack = pack::load(&http, &format!("file://{}", manifest.display())).await.unwrap();
    let platform = Platform::current();
    let layout = GameLayout::new(home);
    let mut installation = GameInstaller::new(http, layout.clone(), platform.clone()).install(&pack, &Silent).await.unwrap();
    installation.server = "127.0.0.1:1".into();
    let session = MinecraftSession { access_token: "offline".into(), player_id: uuid_like(), name: "qa_launcher".into(), xuid: "0".into() };
    let options = LaunchOptions { memory_megabytes: 2048, client_id: "test", launcher_version: "test" };

    let command = launch::command(&installation, &session, &options, &platform);
    let mut game = launch::start(&command, &installation.game_directory, layout.logs().join("live-test.log"), std::time::Duration::from_secs(180))
        .await
        .unwrap();
    assert!(game.try_wait().unwrap().is_none());
    game.kill().await.unwrap();
}

fn uuid_like() -> String {
    let bytes: [u8; 16] = rand::random();
    let hex = hex::encode(bytes);
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}
