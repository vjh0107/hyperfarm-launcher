use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::auth::microsoft::{BrowserOpener, MicrosoftClient, MicrosoftEndpoints};
use crate::auth::store::KeyringAccountStore;
use crate::auth::xbox::{XboxClient, XboxEndpoints};
use crate::auth::{AuthError, Authenticator, MinecraftSession};
use crate::crash::Crash;
use crate::error::{LauncherError, Problem};
use crate::http::Http;
use crate::install::download::Progress;
use crate::install::options::{self, WorkArea};
use crate::install::rules::Platform;
use crate::install::{
    DownloadPlan, GameInstaller, GameLayout, InstallListener, Installation, pack,
};
use crate::launch::{self, GameProcess, LaunchOptions};
use crate::settings::{CLIENT_ID, LauncherConfig, LauncherSettings, SettingsStore};
use crate::ticket::{self, LaunchTicket};

const WINDOW_TIMEOUT: Duration = Duration::from_secs(120);
// 준비해 둔 로그인과 설치를 이만큼 지나서 시작하면 처음부터 다시 맞춘다. 창을 오래 띄워 두는 사이 Minecraft 토큰(24시간)이
// 끝나거나 새 모드 구성이 게시되면, 그 준비로 켠 게임은 접속하지 못하거나 옛 모드로 들어간다
const PREPARED_FOR: Duration = Duration::from_secs(60 * 60);
const SIGNED_OUT: &str = "로그아웃했어요. 다시 시도하면 로그인부터 해요.";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const GAME_LOG: &str = "launcher-game.log";
const SESSION_PROFILE: &str = "https://sessionserver.mojang.com/session/minecraft/profile/";
const GAME_FOLDER: &str = "HyperFarm";
const GAME_MARKER: &str = ".hyperfarm";
const OS_CLUTTER: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum LaunchEvent {
    Checking,
    WaitingForSignIn,
    #[serde(rename_all = "camelCase")]
    SignedIn {
        player_id: String,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    Preparing {
        minecraft_version: String,
        mods: usize,
        shaderpacks: usize,
    },
    Downloading(DownloadPlan),
    #[serde(rename_all = "camelCase")]
    Progress {
        done_bytes: u64,
        total_bytes: u64,
        file: String,
    },
    Ready {
        server: String,
    },
    Launching {
        server: String,
    },
    Launched,
    Failed {
        error: LauncherError,
    },
    #[serde(rename_all = "camelCase")]
    Mismatch {
        web_player_id: String,
        web_name: String,
        session_player_id: String,
        session_name: String,
    },
}

pub type Emitter = Arc<dyn Fn(LaunchEvent) + Send + Sync>;
pub type WorkAreaProbe = Arc<dyn Fn() -> Option<WorkArea> + Send + Sync>;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Retry,
    SwitchAccount,
    ContinueAnyway,
    Play,
}

impl Step {
    // 플레이어가 이미 시작을 고른 단계다. 준비가 끝나도 게임 시작 버튼에서 한 번 더 기다리지 않는다
    fn starts_the_game(self) -> bool {
        matches!(self, Step::ContinueAnyway | Step::Play)
    }
}

struct Prepared {
    session: MinecraftSession,
    installation: Installation,
    layout: GameLayout,
    default_options: BTreeMap<String, String>,
    at: Instant,
}

enum Outcome {
    Ready(String),
    Started(GameProcess),
}

#[derive(Default)]
pub struct Busy(AtomicBool);

pub struct Held<'a>(&'a AtomicBool);

