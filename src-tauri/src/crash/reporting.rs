use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex_lite::Regex;
use serde_json::{Map, Value, json};
use sysinfo::System;
use tokio::task::JoinHandle;
use url::Url;

use super::{Crash, ReportKind, fingerprint, hex, redact_home, summarize};
use crate::http::{Http, HttpError};
use crate::install::options;

const RELEASE: &str = concat!("hyperfarm-launcher@", env!("CARGO_PKG_VERSION"));
const CLIENT: &str = concat!("hyperfarm-launcher/", env!("CARGO_PKG_VERSION"));
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
// Bugsink 의 MAX_EVENT_SIZE(256KiB)를 넘긴 event 는 통째로 버려진다. 요약 줄 몇 개라 칸마다 자르는 것으로 늘 그 안에 든다
const FIELD_LIMIT: usize = 8 * 1024;
const TAG_LIMIT: usize = 200;

// 보내는 것은 게임을 켠 그 실행의 것이라, 크래시를 판정할 때가 아니라 켜기 직전에 잡아 둔다
#[derive(Clone, Debug, Default)]
pub struct LaunchContext {
    pub dsn: Option<String>,
    pub display_adapters: Option<String>,
    pub hyperfarm_mod: Option<String>,
    pub options: PathBuf,
    pub player: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Dsn {
    envelope: Url,
    key: String,
}

impl Dsn {
    fn parse(dsn: &str) -> Option<Self> {
        let mut url = Url::parse(dsn).ok()?;
        if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
            return None;
        }
        let key = url.username().to_string();
        let path = url.path().trim_end_matches('/').to_string();
        let (prefix, project) = path.rsplit_once('/')?;
        if key.is_empty() || project.is_empty() {
            return None;
        }
        url.set_username("").ok()?;
        url.set_password(None).ok()?;
        url.set_path(&format!("{prefix}/api/{project}/envelope/"));
        url.set_query(None);
        Some(Self { envelope: url, key })
    }

    fn auth(&self) -> String {
        format!(
            "Sentry sentry_version=7, sentry_client={CLIENT}, sentry_key={}",
            self.key
        )
    }
}

pub struct CrashReporter {
    http: Http,
    launch: Mutex<Option<LaunchContext>>,
    sending: Mutex<Vec<JoinHandle<()>>>,
}

impl CrashReporter {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            launch: Mutex::new(None),
            sending: Mutex::new(Vec::new()),
        }
    }

    pub fn launching(&self, context: LaunchContext) {
        *self.launch.lock().unwrap() = Some(context);
    }

    pub fn report(&self, crash: &Crash, enabled: bool) {
        let id = &crash.event_id;
        if !enabled {
            log::info!("Not sending crash {id}; crash reports are turned off");
            return;
        }
        let Some(context) = self.launch.lock().unwrap().clone() else {
            log::warn!("Not sending crash {id}; no launch was recorded for it");
            return;
        };
        let Some(address) = context.dsn.as_deref() else {
            log::info!("Not sending crash {id}; the pack names no crash report address");
            return;
        };
        let Some(dsn) = Dsn::parse(address) else {
            log::warn!("Not sending crash {id}; the pack's crash report address is not a DSN");
            return;
        };
        let http = self.http.clone();
        let crash = crash.clone();
        let task = tokio::spawn(async move {
            let backend = tokio::fs::read_to_string(&context.options)
                .await
                .ok()
                .and_then(|text| options::graphics_backend(&text).map(str::to_string));
            let host = System::host_name().ok();
            let event = event(
                &crash,
                &context,
                &Machine {
                    home: dirs::home_dir(),
                    host,
                    graphics_backend: backend,
                    windows: cfg!(windows),
                },
                SystemTime::now(),
            );
            match send(&http, &dsn, &crash.event_id, &event).await {
                Ok(()) => log::info!("Sent crash {}", crash.event_id),
                Err(error) => log::warn!(
                    "Could not send crash {}: {}",
                    crash.event_id,
                    error.without_url()
                ),
            }
        });
        let mut sending = self.sending.lock().unwrap();
        sending.retain(|task| !task.is_finished());
        sending.push(task);
    }

    pub fn is_sending(&self) -> bool {
        self.sending
            .lock()
            .unwrap()
            .iter()
            .any(|task| !task.is_finished())
    }

    pub async fn sent(&self, within: Duration) {
        let sending = std::mem::take(&mut *self.sending.lock().unwrap());
        if sending.is_empty() {
            return;
        }
        if tokio::time::timeout(within, futures_util::future::join_all(sending))
            .await
            .is_err()
        {
            log::warn!("Stopped waiting for a crash report to be sent after {within:?}");
        }
    }
}

struct Machine {
    home: Option<PathBuf>,
    host: Option<String>,
    graphics_backend: Option<String>,
    windows: bool,
}

