use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{LauncherError, Problem};
use crate::http::{Http, HttpError};

const NO_JAVA_EDITION: &str = "이 Microsoft 계정에는 마인크래프트 자바 에디션이 없어요.";
const XBOX_FAILED: &str = "Xbox Live 인증에 실패했어요. 다시 시도해 주세요.";

#[derive(Clone)]
pub struct XboxEndpoints {
    pub user_authenticate: String,
    pub xsts_authorize: String,
    pub minecraft_login: String,
    pub minecraft_profile: String,
}

impl Default for XboxEndpoints {
    fn default() -> Self {
        Self {
            user_authenticate: "https://user.auth.xboxlive.com/user/authenticate".into(),
            xsts_authorize: "https://xsts.auth.xboxlive.com/xsts/authorize".into(),
            minecraft_login: "https://api.minecraftservices.com/authentication/login_with_xbox"
                .into(),
            minecraft_profile: "https://api.minecraftservices.com/minecraft/profile".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MinecraftSession {
    #[serde(skip)]
    pub access_token: String,
    pub player_id: String,
    pub name: String,
    #[serde(skip)]
    pub xuid: String,
}

#[derive(Deserialize)]
struct XboxResponse {
    #[serde(rename = "Token")]
    token: Option<String>,
    #[serde(rename = "DisplayClaims")]
    display_claims: Option<DisplayClaims>,
}

#[derive(Deserialize)]
struct DisplayClaims {
    xui: Option<Vec<Xui>>,
}

#[derive(Deserialize)]
struct Xui {
    uhs: Option<String>,
}

#[derive(Deserialize)]
struct LoginResponse {
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct ProfileResponse {
    id: Option<String>,
    name: Option<String>,
}

pub struct XboxClient {
    http: Http,
    endpoints: XboxEndpoints,
}

impl XboxClient {
    pub fn new(http: Http, endpoints: XboxEndpoints) -> Self {
        Self { http, endpoints }
    }

    pub async fn authenticate(
        &self,
        microsoft_access_token: &str,
    ) -> Result<MinecraftSession, LauncherError> {
        let xbox_live: XboxResponse = xbox(
            self.http
                .post_json(
                    &self.endpoints.user_authenticate,
                    &json!({
                        "Properties": {
                            "AuthMethod": "RPS",
                            "SiteName": "user.auth.xboxlive.com",
                            "RpsTicket": format!("d={microsoft_access_token}"),
                        },
                        "RelyingParty": "http://auth.xboxlive.com",
                        "TokenType": "JWT",
                    }),
                )
                .await,
            XBOX_FAILED,
        )?;
        let xbox_token = xbox_live
            .token
            .ok_or_else(|| LauncherError::new(Problem::LoginFailed, XBOX_FAILED))?;

        let xsts: XboxResponse = xbox(
            self.http
                .post_json(
                    &self.endpoints.xsts_authorize,
                    &json!({
                        "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbox_token] },
                        "RelyingParty": "rp://api.minecraftservices.com/",
                        "TokenType": "JWT",
                    }),
                )
                .await,
            XBOX_FAILED,
        )?;
        let user_hash = xsts
            .display_claims
            .and_then(|claims| claims.xui)
            .and_then(|users| users.into_iter().next())
            .and_then(|user| user.uhs);
        let (Some(xsts_token), Some(user_hash)) = (xsts.token, user_hash) else {
            return Err(LauncherError::new(Problem::LoginFailed, XBOX_FAILED));
        };

        let login: LoginResponse = xbox(
            self.http
                .post_json(
                    &self.endpoints.minecraft_login,
                    &json!({ "identityToken": format!("XBL3.0 x={user_hash};{xsts_token}") }),
                )
                .await,
            NO_JAVA_EDITION,
        )?;
        let minecraft_token = login
            .access_token
            .ok_or_else(|| LauncherError::new(Problem::NoJavaEdition, NO_JAVA_EDITION))?;

        let profile: ProfileResponse = match self
            .http
            .get_json_bearer(&self.endpoints.minecraft_profile, &minecraft_token)
            .await
        {
            Ok(profile) => profile,
            Err(error) if error.status() == Some(404) => {
                return Err(LauncherError::new(Problem::NoJavaEdition, NO_JAVA_EDITION));
            }
            Err(error) => return Err(error.into()),
        };
        let (Some(id), Some(name)) = (profile.id, profile.name) else {
            return Err(LauncherError::new(Problem::NoJavaEdition, NO_JAVA_EDITION));
        };
        Ok(MinecraftSession {
            xuid: xuid(&minecraft_token).unwrap_or_default(),
            access_token: minecraft_token,
            player_id: dashed_uuid(&id)?,
            name,
        })
    }
}

// Minecraft 용 XSTS 는 xid 를 싣지 않는다. 같은 값이 Minecraft 액세스 토큰의 xuid 클레임에 있다
fn xuid(minecraft_token: &str) -> Option<String> {
    let payload = URL_SAFE_NO_PAD
        .decode(minecraft_token.split('.').nth(1)?.trim_end_matches('='))
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    claims.get("xuid")?.as_str().map(str::to_string)
}

fn xbox<T>(result: Result<T, HttpError>, failure: &str) -> Result<T, LauncherError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            log::warn!("Xbox sign-in step failed: {error}: {}", error.body());
            if let Some(described) = describe(error.body()) {
                return Err(described);
            }
            if error
                .status()
                .is_some_and(|status| (400..500).contains(&status))
            {
                return Err(LauncherError::new(Problem::LoginFailed, failure));
            }
            Err(error.into())
        }
    }
}

pub fn describe(body: &str) -> Option<LauncherError> {
    if !body.contains("XErr") {
        return None;
    }
    let (problem, message) = if body.contains("2148916233") {
        (
            Problem::NoXboxProfile,
            "Xbox 프로필을 만든 뒤 다시 시도해 주세요.",
        )
    } else if body.contains("2148916235") {
        (
            Problem::AccountRestricted,
            "Xbox Live 를 쓸 수 없는 국가의 계정이에요.",
        )
    } else if body.contains("2148916236") || body.contains("2148916237") {
        (
            Problem::AccountRestricted,
            "성인 인증을 마친 뒤 다시 시도해 주세요.",
        )
    } else if body.contains("2148916238") {
        (
            Problem::ChildAccount,
            "미성년 계정은 보호자의 Microsoft 가족 그룹에 추가되어야 해요.",
        )
    } else {
        return None;
    };
    Some(LauncherError::new(problem, message))
}

pub fn dashed_uuid(undashed: &str) -> Result<String, LauncherError> {
    if undashed.len() != 32 || !undashed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(LauncherError::new(
            Problem::LoginFailed,
            "마인크래프트 프로필을 읽지 못했어요.",
        ));
    }
    let lower = undashed.to_ascii_lowercase();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &lower[0..8],
        &lower[8..12],
        &lower[12..16],
        &lower[16..20],
        &lower[20..32]
    ))
}
