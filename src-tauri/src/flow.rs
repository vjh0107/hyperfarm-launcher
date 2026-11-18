use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::auth::microsoft::{BrowserOpener, MicrosoftClient, MicrosoftEndpoints};
use crate::auth::store::KeyringAccountStore;
use crate::auth::xbox::{XboxClient, XboxEndpoints};
use crate::auth::{AuthError, Authenticator, MinecraftSession};
use crate::error::LauncherError;
use crate::http::Http;
use crate::install::download::Progress;
use crate::install::rules::Platform;
use crate::install::{DownloadPlan, GameInstaller, GameLayout, InstallListener, pack};
use crate::launch::{self, LaunchOptions};
use crate::settings::{CLIENT_ID, LauncherConfig, LauncherSettings, SettingsStore};
use crate::ticket::{self, LaunchTicket};

const WINDOW_TIMEOUT: Duration = Duration::from_secs(120);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const GAME_LOG: &str = "launcher-game.log";
const SESSION_PROFILE: &str = "https://sessionserver.mojang.com/session/minecraft/profile/";

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum LaunchEvent {
    Checking,
    WaitingForSignIn,
    #[serde(rename_all = "camelCase")]
    SignedIn { player_id: String, name: String },
    #[serde(rename_all = "camelCase")]
    Preparing { minecraft_version: String, mods: usize, shaderpacks: usize },
    Downloading(DownloadPlan),
    #[serde(rename_all = "camelCase")]
    Progress { done_bytes: u64, total_bytes: u64, file: String },
    Launching { server: String },
    Launched,
    Failed { error: LauncherError },
    #[serde(rename_all = "camelCase")]
    Mismatch { web_player_id: String, web_name: String, session_player_id: String, session_name: String },
}

pub type Emitter = Arc<dyn Fn(LaunchEvent) + Send + Sync>;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Retry,
    SwitchAccount,
    ContinueAnyway,
}

pub struct LaunchFlow {
    config: LauncherConfig,
    platform: Platform,
    http: Http,
    authenticator: Authenticator,
    settings: SettingsStore,
    total_memory_bytes: u64,
    emit: Emitter,
    busy: AtomicBool,
    pending_code: Mutex<Option<String>>,
    ticket: Mutex<Option<LaunchTicket>>,
    mismatched: Mutex<Option<MinecraftSession>>,
    sign_in_url: Arc<Mutex<Option<String>>>,
}

impl LaunchFlow {
    pub fn new(config: LauncherConfig, browser: BrowserOpener, emit: Emitter) -> Self {
        let http = Http::new();
        let microsoft = MicrosoftClient::new(http.clone(), MicrosoftEndpoints::consumers(), CLIENT_ID.into(), browser);
        let xbox = XboxClient::new(http.clone(), XboxEndpoints::default());
        let authenticator = Authenticator::new(microsoft, xbox, Box::new(KeyringAccountStore));
        let total_memory = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::nothing().with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
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
            busy: AtomicBool::new(false),
            pending_code: Mutex::new(None),
            ticket: Mutex::new(None),
            mismatched: Mutex::new(None),
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

    pub async fn run(&self, step: Step) {
        if self.busy.swap(true, Ordering::SeqCst) {
            log::info!("A launch is already running; ignoring {step:?}");
            return;
        }
        let event = match self.launch(step).await {
            Ok(()) => LaunchEvent::Launched,
            Err(AuthError::Mismatch { session, web_player_id }) => {
                let web_name = self.player_name(&web_player_id).await.unwrap_or_else(|| "웹 계정".into());
                let event = LaunchEvent::Mismatch {
                    web_player_id,
                    web_name,
                    session_player_id: session.player_id.clone(),
                    session_name: session.name.clone(),
                };
                *self.mismatched.lock().unwrap() = Some(session);
                event
            }
            Err(AuthError::Failed(error)) => {
                log::warn!("Launch stopped with {:?}: {}", error.problem, error.message);
                LaunchEvent::Failed { error }
            }
        };
        self.busy.store(false, Ordering::SeqCst);
        (self.emit)(event);
    }

    pub fn reopen_sign_in(&self) -> Option<String> {
        self.sign_in_url.lock().unwrap().clone()
    }

    pub fn sign_out(&self) {
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

    pub fn game_log(&self) -> PathBuf {
        GameLayout::new(self.game_directory()).logs().join(GAME_LOG)
    }

    pub fn game_directory(&self) -> PathBuf {
        self.settings.game_directory(&self.config.home)
    }

    async fn launch(&self, step: Step) -> Result<(), AuthError> {
        (self.emit)(LaunchEvent::Checking);
        let ticket = self.redeem().await;
        let accepted = if step == Step::ContinueAnyway { self.mismatched.lock().unwrap().take() } else { None };
        let session = match accepted {
            Some(session) => session,
            None => {
                let emit = self.emit.clone();
                let url = self.sign_in_url.clone();
                let waiting = move |sign_in: String| {
                    *url.lock().unwrap() = Some(sign_in);
                    emit(LaunchEvent::WaitingForSignIn);
                };
                if step == Step::SwitchAccount {
                    self.authenticator.switch_account(ticket.as_ref(), &waiting).await?
                } else {
                    self.authenticator.authenticate(ticket.as_ref(), &waiting).await?
                }
            }
        };
        (self.emit)(LaunchEvent::SignedIn { player_id: session.player_id.clone(), name: session.name.clone() });

        let pack = pack::load(&self.http, &self.config.pack_manifest).await?;
        (self.emit)(LaunchEvent::Preparing {
            minecraft_version: pack.minecraft_version.clone(),
            mods: pack.mods.len(),
            shaderpacks: pack.shaderpacks.len(),
        });
        let layout = GameLayout::new(self.game_directory());
        let installer = GameInstaller::new(self.http.clone(), layout.clone(), self.platform.clone());
        let installation = installer.install(&pack, &FlowInstallListener { emit: self.emit.clone() }).await?;

        (self.emit)(LaunchEvent::Launching { server: installation.server.clone() });
        let options = LaunchOptions {
            memory_megabytes: self.settings.load().memory_megabytes,
            client_id: CLIENT_ID,
            launcher_version: env!("CARGO_PKG_VERSION"),
        };
        let command = launch::command(&installation, &session, &options, &self.platform);
        launch::start(&command, &installation.game_directory, layout.logs().join(GAME_LOG), WINDOW_TIMEOUT).await?;
        Ok(())
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
            emit(LaunchEvent::Progress { done_bytes, total_bytes, file: file.to_string() });
        })
    }
}
