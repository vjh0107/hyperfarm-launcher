use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use serde::Deserialize;
use serde_json::Value;

const APPLICATION_ID: &str = "1530169660970897418";
const REFRESH_INTERVAL: Duration = Duration::from_secs(15);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
// 끊긴 연결은 보낼 때에야 드러난다. 같은 줄만 이어지는 동안 Discord 를 다시 켜면 이것이 presence 를 되살린다
const RESEND_AFTER: Duration = Duration::from_secs(60);
const STATUS_FOLDER: &str = "hyperfarm";
const STATUS_FILE: &str = "presence.json";
const LINE_LIMIT_BYTES: usize = 128;
const LINE_MINIMUM_CHARS: usize = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Status {
    details: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct StatusFile {
    details: Option<String>,
    state: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum StatusError {
    #[error("could not read the status file: {0}")]
    Unreadable(io::Error),
    #[error("the status file is not valid JSON: {0}")]
    Invalid(serde_json::Error),
}

struct Connection {
    client: DiscordIpcClient,
    sent: Sent,
}

struct Sent {
    status: Status,
    at: Instant,
}

fn status_file(game_directory: &Path) -> PathBuf {
    game_directory.join(STATUS_FOLDER).join(STATUS_FILE)
}

// 모드는 서버를 옮기거나 끊길 때 이 파일을 지우지만, 꺼지면서 남긴 것은 다음 게임이 앞 게임의 자리를 띄우게 한다
pub async fn forget(game_directory: &Path) {
    match tokio::fs::remove_file(status_file(game_directory)).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("Could not remove the previous in-game status: {error}"),
    }
}

pub async fn show(homepage: String, game_directory: PathBuf, launched: SystemTime) {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs().cast_signed());
    let file = status_file(&game_directory);
    let mut connection = None;
    let mut refresh = tokio::time::interval(REFRESH_INTERVAL);
    loop {
        let now = refresh.tick().await.into_std();
        let homepage = homepage.clone();
        let file = file.clone();
        let publishing = tokio::task::spawn_blocking(move || {
            publish(
                connection,
                read_status(&file, launched),
                now,
                started,
                &homepage,
            )
        });
        connection = match tokio::time::timeout(ANSWER_TIMEOUT, publishing).await {
            Ok(published) => published.ok().flatten(),
            // IPC 에는 read timeout 이 없어 멈춘 스레드를 되찾지 못한다. 차례마다 다시 붙으면 그런 스레드가 쌓인다
            Err(_) => {
                log::warn!("Discord stopped answering; giving up on Rich Presence");
                return;
            }
        };
    }
}

// 파일의 수정 시각을 보는 것은 지우지 못한 앞 게임의 파일을 이번 게임의 자리로 읽지 않기 위해서다
fn read_status(file: &Path, launched: SystemTime) -> Result<Status, StatusError> {
    let modified = match std::fs::metadata(file).and_then(|metadata| metadata.modified()) {
        Ok(modified) => modified,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Status::default()),
        Err(error) => return Err(StatusError::Unreadable(error)),
    };
    if modified < launched {
        return Ok(Status::default());
    }
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Status::default()),
        Err(error) => return Err(StatusError::Unreadable(error)),
    };
    let parsed: StatusFile = serde_json::from_slice(&bytes).map_err(StatusError::Invalid)?;
    Ok(Status {
        details: parsed.details.and_then(line),
        state: parsed.state.and_then(line),
    })
}

// Discord 는 공백을 빼고 두 글자가 안 되는 줄이 하나라도 있으면 활동 전체를 거절한다(osu! 가 겪어 적어 둔 것이다).
// 128 이 글자인지 바이트인지 확인되지 않아 더 좁은 UTF-8 바이트로 센다
fn line(text: String) -> Option<String> {
    let text = text.trim();
    let mut length = 0;
    let end = text
        .char_indices()
        .find(|(_, character)| {
            length += character.len_utf8();
            length > LINE_LIMIT_BYTES
        })
        .map_or(text.len(), |(index, _)| index);
    let text = text[..end].trim_end();
    (text.chars().count() >= LINE_MINIMUM_CHARS).then(|| text.to_string())
}

fn needs_sending(sent: Option<&Sent>, status: &Status, now: Instant) -> bool {
    sent.is_none_or(|sent| sent.status != *status || now.duration_since(sent.at) >= RESEND_AFTER)
}