impl Busy {
    pub fn hold(&self) -> Option<Held<'_>> {
        (!self.0.swap(true, Ordering::SeqCst)).then_some(Held(&self.0))
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct LaunchFlow {
    config: LauncherConfig,
    platform: Platform,
    http: Http,
    authenticator: Authenticator,
    settings: SettingsStore,
    total_memory_bytes: u64,
    emit: Emitter,
    work_area: WorkAreaProbe,
    busy: Busy,
    cancel_sign_in: Notify,
    pending_code: Mutex<Option<String>>,
    ticket: Mutex<Option<LaunchTicket>>,
    mismatched: Mutex<Option<MinecraftSession>>,
    prepared: Mutex<Option<Prepared>>,
    crash: Mutex<Option<Crash>>,
    sign_outs: AtomicU64,
    sign_in_url: Arc<Mutex<Option<String>>>,
}

impl LaunchFlow {
    pub fn new(
        config: LauncherConfig,
        browser: BrowserOpener,
        emit: Emitter,
        work_area: WorkAreaProbe,
    ) -> Self {
        let http = Http::new();
        let microsoft = MicrosoftClient::new(
            http.clone(),
            MicrosoftEndpoints::consumers(),
            CLIENT_ID.into(),
            browser,
        );
        let xbox = XboxClient::new(http.clone(), XboxEndpoints::default());
        let authenticator = Authenticator::new(microsoft, xbox, Box::new(KeyringAccountStore));
        let total_memory = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::nothing()
                .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
        )
        .map_or(0, |system| system.total_memory());
        let settings = SettingsStore::new(config.home.join("launcher.json"), total_memory);
        Self {
            config,
            platform: Platform::current(),
            http,
            authenticator,
            settings,
            total_memory_bytes: total_memory,
            emit,
            work_area,
            busy: Busy::default(),
            cancel_sign_in: Notify::new(),
            pending_code: Mutex::new(None),
            ticket: Mutex::new(None),
            mismatched: Mutex::new(None),
            prepared: Mutex::new(None),
            crash: Mutex::new(None),
            sign_outs: AtomicU64::new(0),
            sign_in_url: Arc::new(Mutex::new(None)),
        }
    }

    pub fn accept_uri(&self, uri: &str) -> bool {
        let Some(code) = ticket::ticket_code(uri) else {
            log::info!("Ignoring a launch link without a valid ticket");
            return false;
        };
        *self.pending_code.lock().unwrap() = Some(code);
        true
    }

    pub fn keep_ticket_for_restart(&self) {
        let Some(code) = self.pending_code.lock().unwrap().clone() else {
            return;
        };
        if let Err(error) = ticket::keep(&self.config.home, &code) {
            log::warn!("Could not keep the launch ticket across the update restart: {error}");
        }
    }

    pub fn forget_kept_ticket(&self) {
        ticket::forget(&self.config.home);
    }

    pub fn restore_kept_ticket(&self) -> bool {
        let Some(code) = ticket::take_kept(&self.config.home) else {
            return false;
        };
        log::info!("Restored the launch ticket kept across the update restart");
        *self.pending_code.lock().unwrap() = Some(code);
        true
    }

