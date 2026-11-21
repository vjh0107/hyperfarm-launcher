use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use regex_lite::{Captures, Regex};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tokio::process::{Child, Command};

use crate::auth::MinecraftSession;
use crate::crash::Crash;
use crate::error::{LauncherError, Problem};
use crate::install::Installation;
use crate::install::rules::{Platform, resolve_arguments};

// 창을 띄우기 직전에 찍히는 줄이다. 이것을 보기 전에 launcher 가 꺼지면 첫 실행의 오류를 볼 길이 없다
const WINDOW_MARKER: &str = "Backend library:";
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const FEATURES: [&str; 1] = ["is_quick_play_multiplayer"];
const FABRIC_NO_GUI: &str = "fabric.noGui";
// 공식 launcher 의 기본 프로필과 같은 GC 다. 판에 든 JVM 인자에는 GC 가 없어, 이것을 빼면 손대지 않은 G1 이 돌아
// 같은 PC 에서 공식 launcher 보다 끊김이 잦다
const INITIAL_HEAP_MEGABYTES: u32 = 2048;
const GC_ARGUMENTS: [&str; 4] = [
    "-XX:+UseCompactObjectHeaders",
    "-XX:+AlwaysPreTouch",
    "-XX:+UseStringDeduplication",
    "-XX:+UseZGC",
];
// 판의 게임 인자가 ${game_directory} 앞에 두는 이름이다. launcher 가 붙이는 것이 아니라 옛 판이 켠 게임에도 있다
const GAME_DIRECTORY_ARGUMENT: &str = "--gameDir";

pub struct GameProcess {
    pub child: Child,
    pub started: SystemTime,
    pub directory: PathBuf,
    pub log: PathBuf,
}

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

    let mut command = vec![path(&installation.java)];
    command.extend(memory_arguments(options.memory_megabytes));
    // 보이스챗이 webrtc 네이티브를 System.load 로 싣는다. Java 25 는 경고만 내지만 이후 판은 막는다.
    command.push("--enable-native-access=ALL-UNNAMED".into());
    // 없으면 모드가 시작 단계에서 실패할 때 Fabric loader 가 Swing 창을 띄우고, 그 창을 닫을 때까지 프로세스가 살아 있어
    // 런처는 크래시를 알지 못한다. 켜 두면 loader 가 오류를 로그에 남기고 곧바로 System.exit(1) 로 끝난다(FabricGuiEntry)
    command.push(format!("-D{FABRIC_NO_GUI}=true"));
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
) -> Result<GameProcess, LauncherError> {
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
    let started = SystemTime::now();
    let mut child = process.spawn().map_err(failed)?;

    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(LauncherError::crashed(Crash::detect(
                status.code(),
                working_directory,
                &log,
                started,
            )));
        }
        if tokio::fs::read(&log)
            .await
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(WINDOW_MARKER))
        {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(GameProcess {
        child,
        started,
        directory: working_directory.to_path_buf(),
        log,
    })
}

// launcher 가 꺼진 뒤에도 남은 게임은 mods 의 jar 를 쥔 채 돈다. 다른 launcher 로 켠 게임은 다른 폴더를 --gameDir 로 받는다.
// 명령줄에는 액세스 토큰이 들어 있으므로 로그에는 pid 만 남긴다
pub fn running_in(game_directory: &Path) -> Option<Pid> {
    let mut system = match System::new() {
        Ok(system) => system,
        Err(error) => {
            log::warn!("Could not list processes to look for a running game: {error}");
            return None;
        }
    };
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    system
        .processes()
        .values()
        .find(|process| launched_in(process.cmd(), game_directory))
        .map(sysinfo::Process::pid)
}

fn launched_in(command: &[impl AsRef<OsStr>], game_directory: &Path) -> bool {
    command.windows(2).any(|pair| {
        pair[0].as_ref() == GAME_DIRECTORY_ARGUMENT
            && same_directory(Path::new(pair[1].as_ref()), game_directory)
    })
}

