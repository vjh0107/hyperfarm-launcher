mod auth;
mod error;
mod flow;
mod http;
mod install;
mod launch;
mod presence;
mod settings;
mod ticket;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;
use tokio::process::Child;

use flow::{LaunchEvent, LaunchFlow, Step};
use install::options::WorkArea;
use settings::{LauncherConfig, LauncherSettings};

const LAUNCH_EVENT: &str = "launch";
const UPDATE_EVENT: &str = "update";
const XBOX_PROFILE: &str = "https://account.xbox.com/";
const CLOSE_AFTER_LAUNCH: Duration = Duration::from_millis(1500);
const GAME_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const UPDATE_STALL_TIMEOUT: Duration = Duration::from_secs(30);

type Flow = Arc<LaunchFlow>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    web_base: String,
    game_directory: PathBuf,
    total_memory_bytes: u64,
}

#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
enum UpdateEvent {
    #[serde(rename_all = "camelCase")]
    Downloading {
        version: String,
        notes: Option<String>,
        done_bytes: u64,
        total_bytes: Option<u64>,
    },
    Installing,
}

#[derive(Default)]
struct Game(Mutex<Option<Child>>);

impl Game {
    fn start(&self, child: Child) {
        *self.0.lock().unwrap() = Some(child);
    }

    // 플래그가 아니라 프로세스를 본다. 기다리던 쪽이 어떤 길로 멈춰도 꺼진 게임이 다음 실행을 막지 않는다
    fn is_running(&self) -> bool {
        let mut game = self.0.lock().unwrap();
        let Some(child) = game.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => return true,
            Ok(Some(status)) => log::info!("The game exited with {status}"),
            Err(error) => log::warn!("Could not check on the game process: {error}"),
        }
        *game = None;
        false
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Startup {
    read_links: bool,
    check_for_update: bool,
}

// 남겨 둔 ticket 이 있으면 업데이트가 방금 다시 띄운 기동이다. Windows 의 NSIS 는 다시 띄울 때 처음 받은 argv(옛
// 링크)를 그대로 넘기므로 그것을 읽으면 남겨 둔 ticket 을 덮는다. 웹의 ticket 은 60초면 사라지는데 받기·설치·재기동에
// 이미 그 시간을 썼으니, 방금 깐 판을 다시 확인하느라 더 쓰지 않는다
fn startup(restored_ticket: bool) -> Startup {
    Startup {
        read_links: !restored_ticket,
        check_for_update: !restored_ticket,
    }
}

struct SkipUpdateCheck(AtomicBool);

#[tauri::command]
async fn begin(
    app: AppHandle,
    flow: State<'_, Flow>,
    skip: State<'_, SkipUpdateCheck>,
) -> Result<(), ()> {
    if !skip.0.swap(false, Ordering::Relaxed) && update(&app, &flow).await {
        return Ok(());
    }
    stay_while_playing(&app, flow.run(Step::Retry).await);
    Ok(())
}

#[tauri::command]
async fn run_step(app: AppHandle, step: Step, flow: State<'_, Flow>) -> Result<(), ()> {
    stay_while_playing(&app, flow.run(step).await);
    Ok(())
}

// 창은 닫고 게임이 도는 동안 남아 Discord 에 플레이 중을 띄운다. 게임이 꺼지면 함께 꺼진다
fn stay_while_playing(app: &AppHandle, game: Option<Child>) {
    let Some(game) = game else {
        return;
    };
    app.state::<Game>().start(game);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CLOSE_AFTER_LAUNCH).await;
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.destroy();
        }
        #[cfg(target_os = "macos")]
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        let homepage = app.state::<Flow>().web_base().to_string();
        let presence = tauri::async_runtime::spawn(presence::show(homepage));
        let mut check = tokio::time::interval(GAME_CHECK_INTERVAL);
        while app.state::<Game>().is_running() {
            check.tick().await;
        }
        presence.abort();
        app.exit(0);
    });
}

#[tauri::command]
fn cancel_sign_in(flow: State<'_, Flow>) {
    flow.cancel_sign_in();
}

#[tauri::command]
fn reopen_sign_in(app: AppHandle, flow: State<'_, Flow>) {
    if let Some(url) = flow.reopen_sign_in()
        && let Err(error) = app.opener().open_url(url, None::<&str>)
    {
        log::warn!("Could not reopen the sign-in page: {error}");
    }
}

#[tauri::command]
fn sign_out(flow: State<'_, Flow>) {
    flow.sign_out();
}

#[tauri::command]
fn app_info(flow: State<'_, Flow>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        web_base: flow.web_base().to_string(),
        game_directory: flow.game_directory(),
        total_memory_bytes: flow.total_memory_bytes(),
    }
}

#[tauri::command]
fn load_settings(flow: State<'_, Flow>) -> LauncherSettings {
    flow.settings()
}

