use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;

const PLAYER_ID: &str = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
const NONCE: &str = "0123456789abcdef0123456789abcdef01234567";
const CHALLENGE: &str = "/api/launcher/crash-reports/challenge";
const REPORTS: &str = "/api/launcher/crash-reports";
const JOIN: &str = "/session/minecraft/join";

fn session(token: &str) -> MinecraftSession {
    MinecraftSession {
        access_token: token.into(),
        player_id: PLAYER_ID.into(),
        name: "Steve_kr".into(),
        xuid: "2535428504476914".into(),
    }
}

fn report() -> CrashReport {
    let game = tempfile::tempdir().unwrap();
    let crash = Crash::detect(
        Some(1),
        game.path(),
        &game.path().join("game.log"),
        SystemTime::now(),
    );
    CrashReport::new(&crash, "26.3", Some("  리소스팩 99%에서 꺼져요 "))
}

fn endpoints(server: &MockServer) -> SupportEndpoints {
    SupportEndpoints {
        web_base: server.uri(),
        session_join: format!("{}{JOIN}", server.uri()),
        retry_delay: Duration::ZERO,
    }
}

fn refusal(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({ "code": code, "message": "x" }))
}

fn filed(status: u16, outcome: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({ "outcome": outcome, "ticketId": 42 }))
}

async fn serve(server: &MockServer, at: &str, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(at))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn serve_once(server: &MockServer, at: &str, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(at))
        .respond_with(response)
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
}

async fn answering(reports: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    serve(
        &server,
        CHALLENGE,
        ResponseTemplate::new(201).set_body_json(json!({ "nonce": NONCE, "expiresInSeconds": 60 })),
    )
    .await;
    serve(&server, JOIN, ResponseTemplate::new(204)).await;
    serve(&server, REPORTS, reports).await;
    server
}

async fn bodies(server: &MockServer, at: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request: &&Request| request.url.path() == at)
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect()
}

async fn file_with(server: &MockServer, sessions: &AtomicUsize) -> Result<Filed, ReportError> {
    file(&Http::new(), &endpoints(server), &report(), || async {
        let issued = sessions.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(session(&format!("token-{issued}")))
    })
    .await
}

