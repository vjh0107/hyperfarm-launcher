use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use regex_lite::{Captures, Regex};
use tokio::process::{Child, Command};

use crate::auth::MinecraftSession;
use crate::error::{LauncherError, Problem};
use crate::install::Installation;
use crate::install::rules::{Platform, resolve_arguments};

// 창을 띄우기 직전에 찍히는 줄이다. 이것을 보기 전에 launcher 가 꺼지면 첫 실행의 오류를 볼 길이 없다
const WINDOW_MARKER: &str = "Backend library:";
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const FEATURES: [&str; 1] = ["is_quick_play_multiplayer"];
// 공식 launcher 의 기본 프로필과 같은 GC 다. 판에 든 JVM 인자에는 GC 가 없어, 이것을 빼면 손대지 않은 G1 이 돌아
// 같은 PC 에서 공식 launcher 보다 끊김이 잦다
const INITIAL_HEAP_MEGABYTES: u32 = 2048;
const GC_ARGUMENTS: [&str; 4] = [
    "-XX:+UseCompactObjectHeaders",
    "-XX:+AlwaysPreTouch",
    "-XX:+UseStringDeduplication",
    "-XX:+UseZGC",
];

pub struct LaunchOptions<'a> {
    pub memory_megabytes: u32,
    pub client_id: &'a str,
    pub launcher_version: &'a str,
}

pub fn command(
    installation: &Installation,
    session: &MinecraftSession,
    options: &LaunchOptions,
    platform: &Platform,
) -> Vec<String> {
    let separator = platform.classpath_separator();
    let classpath = installation
        .classpath
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join(separator);
    let path = |path: &Path| path.to_string_lossy().into_owned();
    let variables: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", session.name.clone()),
        ("auth_uuid", session.player_id.replace('-', "")),
        ("auth_access_token", session.access_token.clone()),
        ("auth_xuid", session.xuid.clone()),
        ("user_type", "msa".into()),
        ("clientid", options.client_id.into()),
        ("version_name", installation.version_name.clone()),
        ("version_type", installation.version_type.clone()),
        ("game_directory", path(&installation.game_directory)),
        ("assets_root", path(&installation.assets_root)),
        ("assets_index_name", installation.asset_index.clone()),
        ("natives_directory", path(&installation.natives)),
        ("library_directory", path(&installation.libraries_root)),
        ("classpath_separator", separator.into()),
        ("classpath", classpath),
        ("launcher_name", "hyperfarm-launcher".into()),
        ("launcher_version", options.launcher_version.into()),
        ("quickPlayMultiplayer", installation.server.clone()),
    ]);
    let features: HashSet<&str> = FEATURES.into_iter().collect();
    let substitute = |arguments| {
        resolve_arguments(arguments, platform, &features)
            .into_iter()
            .map(|template| substitute(&template, &variables))
    };

    // 보이스챗이 webrtc 네이티브를 System.load 로 싣는다. Java 25 는 경고만 내지만 이후 판은 막는다.
    let mut command = vec![path(&installation.java)];
    command.extend(memory_arguments(options.memory_megabytes));
    command.push("--enable-native-access=ALL-UNNAMED".into());
    command.extend(substitute(&installation.jvm_arguments));
    command.push(installation.main_class.clone());
    command.extend(substitute(&installation.game_arguments));
    command
}

fn memory_arguments(memory_megabytes: u32) -> Vec<String> {
    let mut arguments = vec![
        format!("-Xms{}M", memory_megabytes.min(INITIAL_HEAP_MEGABYTES)),
        format!("-Xmx{memory_megabytes}M"),
    ];
    arguments.extend(GC_ARGUMENTS.map(String::from));
    arguments
}

fn substitute(template: &str, variables: &HashMap<&str, String>) -> String {
    let placeholder = Regex::new(r"\$\{([A-Za-z_]+)\}").expect("the placeholder pattern is valid");
    placeholder
        .replace_all(template, |captures: &Captures| {
            variables
                .get(&captures[1])
                .cloned()
                .unwrap_or_else(|| captures[0].to_string())
        })
        .into_owned()
}

pub async fn start(
    command: &[String],
    working_directory: &Path,
    log: PathBuf,
    timeout: Duration,
) -> Result<Child, LauncherError> {
    let failed = |error: std::io::Error| {
        log::warn!("Could not start the game: {error}");
        LauncherError::new(Problem::GameExited, "게임을 실행하지 못했어요.")
    };
    if let Some(parent) = log.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(failed)?;
    }
    // 출력은 파이프가 아니라 파일로 곧장 보낸다. launcher 가 먼저 꺼지면 파이프가 닫혀 게임이 쓰다가 죽는다
    let output = std::fs::File::create(&log).map_err(failed)?;
    let errors = output.try_clone().map_err(failed)?;
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .current_dir(working_directory)
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(errors);
    #[cfg(windows)]
    process.creation_flags(0x0800_0000);
    let mut child = process.spawn().map_err(failed)?;

    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            let code = status
                .code()
                .map_or_else(|| "알 수 없음".to_string(), |code| code.to_string());
            return Err(LauncherError::new(
                Problem::GameExited,
                format!("게임이 시작하자마자 꺼졌어요. (코드 {code})"),
            ));
        }
        if tokio::fs::read(&log)
            .await
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(WINDOW_MARKER))
        {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_the_heap_no_larger_than_its_limit() {
        assert_eq!(memory_arguments(6144)[..2], ["-Xms2048M", "-Xmx6144M"]);
        assert_eq!(memory_arguments(1024)[..2], ["-Xms1024M", "-Xmx1024M"]);
        assert!(memory_arguments(4096).contains(&"-XX:+UseZGC".to_string()));
    }

    #[test]
    fn keeps_unknown_placeholders() {
        let variables = HashMap::from([("auth_player_name", "Steve".to_string())]);
        assert_eq!(
            substitute("--username=${auth_player_name} ${unknown}", &variables),
            "--username=Steve ${unknown}"
        );
    }
}