// Discord 가 꺼져 있거나 도중에 꺼지면 연결을 버리고 다음 차례에 다시 붙는다. 새 연결에는 보낸 것이 없으므로 같은 줄도 다시 간다
fn publish(
    connection: Option<Connection>,
    reading: Result<Status, StatusError>,
    now: Instant,
    started: i64,
    homepage: &str,
) -> Option<Connection> {
    let status = match &reading {
        Ok(status) => status.clone(),
        Err(_) => Status::default(),
    };
    if !needs_sending(
        connection.as_ref().map(|connection| &connection.sent),
        &status,
        now,
    ) {
        return connection;
    }
    let mut client = match connection {
        Some(connection) => connection.client,
        None => {
            let mut client = DiscordIpcClient::new(APPLICATION_ID);
            if let Err(error) = client.connect() {
                log::debug!("Discord is not reachable: {error}");
                return None;
            }
            log::info!("Connected to Discord");
            client
        }
    };
    if let Err(error) = &reading {
        log::warn!("Showing no in-game status: {error}");
    }
    let mut activity = Activity::new()
        .name("하이퍼팜")
        .timestamps(Timestamps::new().start(started))
        .buttons(vec![Button::new("함께 플레이하기", homepage)]);
    if let Some(details) = &status.details {
        activity = activity.details(details.as_str());
    }
    if let Some(state) = &status.state {
        activity = activity.state(state.as_str());
    }
    // 응답을 읽지 않으면 보낼 때마다 쌓인다
    match client.set_activity(activity).and_then(|()| client.recv()) {
        Ok((_, answer)) => {
            if answer.get("evt").and_then(Value::as_str) == Some("ERROR") {
                log::warn!("Discord rejected the activity: {}", answer["data"]);
            }
            Some(Connection {
                client,
                sent: Sent { status, at: now },
            })
        }
        Err(error) => {
            log::info!("Lost the Discord connection: {error}");
            let _ = client.close();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime};

    use super::{
        RESEND_AFTER, Sent, Status, StatusError, line, needs_sending, read_status, status_file,
    };

    fn status(details: Option<&str>, state: Option<&str>) -> Status {
        Status {
            details: details.map(str::to_string),
            state: state.map(str::to_string),
        }
    }

    fn written(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let file = status_file(directory.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, contents).unwrap();
        (directory, file)
    }

    fn launched_before_writing() -> SystemTime {
        SystemTime::now() - Duration::from_secs(60)
    }

    #[test]
    fn reads_both_lines_the_mod_wrote() {
        let (_directory, file) = written(r#"{"details":"타운 3채널","state":"달빛섬 · Lv.12"}"#);

        let read = read_status(&file, launched_before_writing()).unwrap();

        assert_eq!(read, status(Some("타운 3채널"), Some("달빛섬 · Lv.12")));
    }

    #[test]
    fn leaves_a_line_empty_when_its_key_is_missing_or_blank() {
        let (_missing, missing) = written(r#"{"details":"내 섬에서"}"#);
        let (_blank, blank) = written(r#"{"details":"","state":"달빛섬 · Lv.12"}"#);

        assert_eq!(
            read_status(&missing, launched_before_writing()).unwrap(),
            status(Some("내 섬에서"), None)
        );
        assert_eq!(
            read_status(&blank, launched_before_writing()).unwrap(),
            status(None, Some("달빛섬 · Lv.12"))
        );
    }

    #[test]
    fn shows_nothing_when_there_is_no_file() {
        let directory = tempfile::tempdir().unwrap();

        let read = read_status(&status_file(directory.path()), launched_before_writing()).unwrap();

        assert_eq!(read, Status::default());
    }

    #[test]
    fn refuses_a_file_that_is_not_json() {
        let (_directory, file) = written(r#"{"details":"타운 3채"#);

        let read = read_status(&file, launched_before_writing());

        assert!(matches!(read, Err(StatusError::Invalid(_))), "{read:?}");
    }

    #[test]
    fn ignores_a_file_left_from_before_this_game_started() {
        let (_directory, file) = written(r#"{"details":"타운 3채널","state":"달빛섬 · Lv.12"}"#);

        let read = read_status(&file, SystemTime::now() + Duration::from_secs(60)).unwrap();

        assert_eq!(read, Status::default());
    }

    #[test]
    fn sends_only_what_differs_from_the_last_activity_on_this_connection() {
        let now = Instant::now();
        let town = status(Some("타운 3채널"), Some("달빛섬 · Lv.12"));
        let fishing = status(Some("타운 3채널에서 낚시 중"), Some("달빛섬 · Lv.12"));
        let sent_town = Sent {
            status: town.clone(),
            at: now,
        };

        assert!(!needs_sending(Some(&sent_town), &town, now));
        assert!(needs_sending(Some(&sent_town), &fishing, now));
        assert!(needs_sending(Some(&sent_town), &Status::default(), now));
        assert!(needs_sending(None, &town, now));
        assert!(needs_sending(None, &Status::default(), now));
    }

    #[test]
    fn sends_the_same_lines_again_once_a_minute_has_passed() {
        let sent_at = Instant::now();
        let town = status(Some("타운 3채널"), Some("달빛섬 · Lv.12"));
        let sent = Sent {
            status: town.clone(),
            at: sent_at,
        };

        assert!(!needs_sending(
            Some(&sent),
            &town,
            sent_at + RESEND_AFTER - Duration::from_secs(1)
        ));
        assert!(needs_sending(Some(&sent), &town, sent_at + RESEND_AFTER));
    }

    #[test]
    fn cuts_a_long_line_at_a_character_within_discords_limit() {
        assert_eq!(line("섬".repeat(50)).unwrap(), "섬".repeat(42));

        let fits = format!("ab{}", "섬".repeat(42));
        assert_eq!(fits.len(), 128);
        assert_eq!(line(fits.clone()).unwrap(), fits);

        let over = format!("abc{}", "섬".repeat(42));
        assert_eq!(line(over).unwrap(), format!("abc{}", "섬".repeat(41)));
    }

    #[test]
    fn drops_a_line_too_short_for_discord() {
        assert_eq!(line("섬".to_string()), None);
        assert_eq!(line(" 섬 ".to_string()), None);
        assert_eq!(line("섬섬".to_string()).as_deref(), Some("섬섬"));
    }
}
