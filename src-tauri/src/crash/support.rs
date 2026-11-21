use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Crash, operating_system, redact_home};
use crate::auth::{MinecraftSession, same_player};
use crate::error::{LauncherError, Problem};
use crate::http::Http;

const SESSION_JOIN: &str = "https://sessionserver.mojang.com/session/minecraft/join";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_secs(2);
// web-backend 의 CrashReportRequest 가 @Size 로 막는 길이다. Java 는 UTF-16 단위로 세므로 같은 단위로 자른다
const VERSION_LIMIT: usize = 32;
const OS_LIMIT: usize = 64;
const SUMMARY_LIMIT: usize = 1000;
const NOTE_LIMIT: usize = 500;

pub struct SupportEndpoints {
    pub web_base: String,
    pub session_join: String,
    pub retry_delay: Duration,
}

impl SupportEndpoints {
    pub fn new(web_base: &str) -> Self {
        Self {
            web_base: web_base.to_string(),
            session_join: SESSION_JOIN.into(),
            retry_delay: RETRY_DELAY,
        }
    }

    fn challenge(&self) -> String {
        format!("{}/api/launcher/crash-reports/challenge", self.web_base)
    }

    fn reports(&self) -> String {
        format!("{}/api/launcher/crash-reports", self.web_base)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CrashReport {
    event_id: String,
    launcher_version: String,
    minecraft_version: String,
    os: String,
    exit_code: Option<i32>,
    summary: String,
    note: Option<String>,
}

impl CrashReport {
    pub fn new(crash: &Crash, minecraft_version: &str, note: Option<&str>) -> Self {
        let mut lines = Vec::new();
        match &crash.report {
            Some(report) => {
                lines.push(format!("보고서: {}", report.file_name()));
                lines.extend(report.summary());
            }
            None => lines.push("보고서 없음".into()),
        }
        let summary = redact_home(&lines.join("\n"), dirs::home_dir().as_deref());
        Self {
            event_id: crash.event_id.clone(),
            launcher_version: limit(env!("CARGO_PKG_VERSION"), VERSION_LIMIT),
            minecraft_version: limit(minecraft_version, VERSION_LIMIT),
            os: limit(&operating_system(), OS_LIMIT),
            exit_code: crash.code,
            summary: limit(&summary, SUMMARY_LIMIT),
            note: note
                .map(str::trim)
                .filter(|note| !note.is_empty())
                .map(|note| limit(note, NOTE_LIMIT)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    Opened,
    Appended,
    Replayed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filed {
    pub outcome: Outcome,
    pub ticket_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReportProblem {
    NoCrash,
    LoginRequired,
    AccountChanged,
    RateLimited,
    EventTaken,
    Unavailable,
    Network,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct ReportError {
    pub problem: ReportProblem,
    pub message: String,
}

impl ReportError {
    pub fn new(problem: ReportProblem) -> Self {
        let message = match problem {
            ReportProblem::NoCrash => "보고할 크래시가 없어요.",
            ReportProblem::LoginRequired => "로그인이 필요해요.",
            ReportProblem::AccountChanged => "게임을 켠 계정으로 로그인돼 있지 않아요.",
            ReportProblem::RateLimited => "잠시 뒤에 다시 보내 주세요.",
            ReportProblem::EventTaken => "다른 계정으로 이미 보고한 크래시예요.",
            ReportProblem::Unavailable => "지금은 보고할 수 없어요.",
            ReportProblem::Network => "인터넷 연결을 확인해 주세요.",
            ReportProblem::Rejected => "보고하지 못했어요.",
        };
        Self {
            problem,
            message: message.into(),
        }
    }

    fn rate_limited(retry_after: Option<u64>) -> Self {
        let message = match retry_after {
            Some(seconds @ 1..60) => format!("{seconds}초 뒤에 다시 보내 주세요."),
            Some(seconds @ 60..) => format!("{}분 뒤에 다시 보내 주세요.", seconds.div_ceil(60)),
            _ => return Self::new(ReportProblem::RateLimited),
        };
        Self {
            problem: ReportProblem::RateLimited,
            message,
        }
    }
}

// 문의는 크래시를 낸 계정에 붙는다. 그 계정의 Minecraft 토큰으로만 sessionserver 에 본인임을 보일 수 있고, 다른 계정으로
// 보내면 답이 크래시를 겪지 않은 계정의 문의함에 간다. 웹과 다른 계정으로 "이 계정으로 시작" 한 크래시도 게임을 켠 계정이다
pub fn crashed_player_session(
    stored: Result<Option<MinecraftSession>, LauncherError>,
    crashed_player_id: &str,
) -> Result<MinecraftSession, ReportError> {
    match stored {
        Ok(Some(session)) if same_player(&session.player_id, crashed_player_id) => Ok(session),
        Ok(Some(session)) => {
            log::warn!(
                "Not reporting the crash; signed in as {} but the game ran as {crashed_player_id}",
                session.player_id
            );
            Err(ReportError::new(ReportProblem::AccountChanged))
        }
        Ok(None) => {
            log::info!("Not reporting the crash; no stored sign-in to refresh");
            Err(ReportError::new(ReportProblem::LoginRequired))
        }
        Err(error) if error.problem == Problem::Network => {
            Err(ReportError::new(ReportProblem::Network))
        }
        Err(error) => {
            log::warn!(
                "Not reporting the crash; could not refresh the sign-in: {:?}",
                error.problem
            );
            Err(ReportError::new(ReportProblem::LoginRequired))
        }
    }
}

enum Retry {
    Challenge,
    Session,
    Later,
}

enum Failure {
    Retry(Retry, ReportError),
    Final(ReportError),
}

impl Failure {
    fn retry(retry: Retry, problem: ReportProblem) -> Self {
        Failure::Retry(retry, ReportError::new(problem))
    }

    fn fail(problem: ReportProblem) -> Self {
        Failure::Final(ReportError::new(problem))
    }
}

// 같은 eventId 로 다시 보내면 서버가 이미 들어간 보고를 REPLAYED 로 돌려주므로, 응답을 모른 채 다시 보내도 문의는 하나다
pub async fn file<S, F>(
    http: &Http,
    endpoints: &SupportEndpoints,
    report: &CrashReport,
    mut session: S,
) -> Result<Filed, ReportError>
where
    S: FnMut() -> F,
    F: Future<Output = Result<MinecraftSession, ReportError>>,
{
    let id = &report.event_id;
    let mut current = session().await?;
    let mut retried = false;
    loop {
        let error = match attempt(http, endpoints, report, &current).await {
            Ok(filed) => {
                log::info!(
                    "Reported crash {id} as ticket {} ({:?})",
                    filed.ticket_id,
                    filed.outcome
                );
                return Ok(filed);
            }
            Err(Failure::Retry(retry, error)) if !retried => {
                retried = true;
                match retry {
                    Retry::Challenge => {}
                    Retry::Session => current = session().await?,
                    Retry::Later => tokio::time::sleep(endpoints.retry_delay).await,
                }
                log::info!("Reporting crash {id} again after {:?}", error.problem);
                continue;
            }
            Err(Failure::Retry(_, error) | Failure::Final(error)) => error,
        };
        log::warn!("Could not report crash {id}: {:?}", error.problem);
        return Err(error);
    }
}

#[derive(Deserialize)]
struct Challenge {
    nonce: String,
}

#[derive(Deserialize)]
struct Refusal {
    code: Option<String>,
}

async fn attempt(
    http: &Http,
    endpoints: &SupportEndpoints,
    report: &CrashReport,
    session: &MinecraftSession,
) -> Result<Filed, Failure> {
    let answer = post(
        http,
        &endpoints.challenge(),
        &json!({ "name": session.name }),
    )
    .await?;
    let nonce = match answer.status {
        200..=299 => answer.json::<Challenge>()?.nonce,
        429 => {
            return Err(Failure::Final(ReportError::rate_limited(
                answer.retry_after,
            )));
        }
        502 | 503 => return Err(Failure::retry(Retry::Later, ReportProblem::Unavailable)),
        _ => return Err(answer.unexpected("challenge")),
    };

    let answer = post(
        http,
        &endpoints.session_join,
        &json!({
            "accessToken": session.access_token,
            "selectedProfile": session.player_id.replace('-', ""),
            "serverId": nonce,
        }),
    )
    .await?;
    match answer.status {
        200..=299 => {}
        401 | 403 => return Err(Failure::retry(Retry::Session, ReportProblem::Rejected)),
        429 => {
            return Err(Failure::Final(ReportError::rate_limited(
                answer.retry_after,
            )));
        }
        _ => return Err(answer.unexpected("session join")),
    }

    let answer = post(
        http,
        &endpoints.reports(),
        &json!({ "name": session.name, "nonce": nonce, "report": report }),
    )
    .await?;
    match (answer.status, answer.code().as_deref()) {
        (200..=299, _) => answer.json::<Filed>(),
        (401, Some("CHALLENGE_INVALID")) => {
            Err(Failure::retry(Retry::Challenge, ReportProblem::Rejected))
        }
        (401, Some("NOT_VERIFIED")) => Err(Failure::retry(Retry::Session, ReportProblem::Rejected)),
        (409, Some("EVENT_ID_TAKEN")) => Err(Failure::fail(ReportProblem::EventTaken)),
        (429, _) => Err(Failure::Final(ReportError::rate_limited(
            answer.retry_after,
        ))),
        (502 | 503, _) => Err(Failure::retry(Retry::Later, ReportProblem::Unavailable)),
        (400, _) => {
            log::warn!("The crash report was refused: {}", answer.body);
            Err(Failure::fail(ReportProblem::Rejected))
        }
        _ => Err(answer.unexpected("crash report")),
    }
}

struct Answer {
    status: u16,
    retry_after: Option<u64>,
    body: String,
}

impl Answer {
    fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, Failure> {
        serde_json::from_str(&self.body).map_err(|error| {
            log::warn!("The crash report server sent unreadable JSON: {error}");
            Failure::fail(ReportProblem::Unavailable)
        })
    }

    fn code(&self) -> Option<String> {
        serde_json::from_str::<Refusal>(&self.body)
            .ok()
            .and_then(|refusal| refusal.code)
    }

    // 서버가 아직 이 경로를 모르는 판(404)도 여기로 온다. 화면은 멈추지 않고 보고할 수 없다고 알린다
    fn unexpected(&self, step: &str) -> Failure {
        log::warn!(
            "The {step} step answered {} {}",
            self.status,
            self.code().unwrap_or_default()
        );
        Failure::fail(ReportProblem::Unavailable)
    }
}

// 요청 본문에 Minecraft 토큰이 든다. 오류는 상태와 코드만 남기고 요청은 로그에 싣지 않는다
async fn post(http: &Http, url: &str, body: &Value) -> Result<Answer, Failure> {
    let response = http
        .client()
        .post(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(transport)?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok());
    let body = response.text().await.map_err(transport)?;
    Ok(Answer {
        status,
        retry_after,
        body,
    })
}

fn transport(error: reqwest::Error) -> Failure {
    log::warn!(
        "Could not reach the crash report server: {}",
        error.without_url()
    );
    Failure::fail(ReportProblem::Network)
}

fn limit(text: &str, units: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= units
        })
        .collect()
}

#[cfg(test)]
mod tests;
