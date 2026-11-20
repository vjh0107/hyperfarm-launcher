use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::http::Http;

const KEPT_TICKET: &str = "launch-ticket.json";
const KEPT_FOR: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptTicket {
    code: String,
    kept_at: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchTicket {
    pub player_id: String,
    pub login_hint: Option<String>,
}

/// `hyperfarm://launch?ticket=<43자 base64url>` 에서 ticket 을 꺼낸다. 다른 동작이나 모양이 틀린 코드는 버린다.
pub fn ticket_code(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "hyperfarm" || url.host_str() != Some("launch") {
        return None;
    }
    let code = url
        .query_pairs()
        .find(|(key, _)| key == "ticket")?
        .1
        .into_owned();
    is_code(&code).then_some(code)
}

fn is_code(code: &str) -> bool {
    code.len() == 43
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// 업데이트로 다시 켜지면 링크로 받은 ticket 이 메모리와 함께 사라져 브라우저의 아무 계정으로 들어간다
pub fn keep(home: &Path, code: &str) -> std::io::Result<()> {
    keep_at(home, code, SystemTime::now())
}

fn keep_at(home: &Path, code: &str, now: SystemTime) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let kept = KeptTicket {
        code: code.into(),
        kept_at: seconds(now),
    };
    std::fs::write(
        home.join(KEPT_TICKET),
        serde_json::to_vec(&kept).expect("a kept ticket always serializes"),
    )
}

pub fn take_kept(home: &Path) -> Option<String> {
    take_kept_at(home, SystemTime::now())
}

fn take_kept_at(home: &Path, now: SystemTime) -> Option<String> {
    let text = std::fs::read(home.join(KEPT_TICKET)).ok()?;
    forget(home);
    let Ok(kept) = serde_json::from_slice::<KeptTicket>(&text) else {
        log::warn!("Ignoring an unreadable launch ticket kept across a restart");
        return None;
    };
    let fresh = seconds(now)
        .checked_sub(kept.kept_at)
        .is_some_and(|age| age <= KEPT_FOR.as_secs());
    if !fresh {
        log::info!("Ignoring a stale launch ticket kept across a restart");
        return None;
    }
    is_code(&kept.code).then_some(kept.code)
}

pub fn forget(home: &Path) {
    match std::fs::remove_file(home.join(KEPT_TICKET)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            log::warn!("Could not remove the kept launch ticket: {error}");
        }
        _ => {}
    }
}

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// ticket 은 어느 계정으로 로그인할지 고르는 힌트일 뿐이라, 바꾸지 못해도 실행은 멈추지 않는다.
pub async fn redeem(http: &Http, web_base: &str, code: &str) -> Option<LaunchTicket> {
    match http
        .post_empty(&format!("{web_base}/api/launcher/tickets/{code}/redeem"))
        .await
    {
        Ok(ticket) => Some(ticket),
        Err(error) if error.status() == Some(404) => {
            log::info!("Launch ticket was already used or expired; continuing without it");
            None
        }
        // 교환 주소에 ticket 이 들어 있다. 쓰이지 않은 ticket 이 로그에 남지 않게 주소를 뗀다
        Err(error) => {
            log::warn!(
                "Could not redeem the launch ticket; continuing without it: {}",
                error.without_url()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, Once};

    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use std::time::{Duration, SystemTime};

    use super::{KEPT_FOR, keep_at, redeem, take_kept_at, ticket_code};
    use crate::http::Http;

    const CODE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJ0123-_6";

    struct CapturedLog(Mutex<Vec<String>>);

    impl log::Log for CapturedLog {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            self.0.lock().unwrap().push(record.args().to_string());
        }

        fn flush(&self) {}
    }

    static LOG: CapturedLog = CapturedLog(Mutex::new(Vec::new()));

    fn capture_log() {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            log::set_logger(&LOG).unwrap();
            log::set_max_level(log::LevelFilter::Info);
        });
    }

    #[tokio::test]
    async fn keeps_the_ticket_out_of_the_log_when_it_cannot_be_redeemed() {
        capture_log();
        let failing = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&failing)
            .await;
        let unreachable = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let http = Http::new();

        for web_base in [failing.uri(), unreachable] {
            assert_eq!(redeem(&http, &web_base, CODE).await, None);
        }

        let failures: Vec<String> = LOG
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("Could not redeem the launch ticket"))
            .cloned()
            .collect();
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(
            failures.iter().all(|line| !line.contains(CODE)),
            "{failures:?}"
        );
    }

    #[test]
    fn reads_the_ticket_from_the_launch_uri() {
        assert_eq!(
            ticket_code(&format!("hyperfarm://launch?ticket={CODE}")).as_deref(),
            Some(CODE)
        );
    }

    #[test]
    fn ignores_other_actions_and_malformed_tickets() {
        assert_eq!(
            ticket_code(&format!("hyperfarm://open?ticket={CODE}")),
            None
        );
        assert_eq!(ticket_code("hyperfarm://launch?ticket=../../x"), None);
        assert_eq!(ticket_code(&format!("https://launch?ticket={CODE}")), None);
    }

    #[test]
    fn hands_a_kept_ticket_to_the_next_start_only_once() {
        let home = tempfile::tempdir().unwrap();
        let kept_at = SystemTime::now();
        keep_at(home.path(), CODE, kept_at).unwrap();

        let soon = kept_at + Duration::from_secs(5);
        assert_eq!(take_kept_at(home.path(), soon).as_deref(), Some(CODE));
        assert_eq!(take_kept_at(home.path(), soon), None);
    }

    #[test]
    fn drops_a_ticket_kept_too_long_ago() {
        let home = tempfile::tempdir().unwrap();
        let kept_at = SystemTime::now();
        keep_at(home.path(), CODE, kept_at).unwrap();

        let late = kept_at + KEPT_FOR + Duration::from_secs(1);
        assert_eq!(take_kept_at(home.path(), late), None);
        assert_eq!(take_kept_at(home.path(), kept_at), None);
    }
}