    // 업데이트도 이 자리를 잡는다. 실행 도중 업데이트가 재시작하면 받던 게임 파일과 로그인이 끊긴다
    pub fn hold(&self) -> Option<Held<'_>> {
        self.busy.hold()
    }

    pub async fn run(&self, step: Step) -> Option<GameProcess> {
        let Some(held) = self.busy.hold() else {
            log::info!("The launcher is busy; ignoring {step:?}");
            return None;
        };
        self.crash.lock().unwrap().take();
        let mut game = None;
        let sign_outs = self.sign_outs.load(Ordering::SeqCst);
        let event = match self.advance(step, sign_outs).await {
            Ok(Outcome::Ready(server)) => LaunchEvent::Ready { server },
            Ok(Outcome::Started(child)) => {
                game = Some(child);
                LaunchEvent::Launched
            }
            Err(AuthError::Mismatch {
                session,
                web_player_id,
            }) => {
                let web_name = self
                    .player_name(&web_player_id)
                    .await
                    .unwrap_or_else(|| "웹 계정".into());
                // 이름을 묻는 사이 로그아웃했으면 불일치 세션을 두지 않는다. 두면 "이 계정으로 시작" 이 로그아웃한 계정으로 켠다.
                // 잠금을 먼저 잡고 견준다. sign_out 은 세대를 올린 뒤 이 잠금을 기다려 비우므로, 견준 뒤의 로그아웃도 지운다
                let mut mismatched = self.mismatched.lock().unwrap();
                if self.signed_out_since(sign_outs) {
                    log::info!("Signed out during the account mismatch; dropping its session");
                    LaunchEvent::Failed {
                        error: LauncherError::new(Problem::LoginCancelled, SIGNED_OUT),
                    }
                } else {
                    let event = LaunchEvent::Mismatch {
                        web_player_id,
                        web_name,
                        session_player_id: session.player_id.clone(),
                        session_name: session.name.clone(),
                    };
                    *mismatched = Some(session);
                    event
                }
            }
            Err(AuthError::Failed(error)) => {
                log::warn!("Launch stopped with {:?}: {}", error.problem, error.message);
                self.crash.lock().unwrap().clone_from(&error.crash);
                LaunchEvent::Failed { error }
            }
        };
        drop(held);
        (self.emit)(event);
        game
    }

    pub fn cancel_sign_in(&self) {
        self.cancel_sign_in.notify_waiters();
    }

    pub fn reopen_sign_in(&self) -> Option<String> {
        self.sign_in_url.lock().unwrap().clone()
    }

    pub fn sign_out(&self) {
        self.sign_outs.fetch_add(1, Ordering::SeqCst);
        self.prepared.lock().unwrap().take();
        self.mismatched.lock().unwrap().take();
        self.authenticator.sign_out();
    }

    pub fn settings(&self) -> LauncherSettings {
        self.settings.load()
    }

    pub fn save_settings(&self, settings: &LauncherSettings) -> std::io::Result<()> {
        self.settings.save(settings)
    }

    pub fn web_base(&self) -> &str {
        &self.config.web_base
    }

    pub fn total_memory_bytes(&self) -> u64 {
        self.total_memory_bytes
    }

    // 크래시 화면의 보고서 열기와 정보 복사가 읽는 자리다. 화면은 경로를 넘기지 않는다
    pub fn record_crash(&self, crash: Crash) {
        *self.crash.lock().unwrap() = Some(crash);
    }

    pub fn crash(&self) -> Option<Crash> {
        self.crash.lock().unwrap().clone()
    }

    pub fn game_log(&self) -> PathBuf {
        self.log_folder().join(GAME_LOG)
    }

    pub fn log_folder(&self) -> PathBuf {
        GameLayout::new(self.game_directory()).logs()
    }

    pub fn game_directory(&self) -> PathBuf {
        self.settings.game_directory(&self.config.home)
    }

    async fn advance(&self, step: Step, sign_outs: u64) -> Result<Outcome, AuthError> {
        let kept = self.prepared.lock().unwrap().take();
        let prepared = match kept {
            Some(prepared) if step == Step::Play && prepared.at.elapsed() < PREPARED_FOR => {
                Ok(prepared)
            }
            kept => {
                if step == Step::Play && kept.is_some() {
                    log::info!("The prepared launch is over an hour old; preparing again");
                }
                self.prepare(step, sign_outs).await
            }
        };
        // 준비하는 동안 설정에서 로그아웃했으면 그 전에 받은 세션으로 켜지 않는다. 준비가 성공했든 실패했든 로그아웃 뒤에 끝난
        // 인증이 계정을 저장소에 다시 써 넣었을 수 있어 한 번 더 지운다. 그래야 다음 시작이 로그인부터 다시 한다
        if self.signed_out_since(sign_outs) {
            self.authenticator.sign_out();
            log::info!("Signed out while preparing; the next start signs in again");
            return match prepared {
                Ok(prepared) => Ok(Outcome::Ready(prepared.installation.server)),
                Err(AuthError::Mismatch { .. }) => {
                    Err(LauncherError::new(Problem::LoginCancelled, SIGNED_OUT).into())
                }
                Err(error) => Err(error),
            };
        }
        let prepared = prepared?;
        let server = prepared.installation.server.clone();
        if !step.starts_the_game() {
            *self.prepared.lock().unwrap() = Some(prepared);
            return Ok(Outcome::Ready(server));
        }
        self.start(prepared).await.map(Outcome::Started)
    }

    fn signed_out_since(&self, sign_outs: u64) -> bool {
        self.sign_outs.load(Ordering::SeqCst) != sign_outs
    }

    async fn prepare(&self, step: Step, sign_outs: u64) -> Result<Prepared, AuthError> {
        (self.emit)(LaunchEvent::Checking);
        let ticket = self.redeem().await;
        let accepted = if step == Step::ContinueAnyway {
            self.mismatched.lock().unwrap().take()
        } else {
            None
        };
        let session = match accepted {
            Some(session) => session,
            None => {
                let emit = self.emit.clone();
                let url = self.sign_in_url.clone();
                let waiting = move |sign_in: String| {
                    *url.lock().unwrap() = Some(sign_in);
                    emit(LaunchEvent::WaitingForSignIn);
                };
                let signing_in = async {
                    if step == Step::SwitchAccount {
                        self.authenticator
                            .switch_account(ticket.as_ref(), &waiting)
                            .await
                    } else {
                        self.authenticator
                            .authenticate(ticket.as_ref(), &waiting)
                            .await
                    }
                };
                tokio::select! {
                    session = signing_in => session?,
                    () = self.cancel_sign_in.notified() => {
                        return Err(LauncherError::new(Problem::LoginCancelled, "로그인을 취소했어요.").into());
                    }
                }
            }
        };
        if !self.signed_out_since(sign_outs) {
            (self.emit)(LaunchEvent::SignedIn {
                player_id: session.player_id.clone(),
                name: session.name.clone(),
            });
        }

        let pack = pack::load(&self.http, &self.config.pack_manifest).await?;
        (self.emit)(LaunchEvent::Preparing {
            minecraft_version: pack.minecraft_version.clone(),
            mods: pack.mods.len(),
            shaderpacks: pack.shaderpacks.len(),
        });
        let layout = GameLayout::new(self.game_directory());
        mark_game_root(&layout.root);
        let installer =
            GameInstaller::new(self.http.clone(), layout.clone(), self.platform.clone())
                .owning_folder(layout.root == self.config.home);
        let installation = installer
            .install(
                &pack,
                &FlowInstallListener {
                    emit: self.emit.clone(),
                },
            )
            .await?;
        Ok(Prepared {
            session,
            installation,
            layout,
            default_options: pack.default_options,
            at: Instant::now(),
        })
    }

    // 메모리와 options.txt 는 시작 버튼을 누른 때의 것을 쓴다. 기다리는 동안 설정에서 바꾼 메모리가 그대로 들어간다
    async fn start(&self, prepared: Prepared) -> Result<GameProcess, AuthError> {
        let Prepared {
            session,
            installation,
            layout,
            default_options,
            ..
        } = prepared;
        let prefer_vulkan = options::amd_display_adapter().await;
        options::prepare(
            &layout.options(),
            &default_options,
            (self.work_area)(),
            prefer_vulkan,
        )
        .await;

        (self.emit)(LaunchEvent::Launching {
            server: installation.server.clone(),
        });
        let options = LaunchOptions {
            memory_megabytes: self.settings.load().memory_megabytes,
            client_id: CLIENT_ID,
            launcher_version: env!("CARGO_PKG_VERSION"),
        };
        let command = launch::command(&installation, &session, &options, &self.platform);
        Ok(launch::start(
            &command,
            &installation.game_directory,
            layout.logs().join(GAME_LOG),
            WINDOW_TIMEOUT,
        )
        .await?)
    }

    // ticket 은 어느 계정으로 로그인할지 고르는 힌트일 뿐이라, 바꾸지 못해도 실행은 멈추지 않는다
    async fn redeem(&self) -> Option<LaunchTicket> {
        let code = self.pending_code.lock().unwrap().take();
        if let Some(code) = code {
            let redeemed = ticket::redeem(&self.http, &self.config.web_base, &code).await;
            if redeemed.is_none() {
                log::info!("Launch ticket was already used or expired; continuing without it");
            }
            *self.ticket.lock().unwrap() = redeemed;
        }
        self.ticket.lock().unwrap().clone()
    }

    async fn player_name(&self, player_id: &str) -> Option<String> {
        #[derive(Deserialize)]
        struct Profile {
            name: Option<String>,
        }
        let url = format!("{SESSION_PROFILE}{}", player_id.replace('-', ""));
        match self.http.get_json::<Profile>(&url).await {
            Ok(profile) => profile.name,
            Err(error) => {
                log::info!("Could not look up the name of {player_id}: {error}");
                None
            }
        }
    }
}

