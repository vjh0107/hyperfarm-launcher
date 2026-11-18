use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Problem {
    Network,
    LoginCancelled,
    LoginFailed,
    NoXboxProfile,
    AccountRestricted,
    ChildAccount,
    NoJavaEdition,
    DiskFull,
    CorruptDownload,
    PackInvalid,
    UnsupportedPlatform,
    GameExited,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiskSpace {
    pub needed_bytes: u64,
    pub usable_bytes: u64,
}

#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct LauncherError {
    pub problem: Problem,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<DiskSpace>,
}

impl LauncherError {
    pub fn new(problem: Problem, message: impl Into<String>) -> Self {
        Self {
            problem,
            message: message.into(),
            disk: None,
        }
    }

    pub fn network() -> Self {
        Self::new(Problem::Network, "인터넷 연결을 확인해 주세요.")
    }

    pub fn disk_full(needed_bytes: u64, usable_bytes: u64) -> Self {
        Self {
            problem: Problem::DiskFull,
            message: "저장 공간이 부족해요.".into(),
            disk: Some(DiskSpace {
                needed_bytes,
                usable_bytes,
            }),
        }
    }
}
