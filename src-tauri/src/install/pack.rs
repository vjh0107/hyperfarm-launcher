use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{LauncherError, Problem};
use crate::http::Http;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PackManifest {
    pub minecraft_version: String,
    pub fabric_loader_version: String,
    pub server: String,
    pub mods: Vec<PackFile>,
    pub shaderpacks: Vec<PackFile>,
    #[serde(default)]
    pub default_options: BTreeMap<String, String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub file_name: String,
    pub url: String,
    pub sha512: String,
    pub size: u64,
}

impl PackFile {
    pub fn safe_file_name(&self) -> Result<&str, LauncherError> {
        let name = self.file_name.as_str();
        if name.contains('/') || name.contains('\\') || name.starts_with('.') || name.is_empty() {
            return Err(LauncherError::new(Problem::PackInvalid, format!("모드 파일 이름이 올바르지 않아요. ({name})")));
        }
        Ok(name)
    }
}

pub async fn load(http: &Http, source: &str) -> Result<PackManifest, LauncherError> {
    if let Some(path) = source.strip_prefix("file://") {
        let text = tokio::fs::read_to_string(path)
            .await
            .map_err(|_| LauncherError::new(Problem::PackInvalid, "모드 구성 정보를 읽지 못했어요."))?;
        return serde_json::from_str(&text).map_err(|_| LauncherError::new(Problem::PackInvalid, "모드 구성 정보를 읽지 못했어요."));
    }
    Ok(http.get_json(source).await?)
}