// 남의 .minecraft 를 그대로 게임 루트로 쓰면 manifest 에 없는 모드를 지우는 정리가 그 사람의 모드를 지운다.
// versions/ 는 공식 런처의 폴더에도 있어 런처가 쓰던 폴더인지 가르지 못한다
pub fn game_root(picked: &Path) -> PathBuf {
    if is_game_root(picked) || is_empty(picked) {
        picked.to_path_buf()
    } else {
        picked.join(GAME_FOLDER)
    }
}

fn is_game_root(directory: &Path) -> bool {
    directory.join(GAME_MARKER).exists() || directory.join("logs").join(GAME_LOG).exists()
}

fn is_empty(directory: &Path) -> bool {
    std::fs::read_dir(directory).map_or(true, |entries| {
        entries
            .flatten()
            .all(|entry| OS_CLUTTER.contains(&entry.file_name().to_string_lossy().as_ref()))
    })
}

fn mark_game_root(root: &Path) {
    let marked =
        std::fs::create_dir_all(root).and_then(|()| std::fs::write(root.join(GAME_MARKER), ""));
    if let Err(error) = marked {
        log::warn!(
            "Could not mark the game directory {}: {error}",
            root.display()
        );
    }
}

struct FlowInstallListener {
    emit: Emitter,
}

