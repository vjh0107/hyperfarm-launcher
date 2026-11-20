use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

const APPLICATION_ID: &str = "1530169660970897418";
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn show(homepage: String) {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs().cast_signed());
    let mut client = None;
    let mut refresh = tokio::time::interval(REFRESH_INTERVAL);
    loop {
        refresh.tick().await;
        let homepage = homepage.clone();
        let publishing = tokio::task::spawn_blocking(move || publish(client, started, &homepage));
        client = match tokio::time::timeout(ANSWER_TIMEOUT, publishing).await {
            Ok(published) => published.ok().flatten(),
            // IPC 에는 read timeout 이 없어 멈춘 스레드를 되찾지 못한다. 차례마다 다시 붙으면 그런 스레드가 쌓인다
            Err(_) => {
                log::warn!("Discord stopped answering; giving up on Rich Presence");
                return;
            }
        };
    }
}

// Discord 가 꺼져 있거나 도중에 꺼지면 연결을 버리고 다음 차례에 다시 붙는다
fn publish(
    client: Option<DiscordIpcClient>,
    started: i64,
    homepage: &str,
) -> Option<DiscordIpcClient> {
    let mut client = match client {
        Some(client) => client,
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
    let activity = Activity::new()
        .name("하이퍼팜")
        .timestamps(Timestamps::new().start(started))
        .buttons(vec![Button::new("함께 플레이하기", homepage)]);
    // 응답을 읽지 않으면 30초마다 쌓인다
    match client.set_activity(activity).and_then(|()| client.recv()) {
        Ok(_) => Some(client),
        Err(error) => {
            log::info!("Lost the Discord connection: {error}");
            let _ = client.close();
            None
        }
    }
}
