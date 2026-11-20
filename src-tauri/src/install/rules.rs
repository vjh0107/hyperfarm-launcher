use std::collections::{BTreeMap, HashSet};

use regex_lite::Regex;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Platform {
    pub os_name: &'static str,
    pub arch: &'static str,
    pub os_version: String,
}

impl Platform {
    pub const OSX: &'static str = "osx";
    pub const WINDOWS: &'static str = "windows";
    pub const LINUX: &'static str = "linux";

    pub fn current() -> Self {
        let os_name = match std::env::consts::OS {
            "macos" => Self::OSX,
            "windows" => Self::WINDOWS,
            _ => Self::LINUX,
        };
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            "x86" => "x86",
            other => other,
        };
        Self {
            os_name,
            arch,
            os_version: reported_os_version(
                os_name,
                sysinfo::System::os_version().unwrap_or_default(),
            ),
        }
    }

    pub fn java_runtime_key(&self) -> Option<&'static str> {
        Some(match (self.os_name, self.arch) {
            (Self::OSX, "arm64") => "mac-os-arm64",
            (Self::OSX, "x86_64") => "mac-os",
            (Self::WINDOWS, "x86_64") => "windows-x64",
            (Self::WINDOWS, "arm64") => "windows-arm64",
            (Self::WINDOWS, "x86") => "windows-x86",
            (Self::LINUX, "x86_64") => "linux",
            (Self::LINUX, "x86") => "linux-i386",
            _ => return None,
        })
    }

    pub fn java_executable(&self) -> &'static str {
        match self.os_name {
            Self::OSX => "jre.bundle/Contents/Home/bin/java",
            Self::WINDOWS => "bin/javaw.exe",
            _ => "bin/java",
        }
    }

    pub fn classpath_separator(&self) -> &'static str {
        if self.os_name == Self::WINDOWS {
            ";"
        } else {
            ":"
        }
    }
}