// 누구의 크래시인지는 싣지 않는다. 그것은 플레이어가 보고하기로 만든 문의만 잇는다. 그래서 user·server_name 을 두지 않고,
// 요약 줄에 섞여 든 홈 경로·이름·UUID·IP 를 지운다. 보고서 전문은 실행 명령줄(액세스 토큰)과 환경 변수를 담아 싣지 않는다
fn event(crash: &Crash, context: &LaunchContext, machine: &Machine, now: SystemTime) -> Value {
    let text = crash.report.as_ref().and_then(super::Report::read);
    let summary: Vec<String> = match (&crash.report, &text) {
        (Some(report), Some(text)) => summarize(report.kind, text),
        _ => Vec::new(),
    }
    .iter()
    .map(|line| limit(&scrub(line, &context.player, machine), FIELD_LIMIT))
    .collect();
    let exit_code = exit_code(crash.code, machine.windows);
    let fingerprint = match (&crash.report, &text) {
        (Some(report), Some(text)) => fingerprint(report.kind, text),
        _ => None,
    }
    .map(|parts| {
        parts
            .iter()
            .map(|part| limit(&redact_home(part, machine.home.as_deref()), FIELD_LIMIT))
            .collect()
    })
    .unwrap_or_else(|| vec!["exit code".into(), exit_code.clone()]);
    let message = if summary.is_empty() {
        format!("The game exited with code {exit_code}")
    } else {
        summary.join("\n")
    };
    let report_kind = match crash.report.as_ref().map(|report| report.kind) {
        Some(ReportKind::Minecraft) => "crash-report",
        Some(ReportKind::Jvm) => "hs_err",
        None => "none",
    };

    let mut tags = Map::new();
    let mut tag = |name: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            tags.insert(name.into(), limit(&value, TAG_LIMIT).into());
        }
    };
    tag("os", System::long_os_version().ok());
    tag("arch", Some(std::env::consts::ARCH.into()));
    tag(
        "gpu",
        context.display_adapters.as_deref().map(|vendors| {
            vendors
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" / ")
        }),
    );
    tag("graphics_backend", machine.graphics_backend.clone());
    tag("mod", context.hyperfarm_mod.clone());
    tag("exit_code", Some(exit_code.clone()));
    tag("report", Some(report_kind.into()));

    json!({
        "event_id": crash.event_id,
        "timestamp": now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64(),
        "platform": "other",
        "level": "fatal",
        "release": RELEASE,
        "message": { "formatted": limit(&message, FIELD_LIMIT) },
        "fingerprint": fingerprint,
        "tags": tags,
        "contexts": {
            "os": {
                "type": "os",
                "name": System::name().ok(),
                "version": System::os_version().ok(),
                "kernel_version": System::kernel_version().ok(),
            },
        },
        "extra": {
            "exit_code": crash.code,
            "report": crash.report.as_ref().map(super::Report::file_name),
            "summary": summary,
        },
    })
}

fn exit_code(code: Option<i32>, windows: bool) -> String {
    match code {
        None => "unknown".into(),
        Some(code) if windows => hex(code),
        Some(code) => code.to_string(),
    }
}

fn scrub(text: &str, player: &[String], machine: &Machine) -> String {
    let uuid = Regex::new(
        r"\b[0-9a-fA-F]{8}-?[0-9a-fA-F]{4}-?[0-9a-fA-F]{4}-?[0-9a-fA-F]{4}-?[0-9a-fA-F]{12}\b",
    )
    .expect("the UUID pattern is valid");
    let ipv4 = Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").expect("the IPv4 pattern is valid");
    let ipv6 = Regex::new(r"[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}")
        .expect("the IPv6 pattern is valid");
    let mut text = redact_home(text, machine.home.as_deref());
    text = uuid.replace_all(&text, "<uuid>").into_owned();
    text = ipv4.replace_all(&text, "<ip>").into_owned();
    // 시각(21:13:05)과 메서드 참조(Foo::bar)도 이 모양에 걸린다. 콜론이 셋 이상이고 숫자 칸이 셋 이상인 것만 주소로 본다
    text = ipv6
        .replace_all(&text, |captures: &regex_lite::Captures| {
            let found = &captures[0];
            let groups = found.split(':').filter(|group| !group.is_empty()).count();
            if found.matches(':').count() >= 3 && groups >= 3 {
                "<ip>".to_string()
            } else {
                found.to_string()
            }
        })
        .into_owned();
    for word in player {
        text = replace_word(&text, word, "<player>");
    }
    if let Some(host) = &machine.host {
        text = replace_word(&text, host, "<host>");
    }
    text
}

// 이름은 대소문자를 가리지 않는다. 앞뒤가 영문·숫자·밑줄이 아닌 자리만 바꿔, 이름이 다른 낱말 안에 든 것은 그대로 둔다
fn replace_word(text: &str, word: &str, with: &str) -> String {
    if word.len() < 3 {
        return text.to_string();
    }
    let haystack = text.to_ascii_lowercase();
    let needle = word.to_ascii_lowercase();
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut result = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(found) = haystack[from..].find(&needle) {
        let start = from + found;
        let end = start + needle.len();
        let before = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word(c));
        let after = text[end..].chars().next().is_none_or(|c| !is_word(c));
        result.push_str(&text[from..start]);
        result.push_str(if before && after {
            with
        } else {
            &text[start..end]
        });
        from = end;
    }
    result.push_str(&text[from..]);
    result
}

fn limit(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_string();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn envelope(event_id: &str, event: &Value) -> Vec<u8> {
    let payload = serde_json::to_vec(event).expect("the crash event always serializes");
    let mut body = serde_json::to_vec(&json!({ "event_id": event_id }))
        .expect("the envelope header always serializes");
    body.push(b'\n');
    body.extend(
        serde_json::to_vec(&json!({ "type": "event", "length": payload.len() }))
            .expect("the item header always serializes"),
    );
    body.push(b'\n');
    body.extend(payload);
    body.push(b'\n');
    body
}

async fn send(http: &Http, dsn: &Dsn, event_id: &str, event: &Value) -> Result<(), HttpError> {
    let response = http
        .client()
        .post(dsn.envelope.clone())
        .header("X-Sentry-Auth", dsn.auth())
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-sentry-envelope",
        )
        .body(envelope(event_id, event))
        .timeout(SEND_TIMEOUT)
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(HttpError::Status {
        url: dsn.envelope.to_string(),
        status: status.as_u16(),
        body: response.text().await.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests;
