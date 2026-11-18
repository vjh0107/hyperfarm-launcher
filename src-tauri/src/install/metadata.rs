use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use super::rules::Rule;

#[derive(Deserialize)]
pub struct VersionManifest {
    pub versions: Vec<VersionEntry>,
}

#[derive(Deserialize)]
pub struct VersionEntry {
    pub id: String,
    pub url: String,
    pub sha1: String,
}

#[derive(Deserialize, Clone)]
pub struct Download {
    pub path: Option<String>,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Default, Clone)]
pub struct Arguments {
    pub game: Option<Vec<Value>>,
    pub jvm: Option<Vec<Value>>,
    #[serde(rename = "default-user-jvm")]
    pub default_user_jvm: Option<Vec<Value>>,
}

#[derive(Deserialize)]
pub struct VersionInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub arguments: Arguments,
    #[serde(rename = "assetIndex")]
    pub asset_index: AssetIndexRef,
    pub downloads: ClientDownloads,
    #[serde(rename = "javaVersion")]
    pub java_version: JavaVersion,
    pub libraries: Vec<Library>,
    pub logging: Option<Logging>,
}

#[derive(Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize)]
pub struct ClientDownloads {
    pub client: Download,
}

#[derive(Deserialize)]
pub struct JavaVersion {
    pub component: String,
}

#[derive(Deserialize)]
pub struct Logging {
    pub client: Option<LoggingEntry>,
}

#[derive(Deserialize)]
pub struct LoggingEntry {
    pub argument: String,
    pub file: LoggingFile,
}

#[derive(Deserialize)]
pub struct LoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Clone)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    pub url: Option<String>,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Deserialize, Clone)]
pub struct LibraryDownloads {
    pub artifact: Option<Download>,
}

#[derive(Deserialize)]
pub struct FabricProfile {
    pub id: String,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    pub arguments: Option<Arguments>,
    pub libraries: Vec<Library>,
}

#[derive(Deserialize)]
pub struct AssetIndex {
    pub objects: BTreeMap<String, AssetObject>,
}

#[derive(Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

pub type RuntimeIndex = BTreeMap<String, BTreeMap<String, Vec<RuntimeVariant>>>;

#[derive(Deserialize)]
pub struct RuntimeVariant {
    pub manifest: Download,
}

#[derive(Deserialize)]
pub struct RuntimeManifest {
    pub files: BTreeMap<String, RuntimeFile>,
}

#[derive(Deserialize)]
pub struct RuntimeFile {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub executable: bool,
    pub downloads: Option<RuntimeDownloads>,
    pub target: Option<String>,
}

#[derive(Deserialize)]
pub struct RuntimeDownloads {
    pub raw: Download,
}