#[tauri::command]
fn save_settings(settings: LauncherSettings, flow: State<'_, Flow>) -> Result<(), String> {
    flow.save_settings(&settings).map_err(|error| {
        log::warn!("Could not save launcher settings: {error}");
        "설정을 저장하지 못했어요.".to_string()
    })
}

#[tauri::command]
fn open_game_directory(app: AppHandle, flow: State<'_, Flow>) {
    let directory = flow.game_directory();
    let _ = std::fs::create_dir_all(&directory);
    if let Err(error) = app
        .opener()
        .open_path(directory.to_string_lossy(), None::<&str>)
    {
        log::warn!("Could not open the game directory: {error}");
    }
}

#[tauri::command]
fn open_game_log(app: AppHandle, flow: State<'_, Flow>) {
    if let Err(error) = app
        .opener()
        .open_path(flow.game_log().to_string_lossy(), None::<&str>)
    {
        log::warn!("Could not open the game log: {error}");
    }
}

#[tauri::command]
fn open_xbox_profile(app: AppHandle) {
    if let Err(error) = app.opener().open_url(XBOX_PROFILE, None::<&str>) {
        log::warn!("Could not open the Xbox profile page: {error}");
    }
}

#[tauri::command]
async fn choose_game_directory(
    app: AppHandle,
    flow: State<'_, Flow>,
) -> Result<Option<PathBuf>, String> {
    let Some(picked) = app
        .dialog()
        .file()
        .set_directory(flow.game_directory())
        .blocking_pick_folder()
    else {
        return Ok(None);
    };
    let unusable = || "이 폴더는 쓸 수 없어요.".to_string();
    let directory = flow::game_root(&picked.into_path().map_err(|_| unusable())?);
    std::fs::create_dir_all(&directory).map_err(|error| {
        log::warn!("Could not create the game directory: {error}");
        unusable()
    })?;
    let mut settings = flow.settings();
    settings.game_directory = Some(directory.clone());
    save_settings(settings, flow)?;
    Ok(Some(directory))
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

// 고칠 판이 있으면 내려받아 깔고 다시 띄운다. 확인이 실패해도 지금 판으로 게임은 켠다.
// latest.json 이 가리키는 판과 다르기만 하면 받으므로, 게시한 판을 옛 판으로 되돌리면 설치된 런처도 따라 내려간다
async fn update(app: &AppHandle, flow: &LaunchFlow) -> bool {
    let Some(_held) = flow.hold() else {
        log::info!("Skipping the launcher update check while the launcher is busy");
        return false;
    };
    let checked = match app
        .updater_builder()
        .version_comparator(|current, remote| remote.version != current)
        .timeout(UPDATE_CHECK_TIMEOUT)
        .configure_client(|client| client.read_timeout(UPDATE_STALL_TIMEOUT))
        .build()
    {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let mut update = match checked {
        Ok(Some(update)) => update,
        Ok(None) => return false,
        Err(error) => {
            log::warn!("Could not check for launcher updates: {error}");
            return false;
        }
    };
    // 확인의 timeout 이 받기에도 그대로 이어져, 느린 연결에서는 다 받기 전에 끊긴다
    update.timeout = Some(UPDATE_DOWNLOAD_TIMEOUT);
    let version = update.version.clone();
    let notes = update.body.as_deref().and_then(release_summary);
    let mut done_bytes = 0u64;
    let installed = update
        .download_and_install(
            |chunk, total_bytes| {
                done_bytes += chunk as u64;
                let _ = app.emit(
                    UPDATE_EVENT,
                    UpdateEvent::Downloading {
                        version: version.clone(),
                        notes: notes.clone(),
                        done_bytes,
                        total_bytes,
                    },
                );
            },
            // Windows 는 설치 프로그램을 띄우는 자리에서 곧바로 꺼지므로 restart 앞이 아니라 여기서 남긴다
            || {
                flow.keep_ticket_for_restart();
                let _ = app.emit(UPDATE_EVENT, UpdateEvent::Installing);
            },
        )
        .await;
    match installed {
        Ok(()) => app.restart(),
        Err(error) => {
            log::warn!("Could not install launcher {version}: {error}");
            flow.forget_kept_ticket();
            false
        }
    }
}

// GitHub 이 만든 릴리스 노트의 첫 항목에서 작성자와 PR 주소를 떼어 화면 한 줄로 쓴다
fn release_summary(body: &str) -> Option<String> {
    let item = body
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("* ").or_else(|| line.strip_prefix("- ")))?;
    let title = item.split(" by @").next().unwrap_or(item).trim();
    (!title.is_empty()).then(|| title.to_string())
}

// 게임 창은 주 모니터에 뜬다. 모르면 options.txt 의 배율을 건드리지 않는다
fn primary_work_area(app: &AppHandle) -> Option<WorkArea> {
    let monitor = match app.primary_monitor() {
        Ok(Some(monitor)) => monitor,
        Ok(None) => {
            log::info!("No primary monitor; leaving the GUI scale to the game");
            return None;
        }
        Err(error) => {
            log::warn!("Could not read the primary monitor: {error}");
            return None;
        }
    };
    let size = monitor.work_area().size;
    Some(WorkArea {
        width: size.width,
        height: size.height,
        scale_factor: monitor.scale_factor(),
    })
}

fn accept_links(app: &AppHandle, urls: impl IntoIterator<Item = String>) -> bool {
    let flow = app.state::<Flow>();
    urls.into_iter().any(|url| flow.accept_uri(&url))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            },
        ))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("launcher".into()),
                    },
                ))
                .max_file_size(2 * 1024 * 1024)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            let opener = handle.clone();
            let browser = Arc::new(move |url: &str| {
                opener
                    .opener()
                    .open_url(url, None::<&str>)
                    .map_err(std::io::Error::other)
            });
            let emitter = handle.clone();
            let emit = Arc::new(move |event: LaunchEvent| {
                let _ = emitter.emit(LAUNCH_EVENT, event);
            });
            let screens = handle.clone();
            let work_area = Arc::new(move || primary_work_area(&screens));
            let flow = Arc::new(LaunchFlow::new(
                LauncherConfig::load(),
                browser,
                emit,
                work_area,
            ));
            let startup = startup(flow.restore_kept_ticket());
            app.manage::<Flow>(flow);
            app.manage(SkipUpdateCheck(AtomicBool::new(!startup.check_for_update)));

            #[cfg(any(windows, target_os = "linux"))]
            if let Err(error) = app.deep_link().register_all() {
                log::warn!("Could not register the hyperfarm:// scheme: {error}");
            }
            if startup.read_links
                && let Ok(Some(urls)) = app.deep_link().get_current()
            {
                accept_links(&handle, urls.into_iter().map(String::from));
            }
            let linked = handle.clone();
            app.deep_link().on_open_url(move |event| {
                // 같은 계정으로 하나 더 켜면 먼저 켠 게임이 서버에서 끊긴다
                if linked.state::<Game>().is_running() {
                    log::info!("Ignoring a launch link while the game is running");
                    return;
                }
                if !accept_links(&linked, event.urls().into_iter().map(String::from)) {
                    return;
                }
                let app = linked.clone();
                let flow = linked.state::<Flow>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    // 창을 띄워 둔 채 새 판이 나왔을 수 있다. 실행이나 다른 확인이 돌고 있으면 update 가 건너뛴다
                    if update(&app, &flow).await {
                        return;
                    }
                    stay_while_playing(&app, flow.run(Step::Retry).await);
                });
            });
            Ok(())
        })
        .manage(Game::default())
        .invoke_handler(tauri::generate_handler![
            begin,
            run_step,
            cancel_sign_in,
            reopen_sign_in,
            sign_out,
            app_info,
            load_settings,
            save_settings,
            open_game_directory,
            open_game_log,
            open_xbox_profile,
            choose_game_directory,
            quit,
        ])
        .build(tauri::generate_context!())
        .expect("the launcher window failed to start")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
                && app.state::<Game>().is_running()
            {
                api.prevent_exit();
            }
        });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Game, Startup, release_summary, startup};

    fn sleeping_process() -> tokio::process::Child {
        #[cfg(windows)]
        let mut command = {
            let mut command = tokio::process::Command::new("powershell");
            command.args(["-NoProfile", "-Command", "Start-Sleep 30"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = tokio::process::Command::new("sleep");
            command.arg("30");
            command
        };
        command.kill_on_drop(true).spawn().unwrap()
    }

    #[tokio::test]
    async fn stops_counting_the_game_as_running_once_its_process_is_gone() {
        let game = Game::default();
        assert!(!game.is_running());
        game.start(sleeping_process());
        assert!(game.is_running());

        game.0
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .start_kill()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while game.is_running() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the killed game still counts as running");
    }

    #[test]
    fn keeps_the_ticket_an_update_restart_carried_and_does_not_check_again() {
        assert_eq!(
            startup(true),
            Startup {
                read_links: false,
                check_for_update: false,
            }
        );
        assert_eq!(
            startup(false),
            Startup {
                read_links: true,
                check_for_update: true,
            }
        );
    }

    #[test]
    fn summarizes_the_first_release_note_item() {
        let body = "## What's Changed\n### 고친 것\n* 로그인이 늘 실패하던 것을 고친다 by @vjh0107 in https://github.com/x/y/pull/4\n* 다른 것";
        assert_eq!(
            release_summary(body).as_deref(),
            Some("로그인이 늘 실패하던 것을 고친다")
        );
        assert_eq!(release_summary("**Full Changelog**: a...b"), None);
    }
}
