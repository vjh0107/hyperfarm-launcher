use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::loopback::parse_query;
use super::microsoft::{MicrosoftClient, MicrosoftEndpoints};
use super::store::{AccountStore, StoredAccount};
use super::xbox::{XboxClient, XboxEndpoints};
use super::{AuthError, Authenticator};
use crate::error::Problem;
use crate::http::Http;
use crate::ticket::LaunchTicket;

const STEVE: &str = "0f3b1b2e-6a3d-4b8e-9c1d-2e3f4a5b6c7d";
const ALEX: &str = "1a2b3c4d-1a2b-1a2b-1a2b-1a2b3c4d5e6f";

#[derive(Default)]
struct Accounts {
    stored: Option<StoredAccount>,
    deletions: usize,
}

#[derive(Clone, Default)]
struct MemoryAccountStore(Arc<Mutex<Accounts>>);

impl AccountStore for MemoryAccountStore {
    fn load(&self) -> Option<StoredAccount> {
        self.0.lock().unwrap().stored.clone()
    }

    fn save(&self, account: &StoredAccount) {
        self.0.lock().unwrap().stored = Some(account.clone());
    }

    fn delete(&self) {
        let mut accounts = self.0.lock().unwrap();
        accounts.stored = None;
        accounts.deletions += 1;
    }
}

#[derive(Default)]
struct BrowserScript {
    opened: Vec<HashMap<String, String>>,
    silent_error: Option<&'static str>,
    forged_state: Option<&'static str>,
}

struct Fixture {
    server: MockServer,
    accounts: MemoryAccountStore,
    browser: Arc<Mutex<BrowserScript>>,
}

impl Fixture {
    async fn new(profile: &str) -> Self {
        let server = MockServer::start().await;
        let json = |body: serde_json::Value| ResponseTemplate::new(200).set_body_json(body);
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(json(serde_json::json!({"access_token": "ms", "refresh_token": "rt-new"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .respond_with(json(serde_json::json!({"access_token": "ms", "refresh_token": "rt-browser"})))
            .mount(&server)
            .await;
        Mock::given(path("/xbl"))
            .respond_with(json(serde_json::json!({"Token": "xbl", "DisplayClaims": {"xui": [{"uhs": "hash"}]}})))
            .mount(&server)
            .await;
        Mock::given(path("/xsts"))
            .respond_with(json(serde_json::json!({"Token": "xsts", "DisplayClaims": {"xui": [{"uhs": "hash", "xid": "2535"}]}})))
            .mount(&server)
            .await;
        Mock::given(path("/login")).respond_with(json(serde_json::json!({"access_token": "mc"}))).mount(&server).await;
        Mock::given(path("/profile"))
            .respond_with(json(serde_json::json!({"id": profile.replace('-', ""), "name": "Steve"})))
            .mount(&server)
            .await;
        Self { server, accounts: MemoryAccountStore::default(), browser: Arc::default() }
    }

    fn authenticator(&self) -> Authenticator {
        let base = self.server.uri();
        let http = Http::new();
        let script = self.browser.clone();
        let browser = Arc::new(move |page: &str| {
            let query = parse_query(page.split_once('?').map_or("", |(_, query)| query));
            let mut script = script.lock().unwrap();
            script.opened.push(query.clone());
            let answer = match (query.get("prompt").map(String::as_str), script.silent_error) {
                (Some("none"), Some(error)) => format!("error={error}"),
                _ => "code=auth-code".into(),
            };
            let state = script.forged_state.map_or_else(|| query["state"].clone(), str::to_string);
            let redirect = format!("{}/?{answer}&state={state}", query["redirect_uri"]);
            tokio::spawn(async move {
                let _ = reqwest::get(redirect).await;
            });
            Ok(())
        });
        Authenticator::new(
            MicrosoftClient::new(
                http.clone(),
                MicrosoftEndpoints { authorize: format!("{base}/authorize"), token: format!("{base}/token") },
                "client".into(),
                browser,
            ),
            XboxClient::new(
                http,
                XboxEndpoints {
                    user_authenticate: format!("{base}/xbl"),
                    xsts_authorize: format!("{base}/xsts"),
                    minecraft_login: format!("{base}/login"),
                    minecraft_profile: format!("{base}/profile"),
                },
            ),
            Box::new(self.accounts.clone()),
        )
    }

    fn store(&self, player_id: &str, refresh_token: &str) {
        self.accounts.save(&StoredAccount { player_id: player_id.into(), refresh_token: refresh_token.into() });
    }

    fn stored(&self) -> Option<StoredAccount> {
        self.accounts.load()
    }

    fn deletions(&self) -> usize {
        self.accounts.0.lock().unwrap().deletions
    }

    fn opened(&self) -> Vec<HashMap<String, String>> {
        self.browser.lock().unwrap().opened.clone()
    }

    async fn refresh_requests(&self) -> usize {
        let requests = self.server.received_requests().await.unwrap_or_default();
        requests.iter().filter(|request| String::from_utf8_lossy(&request.body).contains("grant_type=refresh_token")).count()
    }
}

fn steve_ticket(login_hint: Option<&str>) -> LaunchTicket {
    LaunchTicket { player_id: STEVE.into(), login_hint: login_hint.map(Into::into) }
}

fn ignore(_: String) {}

fn stored(player_id: &str, refresh_token: &str) -> Option<StoredAccount> {
    Some(StoredAccount { player_id: player_id.into(), refresh_token: refresh_token.into() })
}

#[tokio::test]
async fn uses_the_stored_account_of_the_ticket_player_without_the_browser() {
    let fixture = Fixture::new(STEVE).await;
    fixture.store(STEVE, "rt-old");

    let Ok(session) = fixture.authenticator().authenticate(Some(&steve_ticket(Some("steve@example.com"))), &ignore).await else {
        panic!("sign-in failed");
    };

    assert_eq!(session.player_id, STEVE);
    assert_eq!(session.xuid, "2535");
    assert!(fixture.opened().is_empty());
    assert_eq!(fixture.refresh_requests().await, 1);
    assert_eq!(fixture.stored(), stored(STEVE, "rt-new"));
}

#[tokio::test]
async fn skips_a_stored_account_of_another_player_and_signs_in_silently_with_the_hint() {
    let fixture = Fixture::new(STEVE).await;
    fixture.store(ALEX, "rt-alex");

    assert!(fixture.authenticator().authenticate(Some(&steve_ticket(Some("steve@example.com"))), &ignore).await.is_ok());

    assert_eq!(fixture.refresh_requests().await, 0);
    let opened = fixture.opened();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].get("prompt").map(String::as_str), Some("none"));
    assert_eq!(opened[0].get("login_hint").map(String::as_str), Some("steve@example.com"));
    assert_eq!(opened[0].get("code_challenge_method").map(String::as_str), Some("S256"));
    assert_eq!(fixture.stored(), stored(STEVE, "rt-browser"));
}

#[tokio::test]
async fn opens_the_sign_in_window_with_the_same_hint_when_silent_sign_in_needs_the_player() {
    let fixture = Fixture::new(STEVE).await;
    fixture.browser.lock().unwrap().silent_error = Some("interaction_required");
    let waits = Arc::new(Mutex::new(0));
    let counter = waits.clone();
    let waiting = move |_: String| *counter.lock().unwrap() += 1;

    assert!(fixture.authenticator().authenticate(Some(&steve_ticket(Some("steve@example.com"))), &waiting).await.is_ok());

    let opened = fixture.opened();
    assert_eq!(opened.len(), 2);
    assert!(!opened[1].contains_key("prompt"));
    assert_eq!(opened[1].get("login_hint").map(String::as_str), Some("steve@example.com"));
    assert_eq!(*waits.lock().unwrap(), 1);
}

#[tokio::test]
async fn switching_accounts_drops_the_stored_account_and_asks_for_an_account_without_a_hint() {
    let fixture = Fixture::new(STEVE).await;
    fixture.store(STEVE, "rt-old");

    assert!(fixture.authenticator().switch_account(Some(&steve_ticket(Some("steve@example.com"))), &ignore).await.is_ok());

    assert_eq!(fixture.deletions(), 1);
    assert_eq!(fixture.refresh_requests().await, 0);
    let opened = fixture.opened();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].get("prompt").map(String::as_str), Some("select_account"));
    assert!(!opened[0].contains_key("login_hint"));
}

