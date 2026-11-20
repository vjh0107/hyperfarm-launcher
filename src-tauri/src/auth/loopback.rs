use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use super::microsoft::INTERACTION_ERRORS;

const PAGE: &str = include_str!("signed_in.html");
const LOGO: &[u8] = include_bytes!("../../../src/assets/images/logo.webp");
const HERO: &[u8] = include_bytes!("../../../src/assets/images/bg.webp");
const CHECK: &str = "M20 6L9 17l-5-5";
const CROSS: &str = "M6 6l12 12M18 6L6 18";
const NEXT: &str = "M5 12h14M13 6l6 6-6 6";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn page(query: &HashMap<String, String>) -> String {
    let (title, message, icon, accent, soft) = match query.get("error").map(String::as_str) {
        None => (
            "로그인을 마쳤어요",
            "하이퍼팜 런처로 돌아가 주세요. 곧 게임이 시작돼요.",
            CHECK,
            "#2a9a68",
            "#e7f7ef",
        ),
        // prompt=none 이 로그인 창으로 넘어가는 길이다. 실패로 보이면 다음 창에서 로그인하지 않고 런처로 돌아간다
        Some(error) if INTERACTION_ERRORS.contains(&error) => (
            "로그인이 필요해요",
            "다음 창에서 로그인해 주세요.",
            NEXT,
            "#6b7684",
            "#f0f3f7",
        ),
        Some("access_denied") => (
            "로그인을 취소했어요",
            "다시 하려면 런처에서 다시 시도를 눌러 주세요.",
            CROSS,
            "#6b7684",
            "#f0f3f7",
        ),
        Some(_) => (
            "로그인하지 못했어요",
            "런처로 돌아가 다시 시도해 주세요.",
            CROSS,
            "#f5535c",
            "#fff1f2",
        ),
    };
    PAGE.replace("{{hero}}", &STANDARD.encode(HERO))
        .replace("{{logo}}", &STANDARD.encode(LOGO))
        .replace("{{title}}", title)
        .replace("{{message}}", message)
        .replace("{{icon}}", icon)
        .replace("{{accent}}", accent)
        .replace("{{accent_soft}}", soft)
}

pub struct LoopbackReceiver {
    listener: TcpListener,
}

impl LoopbackReceiver {
    pub async fn bind() -> std::io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(("127.0.0.1", 0)).await?,
        })
    }

    pub fn redirect_uri(&self) -> String {
        let port = self
            .listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or_default();
        format!("http://localhost:{port}")
    }

    pub async fn await_redirect(&self, timeout: Duration) -> Option<HashMap<String, String>> {
        tokio::time::timeout(timeout, self.accept_redirect())
            .await
            .ok()
    }

    // 브라우저는 미리 열어 둔 빈 연결을 붙든 채 진짜 redirect 를 다른 연결로 보낸다. 하나씩 받으면 그 빈 연결이 막는다
    async fn accept_redirect(&self) -> HashMap<String, String> {
        let mut answering = JoinSet::new();
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    if let Ok((stream, _)) = accepted {
                        answering.spawn(tokio::time::timeout(REQUEST_TIMEOUT, answer(stream)));
                    }
                }
                Some(answered) = answering.join_next() => {
                    if let Ok(Ok(Some(query))) = answered {
                        return query;
                    }
                }
            }
        }
    }
}

async fn answer(mut stream: TcpStream) -> Option<HashMap<String, String>> {
    let mut buffer = vec![0u8; 8192];
    let mut read = 0;
    while read < buffer.len() {
        let n = stream.read(&mut buffer[read..]).await.ok()?;
        if n == 0 {
            break;
        }
        read += n;
        if buffer[..read]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
    }
    let request = String::from_utf8_lossy(&buffer[..read]);
    let target = request.lines().next()?.split_whitespace().nth(1)?;
    let query = parse_query(target.split_once('?').map(|(_, query)| query).unwrap_or(""));
    if !query.contains_key("code") && !query.contains_key("error") {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return None;
    }
    let body = page(&query);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    Some(query)
}

pub fn parse_query(raw: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect()
}
