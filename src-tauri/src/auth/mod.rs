mod loopback;
pub mod microsoft;
pub mod store;
pub mod xbox;

use std::time::Duration;

use crate::error::{LauncherError, Problem};
use crate::ticket::LaunchTicket;
use microsoft::{MicrosoftClient, MicrosoftTokens, Prompt, RefreshError, SignInError};
use store::{AccountStore, StoredAccount};
pub use xbox::MinecraftSession;
use xbox::XboxClient;

const SILENT_TIMEOUT: Duration = Duration::from_secs(20);
const INTERACTIVE_TIMEOUT: Duration = Duration::from_secs(300);

pub type Waiting = dyn Fn(String) + Send + Sync;

pub enum AuthError {
    Failed(LauncherError),
    Mismatch { session: MinecraftSession, web_player_id: String },
}

impl From<LauncherError> for AuthError {
    fn from(error: LauncherError) -> Self {
        AuthError::Failed(error)
    }
}

pub struct Authenticator {
    microsoft: MicrosoftClient,
    xbox: XboxClient,
    accounts: Box<dyn AccountStore>,
}

impl Authenticator {
    pub fn new(microsoft: MicrosoftClient, xbox: XboxClient, accounts: Box<dyn AccountStore>) -> Self {
        Self { microsoft, xbox, accounts }
    }

    pub async fn authenticate(&self, ticket: Option<&LaunchTicket>, waiting: &Waiting) -> Result<MinecraftSession, AuthError> {
        let session = match self.from_stored_account(ticket).await? {
            Some(session) => session,
            None => self.from_browser(ticket, waiting).await?,
        };
        checked(session, ticket)
    }

    pub async fn switch_account(&self, ticket: Option<&LaunchTicket>, waiting: &Waiting) -> Result<MinecraftSession, AuthError> {
        self.accounts.delete();
        let tokens = self.interactive(Prompt::SelectAccount, None, waiting).await?;
        checked(self.complete(tokens, None).await?, ticket)
    }

    pub fn sign_out(&self) {
        self.accounts.delete();
    }

    async fn from_stored_account(&self, ticket: Option<&LaunchTicket>) -> Result<Option<MinecraftSession>, LauncherError> {
        let Some(stored) = self.accounts.load() else { return Ok(None) };
        if ticket.is_some_and(|ticket| !same_player(&ticket.player_id, &stored.player_id)) {
            return Ok(None);
        }
        match self.microsoft.refresh(&stored.refresh_token).await {
            Ok(tokens) => Ok(Some(self.complete(tokens, Some(&stored.refresh_token)).await?)),
            Err(RefreshError::InvalidGrant) => {
                log::info!("Stored Microsoft sign-in expired; signing in through the browser");
                self.accounts.delete();
                Ok(None)
            }
            Err(RefreshError::Failed(error)) => Err(error),
        }
    }

    async fn from_browser(&self, ticket: Option<&LaunchTicket>, waiting: &Waiting) -> Result<MinecraftSession, LauncherError> {
        let login_hint = ticket.and_then(|ticket| ticket.login_hint.as_deref());
        let tokens = match self.microsoft.sign_in(Prompt::None, login_hint, SILENT_TIMEOUT, None).await {
            Ok(tokens) => tokens,
            Err(SignInError::InteractionRequired(reason)) => {
                log::info!("Silent sign-in needs the player: {reason}");
                let prompt = if login_hint.is_some() { Prompt::IfNeeded } else { Prompt::SelectAccount };
                self.interactive(prompt, login_hint, waiting).await?
            }
            Err(SignInError::Failed(error)) => return Err(error),
        };
        self.complete(tokens, None).await
    }

    async fn interactive(&self, prompt: Prompt, login_hint: Option<&str>, waiting: &Waiting) -> Result<MicrosoftTokens, LauncherError> {
        match self.microsoft.sign_in(prompt, login_hint, INTERACTIVE_TIMEOUT, Some(waiting)).await {
            Ok(tokens) => Ok(tokens),
            Err(SignInError::InteractionRequired(reason)) => {
                Err(LauncherError::new(Problem::LoginFailed, format!("Microsoft 로그인에 실패했어요. ({reason})")))
            }
            Err(SignInError::Failed(error)) => Err(error),
        }
    }

    async fn complete(&self, tokens: MicrosoftTokens, previous_refresh: Option<&str>) -> Result<MinecraftSession, LauncherError> {
        let session = self.xbox.authenticate(&tokens.access_token).await?;
        if let Some(refresh_token) = tokens.refresh_token.as_deref().or(previous_refresh) {
            self.accounts.save(&StoredAccount { player_id: session.player_id.clone(), refresh_token: refresh_token.to_string() });
        }
        Ok(session)
    }
}

fn checked(session: MinecraftSession, ticket: Option<&LaunchTicket>) -> Result<MinecraftSession, AuthError> {
    match ticket {
        Some(ticket) if !same_player(&ticket.player_id, &session.player_id) => {
            Err(AuthError::Mismatch { session, web_player_id: ticket.player_id.clone() })
        }
        _ => Ok(session),
    }
}

fn same_player(left: &str, right: &str) -> bool {
    left.replace('-', "").eq_ignore_ascii_case(&right.replace('-', ""))
}

#[cfg(test)]
mod tests;