#[tokio::test]
async fn discards_an_expired_refresh_token_and_moves_to_the_browser() {
    let fixture = Fixture::new(STEVE).await;
    Mock::given(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({"error": "invalid_grant"})))
        .with_priority(1)
        .mount(&fixture.server)
        .await;
    fixture.store(STEVE, "rt-expired");

    assert!(fixture.authenticator().authenticate(None, &ignore).await.is_ok());

    assert_eq!(fixture.deletions(), 1);
    assert_eq!(fixture.opened().len(), 1);
    assert_eq!(fixture.stored(), stored(STEVE, "rt-browser"));
}

#[tokio::test]
async fn refuses_a_minecraft_account_other_than_the_web_account() {
    let fixture = Fixture::new(ALEX).await;

    let result = fixture.authenticator().authenticate(Some(&steve_ticket(None)), &ignore).await;

    assert!(matches!(result, Err(AuthError::Mismatch { ref web_player_id, .. }) if web_player_id == STEVE));
}

#[tokio::test]
async fn rejects_a_redirect_with_another_state() {
    let fixture = Fixture::new(STEVE).await;
    fixture.browser.lock().unwrap().forged_state = Some("forged");

    let result = fixture.authenticator().authenticate(None, &ignore).await;

    assert!(matches!(result, Err(AuthError::Failed(ref error)) if error.problem == Problem::LoginFailed));
    let token_requests = fixture.server.received_requests().await.unwrap_or_default().iter().filter(|r| r.url.path() == "/token").count();
    assert_eq!(token_requests, 0);
}

#[tokio::test]
async fn explains_a_missing_xbox_profile() {
    let fixture = Fixture::new(STEVE).await;
    Mock::given(path("/xsts"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"XErr": 2148916233u64})))
        .with_priority(1)
        .mount(&fixture.server)
        .await;

    let result = fixture.authenticator().authenticate(None, &ignore).await;

    assert!(matches!(result, Err(AuthError::Failed(ref error)) if error.problem == Problem::NoXboxProfile));
}
