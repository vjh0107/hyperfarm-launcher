use serde::Deserialize;

use crate::http::Http;

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
    let valid = code.len() == 43
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then_some(code)
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
        Err(error) => {
            log::warn!("Could not redeem the launch ticket; continuing without it: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ticket_code;

    const CODE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJ0123-_6";

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
}
