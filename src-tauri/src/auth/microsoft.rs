use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::loopback::LoopbackReceiver;
use crate::error::{LauncherError, Problem};
use crate::http::{Http, HttpError};

const SCOPE: &str = "XboxLive.signin offline_access";
pub(super) const INTERACTION_ERRORS: [&str; 4] = [
    "interaction_required",
    "login_required",
    "consent_required",
    "account_selection_required",
];

#[derive(Clone)]
pub struct MicrosoftEndpoints {
    pub authorize: String,
    pub token: String,
}

impl MicrosoftEndpoints {
    // Xbox Live 의 RpsTicket 은 개인 계정 테넌트에서 XboxLive.signin 을 받은 토큰만 받는다. 빠지면 401 이다
    pub fn consumers() -> Self {
        Self {
            authorize: "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize".into(),
            token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prompt {
    None,
    IfNeeded,
    SelectAccount,
}

impl Prompt {
    fn parameter(self) -> Option<&'static str> {
        match self {
            Prompt::None => Some("none"),
            Prompt::IfNeeded => None,
            Prompt::SelectAccount => Some("select_account"),
        }
    }
}

#[derive(Deserialize)]
pub struct MicrosoftTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

pub enum SignInError {
    InteractionRequired(String),
    Failed(LauncherError),
}

impl From<LauncherError> for SignInError {
    fn from(error: LauncherError) -> Self {
        SignInError::Failed(error)
    }
}

pub enum RefreshError {
    InvalidGrant,
    Failed(LauncherError),
}

pub type BrowserOpener = Arc<dyn Fn(&str) -> std::io::Result<()> + Send + Sync>;

pub struct MicrosoftClient {
    http: Http,
    endpoints: MicrosoftEndpoints,
    client_id: String,
    browser: BrowserOpener,
}

impl MicrosoftClient {
    pub fn new(
        http: Http,
        endpoints: MicrosoftEndpoints,
        client_id: String,
        browser: BrowserOpener,
    ) -> Self {
        Self {
            http,
            endpoints,
            client_id,
            browser,
        }
    }

    pub async fn sign_in(
        &self,
        prompt: Prompt,
        login_hint: Option<&str>,
        timeout: Duration,
        on_waiting: Option<&(dyn Fn(String) + Send + Sync)>,
    ) -> Result<MicrosoftTokens, SignInError> {
        let verifier = random_token();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_token();
        let receiver = LoopbackReceiver::bind()
            .await
            .map_err(|_| LauncherError::new(Problem::LoginFailed, "로그인 창을 열지 못했어요."))?;
        let redirect_uri = receiver.redirect_uri();
        let page = self.authorize_url(prompt, login_hint, &challenge, &state, &redirect_uri);
        (self.browser)(&page)
            .map_err(|_| LauncherError::new(Problem::LoginFailed, "브라우저를 열지 못했어요."))?;
        if let Some(waiting) = on_waiting {
            waiting(page.clone());
        }
        let Some(response) = receiver.await_redirect(timeout).await else {
            if prompt == Prompt::None {
                return Err(SignInError::InteractionRequired("timeout".into()));
            }
            return Err(LauncherError::new(
                Problem::LoginCancelled,
                "로그인 시간이 지났어요. 다시 시도해 주세요.",
            )
            .into());
        };
        if response.get("state") != Some(&state) {
            return Err(LauncherError::new(
                Problem::LoginFailed,
                "로그인 응답이 올바르지 않아요. 다시 시도해 주세요.",
            )
            .into());
        }
        if let Some(error) = response.get("error") {
            if INTERACTION_ERRORS.contains(&error.as_str()) {
                return Err(SignInError::InteractionRequired(error.clone()));
            }
            if error == "access_denied" {
                return Err(
                    LauncherError::new(Problem::LoginCancelled, "로그인을 취소했어요.").into(),
                );
            }
            return Err(LauncherError::new(
                Problem::LoginFailed,
                format!("Microsoft 로그인에 실패했어요. ({error})"),
            )
            .into());
        }
        let Some(code) = response.get("code").filter(|code| !code.is_empty()) else {
            return Err(LauncherError::new(
                Problem::LoginFailed,
                "로그인 응답에 인증 코드가 없어요.",
            )
            .into());
        };
        Ok(self
            .token(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &redirect_uri),
                ("code_verifier", &verifier),
            ])
            .await
            .map_err(LauncherError::from)?)
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<MicrosoftTokens, RefreshError> {
        match self
            .token(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .await
        {
            Ok(tokens) => Ok(tokens),
            Err(error) if error.status() == Some(400) && error.body().contains("invalid_grant") => {
                Err(RefreshError::InvalidGrant)
            }
            Err(error) => Err(RefreshError::Failed(error.into())),
        }
    }

    pub fn authorize_url(
        &self,
        prompt: Prompt,
        login_hint: Option<&str>,
        challenge: &str,
        state: &str,
        redirect_uri: &str,
    ) -> String {
        let mut url = url::Url::parse(&self.endpoints.authorize)
            .expect("the authorize endpoint is a valid URL");
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("client_id", &self.client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("response_mode", "query")
                .append_pair("scope", SCOPE)
                .append_pair("state", state)
                .append_pair("code_challenge", challenge)
                .append_pair("code_challenge_method", "S256");
            if let Some(parameter) = prompt.parameter() {
                query.append_pair("prompt", parameter);
            }
            if let Some(hint) = login_hint {
                query.append_pair("login_hint", hint);
            }
        }
        url.into()
    }

    async fn token(&self, grant: &[(&str, &str)]) -> Result<MicrosoftTokens, HttpError> {
        let mut form = grant.to_vec();
        form.push(("client_id", &self.client_id));
        form.push(("scope", SCOPE));
        self.http.post_form(&self.endpoints.token, &form).await
    }
}

fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
