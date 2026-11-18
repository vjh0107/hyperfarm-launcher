use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const PAGE: &str = "<!doctype html><html lang=\"ko\"><head><meta charset=\"utf-8\"><title>하이퍼팜</title></head>\
<body style=\"font-family:sans-serif;text-align:center;padding-top:20vh\"><p>로그인을 마쳤어요. 이 탭은 닫아도 돼요.</p></body></html>";

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

    async fn accept_redirect(&self) -> HashMap<String, String> {
        loop {
            let Ok((stream, _)) = self.listener.accept().await else {
                continue;
            };
            if let Some(query) = answer(stream).await {
                return query;
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
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
        PAGE.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    Some(query)
}

pub fn parse_query(raw: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect()
}
