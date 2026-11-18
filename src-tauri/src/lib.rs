mod auth;
mod error;
mod flow;
mod http;
mod install;
mod launch;
mod settings;
mod ticket;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;

use flow::{LaunchEvent, LaunchFlow, Step};
use settings::{LauncherConfig, LauncherSettings};

const LAUNCH_EVENT: &str = "launch";
const UPDATE_EVENT: &str = "update";
const XBOX_PROFILE: &str = "https://account.xbox.com/";
const CLOSE_AFTER_LAUNCH: Duration = Duration::from_millis(1500);

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

#[tauri::command]
async fn begin(app: AppHandle, flow: State<'_, Flow>) -> Result<(), ()> {
    if update(&app).await {
        return Ok(());
    }
    flow.run(Step::Retry).await;
    Ok(())
}

#[tauri::command]
async fn run_step(step: Step, flow: State<'_, Flow>) -> Result<(), ()> {
    flow.run(step).await;
    Ok(())
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
    let directory = picked
        .into_path()
        .map_err(|_| "이 폴더는 쓸 수 없어요.".to_string())?;
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
async fn update(app: &AppHandle) -> bool {
    let checked = match app
        .updater_builder()
        .version_comparator(|current, remote| remote.version != current)
        .build()
    {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let update = match checked {
        Ok(Some(update)) => update,
        Ok(None) => return false,
        Err(error) => {
            log::warn!("Could not check for launcher updates: {error}");
            return false;
        }
    };
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
            || {
                let _ = app.emit(UPDATE_EVENT, UpdateEvent::Installing);
            },
        )
        .await;
    match installed {
        Ok(()) => app.restart(),
        Err(error) => {
            log::warn!("Could not install launcher {version}: {error}");
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
                if matches!(event, LaunchEvent::Launched) {
                    let closing = emitter.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(CLOSE_AFTER_LAUNCH).await;
                        closing.exit(0);
                    });
                }
                let _ = emitter.emit(LAUNCH_EVENT, event);
            });
            app.manage::<Flow>(Arc::new(LaunchFlow::new(
                LauncherConfig::load(),
                browser,
                emit,
            )));

            #[cfg(any(windows, target_os = "linux"))]
            if let Err(error) = app.deep_link().register_all() {
                log::warn!("Could not register the hyperfarm:// scheme: {error}");
            }
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                accept_links(&handle, urls.into_iter().map(String::from));
            }
            let linked = handle.clone();
            app.deep_link().on_open_url(move |event| {
                if !accept_links(&linked, event.urls().into_iter().map(String::from)) {
                    return;
                }
                let app = linked.clone();
                let flow = linked.state::<Flow>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    // 창을 띄워 둔 채 새 판이 나왔을 수 있다. 실행 중이면 그 실행을 끊지 않는다
                    if !flow.is_busy() && update(&app).await {
                        return;
                    }
                    flow.run(Step::Retry).await;
                });
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            begin,
            run_step,
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
        .run(tauri::generate_context!())
        .expect("the launcher window failed to start");
}

#[cfg(test)]
mod tests {
    use super::release_summary;

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