// sysinfo 는 Windows 를 "10 (19045)" 로 적어, 그대로 견주면 Mojang 의 versionRange(10.0.17134)보다 작게 읽힌다.
// Mojang 은 Java 의 os.version 처럼 Windows 11 도 10.0.<빌드> 로 본다
fn reported_os_version(os_name: &str, raw: String) -> String {
    if os_name != Platform::WINDOWS {
        return raw;
    }
    let windows = Regex::new(r"^1[01] \((\d+)\)$").expect("the Windows version pattern is valid");
    match windows.captures(&raw) {
        Some(captures) => format!("10.0.{}", &captures[1]),
        None => raw,
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Rule {
    pub action: String,
    pub os: Option<Os>,
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Os {
    pub name: Option<String>,
    pub arch: Option<String>,
    pub version: Option<String>,
    #[serde(rename = "versionRange")]
    pub version_range: Option<VersionRange>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct VersionRange {
    pub min: Option<String>,
    pub max: Option<String>,
}

pub fn allows(rules: &[Rule], platform: &Platform, features: &HashSet<&str>) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        if rule.matches(platform, features) {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

impl Rule {
    fn matches(&self, platform: &Platform, enabled: &HashSet<&str>) -> bool {
        if let Some(os) = &self.os
            && !os.matches(platform)
        {
            return false;
        }
        if let Some(features) = &self.features {
            return features
                .iter()
                .all(|(name, wanted)| *wanted == enabled.contains(name.as_str()));
        }
        true
    }
}

impl Os {
    fn matches(&self, platform: &Platform) -> bool {
        self.name
            .as_deref()
            .is_none_or(|name| name == platform.os_name)
            && self
                .arch
                .as_deref()
                .is_none_or(|arch| arch == platform.arch)
            && self.version.as_deref().is_none_or(|pattern| {
                Regex::new(pattern).is_ok_and(|regex| regex.is_match(&platform.os_version))
            })
            && self
                .version_range
                .as_ref()
                .is_none_or(|range| range.contains(&platform.os_version))
    }
}

impl VersionRange {
    fn contains(&self, version: &str) -> bool {
        self.min
            .as_deref()
            .is_none_or(|min| compare_versions(version, min).is_ge())
            && self
                .max
                .as_deref()
                .is_none_or(|max| compare_versions(version, max).is_lt())
    }
}

pub fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let parse = |text: &str| -> Vec<u64> {
        text.split('.')
            .map(|part| {
                part.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (a, b) = (parse(left), parse(right));
    for i in 0..a.len().max(b.len()) {
        let ordering = a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0));
        if ordering.is_ne() {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
}

#[derive(Clone, Debug, PartialEq)]
pub struct Argument {
    pub rules: Vec<Rule>,
    pub values: Vec<String>,
}

#[derive(Deserialize)]
struct Conditional {
    #[serde(default)]
    rules: Vec<Rule>,
    value: Value,
}

pub fn parse_arguments(raw: Option<&Vec<Value>>) -> Vec<Argument> {
    raw.map(|values| {
        values
            .iter()
            .filter_map(|value| match value {
                Value::String(text) => Some(Argument {
                    rules: vec![],
                    values: vec![text.clone()],
                }),
                other => serde_json::from_value::<Conditional>(other.clone())
                    .ok()
                    .map(|conditional| Argument {
                        rules: conditional.rules,
                        values: match conditional.value {
                            Value::Array(items) => items
                                .iter()
                                .filter_map(|item| item.as_str().map(str::to_string))
                                .collect(),
                            Value::String(text) => vec![text],
                            _ => vec![],
                        },
                    }),
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn resolve_arguments(
    arguments: &[Argument],
    platform: &Platform,
    features: &HashSet<&str>,
) -> Vec<String> {
    arguments
        .iter()
        .filter(|argument| allows(&argument.rules, platform, features))
        .flat_map(|argument| argument.values.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform(os_name: &'static str, arch: &'static str, version: &str) -> Platform {
        Platform {
            os_name,
            arch,
            os_version: version.into(),
        }
    }

    fn os_rule(action: &str, name: &str) -> Rule {
        Rule {
            action: action.into(),
            os: Some(Os {
                name: Some(name.into()),
                arch: None,
                version: None,
                version_range: None,
            }),
            features: None,
        }
    }

    #[test]
    fn allows_only_the_named_operating_system() {
        let rules = [os_rule("allow", Platform::OSX)];
        assert!(allows(
            &rules,
            &platform(Platform::OSX, "arm64", "15.1"),
            &HashSet::new()
        ));
        assert!(!allows(
            &rules,
            &platform(Platform::WINDOWS, "x86_64", "10.0"),
            &HashSet::new()
        ));
    }

    #[test]
    fn later_rules_override_earlier_ones() {
        let rules = [
            Rule {
                action: "allow".into(),
                os: None,
                features: None,
            },
            os_rule("disallow", Platform::OSX),
        ];
        assert!(allows(
            &[],
            &platform(Platform::OSX, "arm64", "15"),
            &HashSet::new()
        ));
        assert!(!allows(
            &rules,
            &platform(Platform::OSX, "arm64", "15"),
            &HashSet::new()
        ));
        assert!(allows(
            &rules,
            &platform(Platform::WINDOWS, "x86_64", "10.0"),
            &HashSet::new()
        ));
    }

    #[test]
    fn feature_rules_follow_enabled_features() {
        let rules = [Rule {
            action: "allow".into(),
            os: None,
            features: Some(BTreeMap::from([("is_demo_user".into(), true)])),
        }];
        let mac = platform(Platform::OSX, "arm64", "15");
        assert!(!allows(
            &rules,
            &mac,
            &HashSet::from(["is_quick_play_multiplayer"])
        ));
        assert!(allows(&rules, &mac, &HashSet::from(["is_demo_user"])));
    }

    #[test]
    fn version_range_includes_min_and_excludes_max() {
        let range = |min: Option<&str>, max: Option<&str>| Rule {
            action: "allow".into(),
            os: Some(Os {
                name: Some(Platform::WINDOWS.into()),
                arch: None,
                version: None,
                version_range: Some(VersionRange {
                    min: min.map(Into::into),
                    max: max.map(Into::into),
                }),
            }),
            features: None,
        };
        let modern = [range(Some("10.0.17134"), None)];
        let legacy = [range(None, Some("10.0.17134"))];
        let none = HashSet::new();
        let windows = |sysinfo: &str| {
            platform(
                Platform::WINDOWS,
                "x86_64",
                &reported_os_version(Platform::WINDOWS, sysinfo.into()),
            )
        };
        assert!(allows(&modern, &windows("10 (19045)"), &none));
        assert!(allows(&modern, &windows("11 (22631)"), &none));
        assert!(!allows(&modern, &windows("10 (16299)"), &none));
        assert!(allows(&legacy, &windows("10 (16299)"), &none));
        assert!(!allows(&legacy, &windows("10 (17134)"), &none));
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(compare_versions("10.0.9", "10.0.17134").is_lt());
        assert!(compare_versions("10.0", "10.0.0").is_eq());
    }

    #[test]
    fn parses_plain_and_conditional_arguments() {
        let raw = serde_json::json!([
            "--username",
            { "rules": [{ "action": "allow", "features": { "is_demo_user": true } }], "value": "--demo" },
            { "rules": [{ "action": "allow", "features": { "is_quick_play_multiplayer": true } }], "value": ["--quickPlayMultiplayer", "${quickPlayMultiplayer}"] }
        ]);
        let arguments = parse_arguments(raw.as_array());
        let resolved = resolve_arguments(
            &arguments,
            &platform(Platform::OSX, "arm64", "15"),
            &HashSet::from(["is_quick_play_multiplayer"]),
        );
        assert_eq!(
            resolved,
            [
                "--username",
                "--quickPlayMultiplayer",
                "${quickPlayMultiplayer}"
            ]
        );
    }
}
