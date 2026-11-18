use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CLIENT_ID: &str = "0d78a481-dd72-458d-9a39-2eb991abdba2";
const WEB_BASE: &str = "https://hyperf.kr";
const PACK_MANIFEST: &str = "https://pack.hyperf.kr/launcher/manifest.json";
const SIXTEEN_GIGABYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LauncherConfig {
    pub web_base: String,
    pub pack_manifest: String,
    pub home: PathBuf,
}

impl LauncherConfig {
    pub fn load() -> Self {
        let variable = |name: &str, default: &str| std::env::var(name).ok().filter(|value| !value.is_empty()).unwrap_or_else(|| default.into());
        Self {
            web_base: variable("HYPERFARM_WEB", WEB_BASE),
            pack_manifest: variable("HYPERFARM_PACK", PACK_MANIFEST),
            home: std::env::var_os("HYPERFARM_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("HyperFarm")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSettings {
    pub memory_megabytes: u32,
    pub game_directory: Option<PathBuf>,
}

impl LauncherSettings {
    pub fn defaults(total_memory_bytes: u64) -> Self {
        Self { memory_megabytes: if total_memory_bytes >= SIXTEEN_GIGABYTES { 6144 } else { 4096 }, game_directory: None }
    }
}

pub struct SettingsStore {
    file: PathBuf,
    total_memory_bytes: u64,
}

impl SettingsStore {
    pub fn new(file: PathBuf, total_memory_bytes: u64) -> Self {
        Self { file, total_memory_bytes }
    }

    pub fn load(&self) -> LauncherSettings {
        let Ok(text) = std::fs::read_to_string(&self.file) else { return LauncherSettings::defaults(self.total_memory_bytes) };
        serde_json::from_str(&text).unwrap_or_else(|error| {
            log::warn!("Ignoring unreadable launcher settings {}: {error}", self.file.display());
            LauncherSettings::defaults(self.total_memory_bytes)
        })
    }

    pub fn save(&self, settings: &LauncherSettings) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.file, serde_json::to_string_pretty(settings).expect("settings always serialize"))
    }

    pub fn game_directory(&self, home: &Path) -> PathBuf {
        self.load().game_directory.unwrap_or_else(|| home.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_defaults_when_the_file_is_broken() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("launcher.json");
        std::fs::write(&file, "{not json").unwrap();
        let store = SettingsStore::new(file, SIXTEEN_GIGABYTES);
        assert_eq!(store.load(), LauncherSettings { memory_megabytes: 6144, game_directory: None });
    }

    #[test]
    fn gives_less_memory_below_sixteen_gigabytes() {
        assert_eq!(LauncherSettings::defaults(SIXTEEN_GIGABYTES - 1).memory_megabytes, 4096);
    }
}