impl InstallListener for FlowInstallListener {
    fn downloading(&self, plan: DownloadPlan) {
        (self.emit)(LaunchEvent::Downloading(plan));
    }

    fn progress(&self) -> Progress {
        let emit = self.emit.clone();
        let last = Mutex::new(Instant::now() - PROGRESS_INTERVAL);
        Arc::new(move |done_bytes, total_bytes, file| {
            let mut last = last.lock().unwrap();
            if last.elapsed() < PROGRESS_INTERVAL && done_bytes < total_bytes {
                return;
            }
            *last = Instant::now();
            emit(LaunchEvent::Progress {
                done_bytes,
                total_bytes,
                file: file.to_string(),
            });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lets_one_holder_in_at_a_time_and_frees_the_place_when_it_ends() {
        let busy = Busy::default();
        let held = busy.hold();
        assert!(held.is_some());
        assert!(busy.hold().is_none());
        drop(held);
        assert!(busy.hold().is_some());
    }

    #[test]
    fn waits_for_the_play_button_unless_the_player_already_chose_to_start() {
        let step = |name: &str| serde_json::from_str::<Step>(&format!("\"{name}\"")).unwrap();
        assert!(!step("retry").starts_the_game());
        assert!(!step("switchAccount").starts_the_game());
        assert!(step("continueAnyway").starts_the_game());
        assert!(step("play").starts_the_game());
    }

    #[test]
    fn uses_an_empty_folder_as_it_is() {
        let picked = tempfile::tempdir().unwrap();
        std::fs::write(picked.path().join(".DS_Store"), "").unwrap();
        assert_eq!(game_root(picked.path()), picked.path());
    }

    #[test]
    fn puts_the_game_below_a_folder_the_launcher_did_not_make() {
        let picked = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(picked.path().join("versions")).unwrap();
        std::fs::create_dir_all(picked.path().join("mods")).unwrap();
        assert_eq!(game_root(picked.path()), picked.path().join(GAME_FOLDER));
    }

    #[test]
    fn keeps_a_folder_the_launcher_already_used() {
        let marked = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(marked.path().join("mods")).unwrap();
        mark_game_root(marked.path());
        assert_eq!(game_root(marked.path()), marked.path());

        let launched = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(launched.path().join("logs")).unwrap();
        std::fs::write(launched.path().join("logs").join(GAME_LOG), "").unwrap();
        assert_eq!(game_root(launched.path()), launched.path());
    }
}