// Windows 는 대소문자와 구분자가, macOS 는 링크(/var 와 /private/var)가 달라도 같은 폴더라 글자로만 견주면 놓친다
fn same_directory(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation(game_directory: &Path) -> Installation {
        Installation {
            java: PathBuf::from("java"),
            classpath: Vec::new(),
            main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".into(),
            jvm_arguments: Vec::new(),
            game_arguments: vec![crate::install::rules::Argument {
                rules: Vec::new(),
                values: [
                    "--username",
                    "${auth_player_name}",
                    "--gameDir",
                    "${game_directory}",
                    "--accessToken",
                    "${auth_access_token}",
                ]
                .map(String::from)
                .to_vec(),
            }],
            version_name: "fabric-loader-0.20.1-26.3".into(),
            version_type: "release".into(),
            asset_index: "30".into(),
            assets_root: PathBuf::from("assets"),
            libraries_root: PathBuf::from("libraries"),
            natives: PathBuf::from("natives"),
            game_directory: game_directory.to_path_buf(),
            server: "hyperf.kr".into(),
        }
    }

    fn session() -> MinecraftSession {
        MinecraftSession {
            access_token: "token".into(),
            player_id: "id".into(),
            name: "name".into(),
            xuid: "0".into(),
        }
    }

    fn options() -> LaunchOptions<'static> {
        LaunchOptions {
            memory_megabytes: 4096,
            client_id: "client",
            launcher_version: "test",
        }
    }

    #[test]
    fn finds_the_game_it_launched_into_this_folder_and_no_other() {
        let ours = tempfile::tempdir().unwrap();
        let vanilla = tempfile::tempdir().unwrap();
        let command = |directory: &Path| {
            command(
                &installation(directory),
                &session(),
                &options(),
                &Platform::current(),
            )
        };

        assert!(launched_in(&command(ours.path()), ours.path()));
        assert!(!launched_in(&command(vanilla.path()), ours.path()));
        let mentions_the_folder_elsewhere = [
            "java".into(),
            format!("-Djava.library.path={}", ours.path().display()),
            "--assetsDir".into(),
            ours.path().display().to_string(),
        ];
        assert!(!launched_in(&mentions_the_folder_elsewhere, ours.path()));
    }

    #[tokio::test]
    async fn finds_a_running_process_by_the_folder_it_was_given() {
        let ours = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("powershell");
            // -Command 는 뒤의 인자를 모두 이어 명령으로 읽으므로 # 로 주석이 되게 한다
            command.args(["-NoProfile", "-Command", "Start-Sleep 30 #"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            // 명령이 sleep 하나뿐이면 sh 가 sleep 으로 exec 해 --gameDir 가 명령줄에서 사라진다
            command.args(["-c", "sleep 30; exit 0", "sh"]);
            command
        };
        let game = command
            .arg("--gameDir")
            .arg(ours.path())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = Pid::from_u32(game.id().unwrap());

        let found = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(found) = running_in(ours.path()) {
                    return found;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the running game was not found");
        assert_eq!(found, pid);
        assert_eq!(running_in(elsewhere.path()), None);
    }

    #[test]
    fn recognizes_the_folder_however_its_path_was_spelled() {
        let ours = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ours.path().join("mods")).unwrap();
        let spelled = ours.path().join("mods").join("..");
        let command = [OsStr::new("--gameDir"), spelled.as_os_str()];
        assert!(launched_in(&command, ours.path()));
    }

    #[test]
    fn starts_the_heap_no_larger_than_its_limit() {
        assert_eq!(memory_arguments(6144)[..2], ["-Xms2048M", "-Xmx6144M"]);
        assert_eq!(memory_arguments(1024)[..2], ["-Xms1024M", "-Xmx1024M"]);
        assert!(memory_arguments(4096).contains(&"-XX:+UseZGC".to_string()));
    }

    #[test]
    fn tells_the_fabric_loader_to_exit_instead_of_opening_its_error_window() {
        let installation = installation(Path::new("."));
        let command = command(&installation, &session(), &options(), &Platform::current());
        let property = command
            .iter()
            .position(|argument| argument == "-Dfabric.noGui=true")
            .expect("the loader still opens its own error window");
        let main_class = command
            .iter()
            .position(|argument| *argument == installation.main_class)
            .unwrap();
        assert!(property < main_class);
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
