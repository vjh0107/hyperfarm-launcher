use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use tokio::process::Child;

const APPLICATION_ID: &str = "1530169660970897418";
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

pub async fn show_until_exit(game: &mut Child, homepage: String) {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs().cast_signed());
    let mut client = None;
    let mut refresh = tokio::time::interval(REFRESH_INTERVAL);
    loop {
        tokio::select! {
            status = game.wait() => {
                log::info!("The game exited with {status:?}");
                break;
            }
            _ = refresh.tick() => {
                let homepage = homepage.clone();
                client = tokio::task::spawn_blocking(move || publish(client, started, &homepage))
                    .await
                    .ok()
                    .flatten();
            }
        }
    }
    if let Some(mut client) = client {
        let _ = tokio::task::spawn_blocking(move || client.close()).await;
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
