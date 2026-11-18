use std::collections::HashSet;
use std::path::Path;

use super::download::{DownloadTask, Hash};
use super::metadata::Library;
use super::rules::{Platform, allows};
use crate::error::{LauncherError, Problem};

pub struct MavenCoordinate {
    group: String,
    artifact: String,
    version: String,
    classifier: Option<String>,
    extension: String,
}

impl MavenCoordinate {
    pub fn parse(name: &str) -> Option<Self> {
        let (coordinate, extension) = name.split_once('@').unwrap_or((name, "jar"));
        let parts: Vec<&str> = coordinate.split(':').collect();
        if !(3..=4).contains(&parts.len()) {
            return None;
        }
        Some(Self {
            group: parts[0].into(),
            artifact: parts[1].into(),
            version: parts[2].into(),
            classifier: parts.get(3).map(|classifier| classifier.to_string()),
            extension: extension.into(),
        })
    }

    pub fn key(&self) -> String {
        match &self.classifier {
            Some(classifier) => format!("{}:{}:{classifier}", self.group, self.artifact),
            None => format!("{}:{}", self.group, self.artifact),
        }
    }

    pub fn path(&self) -> String {
        let classifier = self
            .classifier
            .as_ref()
            .map(|c| format!("-{c}"))
            .unwrap_or_default();
        format!(
            "{}/{}/{}/{}-{}{classifier}.{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            self.artifact,
            self.version,
            self.extension
        )
    }
}

// Fabric 의 ASM 처럼 같은 아티팩트를 두 쪽이 다른 판으로 실으면 Fabric 판이 이겨야 로더가 뜬다
pub fn resolve(
    fabric: &[Library],
    vanilla: &[Library],
    platform: &Platform,
    root: &Path,
) -> Result<Vec<DownloadTask>, LauncherError> {
    let mut seen = HashSet::new();
    let mut tasks = Vec::new();
    for library in fabric.iter().chain(vanilla) {
        if !allows(&library.rules, platform, &HashSet::new()) {
            continue;
        }
        let coordinate = MavenCoordinate::parse(&library.name).ok_or_else(|| {
            LauncherError::new(
                Problem::PackInvalid,
                format!("라이브러리 이름을 읽지 못했어요. ({})", library.name),
            )
        })?;
        if seen.insert(coordinate.key()) {
            tasks.push(task(library, &coordinate, root)?);
        }
    }
    Ok(tasks)
}

fn task(
    library: &Library,
    coordinate: &MavenCoordinate,
    root: &Path,
) -> Result<DownloadTask, LauncherError> {
    if let Some(artifact) = library
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.artifact.as_ref())
    {
        let path = artifact.path.clone().unwrap_or_else(|| coordinate.path());
        return Ok(DownloadTask::new(
            &artifact.url,
            root.join(path),
            Some(artifact.size),
            Some(Hash::sha1(&artifact.sha1)),
        ));
    }
    if let Some(base) = &library.url {
        let base = if base.ends_with('/') {
            base.clone()
        } else {
            format!("{base}/")
        };
        return Ok(DownloadTask::new(
            &format!("{base}{}", coordinate.path()),
            root.join(coordinate.path()),
            library.size,
            library.sha1.as_deref().map(Hash::sha1),
        ));
    }
    Err(LauncherError::new(
        Problem::PackInvalid,
        format!("라이브러리를 받을 주소가 없어요. ({})", library.name),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::metadata::{Download, LibraryDownloads};
    use crate::install::rules::{Os, Rule};

    fn vanilla(name: &str, rules: Vec<Rule>) -> Library {
        let path = MavenCoordinate::parse(name).unwrap().path();
        Library {
            name: name.into(),
            downloads: Some(LibraryDownloads {
                artifact: Some(Download {
                    path: Some(path.clone()),
                    sha1: "bb".into(),
                    size: 20,
                    url: format!("https://libraries.minecraft.net/{path}"),
                }),
            }),
            rules,
            url: None,
            sha1: None,
            size: None,
        }
    }

    fn windows() -> Platform {
        Platform {
            os_name: Platform::WINDOWS,
            arch: "x86_64",
            os_version: "10.0.19045".into(),
        }
    }

    fn only(os: &str) -> Vec<Rule> {
        vec![Rule {
            action: "allow".into(),
            os: Some(Os {
                name: Some(os.into()),
                arch: None,
                version: None,
                version_range: None,
            }),
            features: None,
        }]
    }

    #[test]
    fn fabric_wins_over_vanilla_for_the_same_artifact() {
        let fabric = Library {
            name: "org.ow2.asm:asm:9.10.1".into(),
            downloads: None,
            rules: vec![],
            url: Some("https://maven.fabricmc.net/".into()),
            sha1: Some("aa".into()),
            size: Some(10),
        };
        let tasks = resolve(
            &[fabric],
            &[vanilla("org.ow2.asm:asm:9.6", vec![])],
            &windows(),
            Path::new("/libs"),
        )
        .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].url,
            "https://maven.fabricmc.net/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar"
        );
        assert_eq!(
            tasks[0].target,
            Path::new("/libs/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar")
        );
    }

    #[test]
    fn keeps_native_classifiers_apart_and_drops_other_platforms() {
        let tasks = resolve(
            &[],
            &[
                vanilla("org.lwjgl:lwjgl:3.4.3", vec![]),
                vanilla(
                    "org.lwjgl:lwjgl:3.4.3:natives-windows",
                    only(Platform::WINDOWS),
                ),
                vanilla("org.lwjgl:lwjgl:3.4.3:natives-macos", only(Platform::OSX)),
            ],
            &windows(),
            Path::new("/libs"),
        )
        .unwrap();
        let names: Vec<_> = tasks
            .iter()
            .map(|task| task.target.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["lwjgl-3.4.3.jar", "lwjgl-3.4.3-natives-windows.jar"]
        );
    }
}