#[tokio::test]
async fn proves_the_account_through_the_session_server_before_filing_the_report() {
    let server = answering(filed(201, "OPENED")).await;
    let sessions = AtomicUsize::new(0);

    let filed = file_with(&server, &sessions).await.unwrap();

    assert_eq!(
        filed,
        Filed {
            outcome: Outcome::Opened,
            ticket_id: 42
        }
    );
    assert_eq!(
        bodies(&server, CHALLENGE).await,
        [json!({ "name": "Steve_kr" })]
    );
    assert_eq!(
        bodies(&server, JOIN).await,
        [json!({
            "accessToken": "token-1",
            "selectedProfile": "069a79f444e94726a5befca90e38aaf5",
            "serverId": NONCE,
        })]
    );
    let sent = bodies(&server, REPORTS).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["name"], "Steve_kr");
    assert_eq!(sent[0]["nonce"], NONCE);
    let report = &sent[0]["report"];
    assert!(
        report["eventId"]
            .as_str()
            .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())),
        "{report}"
    );
    assert_eq!(report["minecraftVersion"], "26.3");
    assert_eq!(report["launcherVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["exitCode"], 1);
    assert_eq!(report["summary"], "보고서 없음");
    assert_eq!(report["note"], "리소스팩 99%에서 꺼져요");
    assert!(!sent[0].to_string().contains("token-1"));
}

#[tokio::test]
async fn reads_a_report_the_server_already_had_as_replayed() {
    let server = answering(filed(200, "REPLAYED")).await;

    let filed = file_with(&server, &AtomicUsize::new(0)).await.unwrap();

    assert_eq!(filed.outcome, Outcome::Replayed);
}

#[tokio::test]
async fn asks_for_a_new_challenge_once_when_the_first_one_was_spent() {
    let server = answering(filed(201, "APPENDED")).await;
    serve_once(&server, REPORTS, refusal(401, "CHALLENGE_INVALID")).await;
    let sessions = AtomicUsize::new(0);

    let filed = file_with(&server, &sessions).await.unwrap();

    assert_eq!(filed.outcome, Outcome::Appended);
    assert_eq!(bodies(&server, CHALLENGE).await.len(), 2);
    let sent = bodies(&server, REPORTS).await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["report"]["eventId"], sent[1]["report"]["eventId"]);
    assert_eq!(sessions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn joins_again_with_a_fresh_session_when_the_server_could_not_verify_it() {
    let server = answering(filed(201, "OPENED")).await;
    serve_once(&server, REPORTS, refusal(401, "NOT_VERIFIED")).await;
    let sessions = AtomicUsize::new(0);

    file_with(&server, &sessions).await.unwrap();

    assert_eq!(sessions.load(Ordering::SeqCst), 2);
    let tokens: Vec<Value> = bodies(&server, JOIN)
        .await
        .iter()
        .map(|body| body["accessToken"].clone())
        .collect();
    assert_eq!(tokens, [json!("token-1"), json!("token-2")]);
}

#[tokio::test]
async fn tries_once_more_when_verification_is_down_and_then_gives_up() {
    let server = answering(refusal(503, "VERIFICATION_UNAVAILABLE")).await;

    let error = file_with(&server, &AtomicUsize::new(0)).await.unwrap_err();

    assert_eq!(error.problem, ReportProblem::Unavailable);
    assert_eq!(bodies(&server, REPORTS).await.len(), 2);

    let recovering = answering(filed(201, "OPENED")).await;
    serve_once(&recovering, REPORTS, ResponseTemplate::new(502)).await;
    let filed = file_with(&recovering, &AtomicUsize::new(0)).await.unwrap();
    assert_eq!(filed.outcome, Outcome::Opened);
}

#[tokio::test]
async fn stops_after_one_retry_when_the_challenge_keeps_failing() {
    let server = answering(refusal(401, "CHALLENGE_INVALID")).await;

    let error = file_with(&server, &AtomicUsize::new(0)).await.unwrap_err();

    assert_eq!(error.problem, ReportProblem::Rejected);
    assert_eq!(bodies(&server, REPORTS).await.len(), 2);
}

#[tokio::test]
async fn turns_each_refusal_into_its_own_reason_without_retrying() {
    let cases = [
        (refusal(409, "EVENT_ID_TAKEN"), ReportProblem::EventTaken),
        (refusal(429, "RATE_LIMITED"), ReportProblem::RateLimited),
        (refusal(400, "INVALID"), ReportProblem::Rejected),
        (refusal(401, "SOMETHING_ELSE"), ReportProblem::Unavailable),
        (ResponseTemplate::new(500), ReportProblem::Unavailable),
    ];
    for (response, problem) in cases {
        let server = answering(response).await;
        let error = file_with(&server, &AtomicUsize::new(0)).await.unwrap_err();
        assert_eq!(error.problem, problem);
        assert_eq!(bodies(&server, REPORTS).await.len(), 1, "{problem:?}");
    }
}

#[tokio::test]
async fn reports_that_it_cannot_file_while_the_server_does_not_know_the_path() {
    let server = MockServer::start().await;

    let error = file_with(&server, &AtomicUsize::new(0)).await.unwrap_err();

    assert_eq!(error.problem, ReportProblem::Unavailable);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn tells_how_long_to_wait_when_the_server_is_rate_limiting() {
    let server = answering(filed(201, "OPENED")).await;
    serve_once(
        &server,
        CHALLENGE,
        refusal(429, "RATE_LIMITED").insert_header("Retry-After", "90"),
    )
    .await;

    let error = file_with(&server, &AtomicUsize::new(0)).await.unwrap_err();

    assert_eq!(error.problem, ReportProblem::RateLimited);
    assert_eq!(error.message, "2분 뒤에 다시 보내 주세요.");
    assert!(bodies(&server, JOIN).await.is_empty());
}

#[tokio::test]
async fn proves_the_account_again_when_the_session_server_refuses_the_token() {
    let server = answering(filed(201, "OPENED")).await;
    serve_once(&server, JOIN, ResponseTemplate::new(403)).await;
    let sessions = AtomicUsize::new(0);

    file_with(&server, &sessions).await.unwrap();

    assert_eq!(sessions.load(Ordering::SeqCst), 2);
    assert_eq!(bodies(&server, REPORTS).await.len(), 1);
}

#[tokio::test]
async fn says_the_connection_failed_when_the_server_cannot_be_reached() {
    let unreachable = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let endpoints = SupportEndpoints {
        web_base: unreachable.clone(),
        session_join: unreachable,
        retry_delay: Duration::ZERO,
    };

    let error = file(&Http::new(), &endpoints, &report(), || async {
        Ok(session("token"))
    })
    .await
    .unwrap_err();

    assert_eq!(error.problem, ReportProblem::Network);
}

#[test]
fn reports_only_as_the_account_that_played() {
    let played = crashed_player_session(Ok(Some(session("t"))), &PLAYER_ID.replace('-', ""));
    assert_eq!(played.unwrap().player_id, PLAYER_ID);

    let other = crashed_player_session(
        Ok(Some(session("t"))),
        "1a2b3c4d-1a2b-1a2b-1a2b-1a2b3c4d5e6f",
    );
    assert_eq!(other.unwrap_err().problem, ReportProblem::AccountChanged);

    let signed_out = crashed_player_session(Ok(None), PLAYER_ID);
    assert_eq!(
        signed_out.unwrap_err().problem,
        ReportProblem::LoginRequired
    );

    let offline = crashed_player_session(Err(LauncherError::network()), PLAYER_ID);
    assert_eq!(offline.unwrap_err().problem, ReportProblem::Network);
}

#[test]
fn cuts_long_fields_where_the_server_counts_utf16_units() {
    assert_eq!(limit("가나다", 2), "가나");
    assert_eq!(limit("a😀b", 2), "a");
    assert_eq!(limit("a😀b", 3), "a😀");

    let game = tempfile::tempdir().unwrap();
    let crash = Crash::detect(
        None,
        game.path(),
        &game.path().join("game.log"),
        SystemTime::now(),
    );
    let note = "가".repeat(NOTE_LIMIT + 10);
    let report = CrashReport::new(&crash, "26.3", Some(&note));
    assert_eq!(report.note.unwrap().chars().count(), NOTE_LIMIT);
    assert_eq!(CrashReport::new(&crash, "26.3", Some("  \n")).note, None);
}
