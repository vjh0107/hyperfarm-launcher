use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CLIENT_ID: &str = "0d78a481-dd72-458d-9a39-2eb991abdba2";
const WEB_BASE: &str = "https://hyperf.kr";
const PACK_MANIFEST: &str = "https://pack.hyperf.kr/launcher/manifest.json";
// 16GB 를 꽂은 PC 도 펌웨어와 내장 그래픽이 떼어 가 OS 에는 15.xGiB 로 보인다. 16GiB 로 가르면 그 PC 들이 모두 아래 칸에 든다
const LARGER_HEAP_FROM: u64 = 15 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LauncherConfig {
    pub web_base: String,
    pub pack_manifest: String,
    pub home: PathBuf,
}

impl LauncherConfig {
    pub fn load() -> Self {
        let variable = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| default.into())
        };
        Self {
            web_base: variable("HYPERFARM_WEB", WEB_BASE),
            pack_manifest: variable("HYPERFARM_PACK", PACK_MANIFEST),
            home: std::env::var_os("HYPERFARM_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    dirs::data_dir()
                        .unwrap_or_else(std::env::temp_dir)
                        .join("HyperFarm")
                }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSettings {
    pub memory_megabytes: u32,
    pub game_directory: Option<PathBuf>,
    // 0.1.13 까지의 launcher.json 에는 이 키가 없다. 기본값이 없으면 그 파일을 읽지 못해 메모리와 폴더 설정이 통째로 초기화된다
    #[serde(default = "sends_crash_reports_by_default")]
    pub send_crash_reports: bool,
}

fn sends_crash_reports_by_default() -> bool {
    true
}

impl LauncherSettings {
    pub fn defaults(total_memory_bytes: u64) -> Self {
        Self {
            memory_megabytes: if total_memory_bytes >= LARGER_HEAP_FROM {
                6144
            } else {
                4096
            },
            game_directory: None,
            send_crash_reports: sends_crash_reports_by_default(),
        }
    }
}

pub struct SettingsStore {
    file: PathBuf,
    total_memory_bytes: u64,
}

impl SettingsStore {
    pub fn new(file: PathBuf, total_memory_bytes: u64) -> Self {
        Self {
            file,
            total_memory_bytes,
        }
    }

    pub fn load(&self) -> LauncherSettings {
        let Ok(text) = std::fs::read_to_string(&self.file) else {
            return LauncherSettings::defaults(self.total_memory_bytes);
        };
        serde_json::from_str(&text).unwrap_or_else(|error| {
            log::warn!(
                "Ignoring unreadable launcher settings {}: {error}",
                self.file.display()
            );
            LauncherSettings::defaults(self.total_memory_bytes)
        })
    }

    pub fn save(&self, settings: &LauncherSettings) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            &self.file,
            serde_json::to_string_pretty(settings).expect("settings always serialize"),
        )
    }

    pub fn game_directory(&self, home: &Path) -> PathBuf {
        self.load()
            .game_directory
            .unwrap_or_else(|| home.to_path_buf())
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
        let store = SettingsStore::new(file, LARGER_HEAP_FROM);
        assert_eq!(
            store.load(),
            LauncherSettings {
                memory_megabytes: 6144,
                game_directory: None,
                send_crash_reports: true,
            }
        );
    }

    #[test]
    fn keeps_the_settings_of_a_file_written_before_crash_reports_existed() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("launcher.json");
        std::fs::write(
            &file,
            r#"{"memoryMegabytes": 8192, "gameDirectory": "/games/HyperFarm"}"#,
        )
        .unwrap();
        let store = SettingsStore::new(file, LARGER_HEAP_FROM);
        assert_eq!(
            store.load(),
            LauncherSettings {
                memory_megabytes: 8192,
                game_directory: Some(PathBuf::from("/games/HyperFarm")),
                send_crash_reports: true,
            }
        );
    }

    #[test]
    fn gives_a_sixteen_gigabyte_pc_the_larger_heap() {
        let reported_by_windows = 16_947_000_000;
        assert_eq!(
            LauncherSettings::defaults(reported_by_windows).memory_megabytes,
            6144
        );
    }

    #[test]
    fn gives_less_memory_below_fifteen_gibibytes() {
        assert_eq!(
            LauncherSettings::defaults(LARGER_HEAP_FROM - 1).memory_megabytes,
            4096
        );
    }
}
