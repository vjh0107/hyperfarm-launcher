use std::collections::BTreeMap;
use std::path::Path;

use super::download::move_into_place;

// hyperfarm 의 Vellum 화면이 스크롤도 잘림도 없이 들어가는 GUI 좌표 크기다
const VELLUM_WIDTH: u32 = 629;
const VELLUM_HEIGHT: u32 = 452;
// macOS(28pt)·Windows 100%(31~32px)의 제목 표시줄을 덮는 가장 작은 값이다. 모자라면 한 칸 큰 배율을 골라 최대화해도
// Vellum 이 잘리고, 늘리면 QHD(작업 영역 높이 1392)가 3 에서 2 로 떨어진다
const TITLE_BAR: f64 = 32.0;
// 26.3 의 data version. 없으면 게임이 0 판 파일로 읽어 data fixer 를 처음부터 모두 돌린다
const DATA_VERSION: u32 = 5023;
const GUI_SCALE: &str = "guiScale";
const FORCE_UNICODE_FONT: &str = "forceUnicodeFont";
const KEY_PREFIX: &str = "key_";
// 26.3 과 지금 모드 구성에서 기본으로 키가 없는 조작이다. 게임이 처음 파일을 쓰기 전이나 그 모드가 새로 들어온 첫 실행에는
// 줄이 없어 게임이 Not bound 로 두므로, 줄이 없으면 미리 F24 로 넣는다
const UNBOUND_BY_DEFAULT: [&str; 7] = [
    "key_key.smoothCamera",
    "key_key.spectatorOutlines",
    "key_key.dynamic_fps.toggle_forced",
    "key_key.dynamic_fps.toggle_disabled",
    "key_key.entityculling.toggle",
    "key_key.entityculling.toggleBoxes",
    "key_key.hyperfarm.voice.deafen",
];
const UNBOUND: &str = "key.keyboard.unknown";
const UNPRESSED_KEY: &str = "key.keyboard.f24";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkArea {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
}

pub fn gui_scale(area: WorkArea) -> u32 {
    let title_bar = (TITLE_BAR * area.scale_factor).ceil() as u32;
    let width = area.width;
    let height = area.height.saturating_sub(title_bar);
    (1..)
        .take_while(|&scale| {
            width.div_ceil(scale) >= VELLUM_WIDTH && height.div_ceil(scale) >= VELLUM_HEIGHT
        })
        .last()
        .unwrap_or(1)
}

pub async fn prepare(
    file: &Path,
    defaults: &BTreeMap<String, String>,
    work_area: Option<WorkArea>,
) {
    let existing = match tokio::fs::read(file).await {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => {
                log::warn!("Leaving {} alone; it is not UTF-8", file.display());
                return;
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            log::warn!("Could not read {}: {error}", file.display());
            return;
        }
    };
    let scale = work_area.map(gui_scale);
    let Some(text) = edit(existing.as_deref(), defaults, scale) else {
        return;
    };
    let before = existing.as_deref().and_then(|old| option(old, GUI_SCALE));
    let after = option(&text, GUI_SCALE);
    let set_scale = if after == before { None } else { after };
    match write(file, &text).await {
        Ok(()) => match (work_area, set_scale) {
            (Some(area), Some(scale)) => log::info!(
                "Set the default GUI scale in {} for a {}x{} work area at {}x to {scale}",
                file.display(),
                area.width,
                area.height,
                area.scale_factor
            ),
            _ => log::info!("Updated {}", file.display()),
        },
        Err(error) => log::warn!("Could not write {}: {error}", file.display()),
    }
}

fn option<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    contents
        .lines()
        .rev()
        .find_map(|line| entry(line).filter(|(name, _)| *name == key))
        .map(|(_, value)| value)
}

async fn write(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let partial = file.with_extension("txt.part");
    tokio::fs::write(&partial, text).await?;
    move_into_place(&partial, file).await
}

// 게임이 시작할 때마다 파일을 통째로 다시 쓰므로, 플레이어가 고른 1 이상의 배율은 그대로 두고 자동(0)일 때만 채운다
pub fn edit(
    existing: Option<&str>,
    defaults: &BTreeMap<String, String>,
    gui_scale: Option<u32>,
) -> Option<String> {
    let Some(text) = existing.filter(|text| !text.trim().is_empty()) else {
        return created(defaults, gui_scale);
    };
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();
    let mut changed = bind_unbound_keys(&mut lines, ending);
    if let Some(scale) = gui_scale
        && !value(&lines, GUI_SCALE)
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|chosen| chosen > 0)
    {
        let unicode = value(&lines, FORCE_UNICODE_FONT) == Some("true");
        let scale = even_for_unicode(scale, unicode).to_string();
        changed |= set(&mut lines, GUI_SCALE, &scale, ending);
    }
    changed.then(|| lines.concat())
}

// 26.3 은 입력을 SDL 로 받으면서 키를 정하지 않은 동작을 Fn·IME 전환 같은 알 수 없는 키에도 일으킨다(MC-311949,
// 26.4 Snapshot 2 에서 고침). 기본으로 키가 없는 시네마틱 카메라가 저절로 켜지고 모드의 끄기 키도 함께 눌린다.
// 실제 키에 묶인 동작은 일어나지 않으므로, 키보드에 거의 없는 F24 에 묶는다
fn bind_unbound_keys(lines: &mut Vec<String>, ending: &str) -> bool {
    let mut changed = false;
    for line in lines.iter_mut() {
        if let Some((name, UNBOUND)) = entry(line)
            && name.starts_with(KEY_PREFIX)
        {
            let tail = line[content(line).len()..].to_string();
            *line = format!("{name}:{UNPRESSED_KEY}{tail}");
            changed = true;
        }
    }
    for key in UNBOUND_BY_DEFAULT {
        if value(lines, key).is_none() {
            changed |= set(lines, key, UNPRESSED_KEY, ending);
        }
    }
    changed
}

fn value<'a>(lines: &'a [String], key: &str) -> Option<&'a str> {
    lines
        .iter()
        .rev()
        .find_map(|line| entry(line).filter(|(name, _)| *name == key))
        .map(|(_, value)| value)
}

fn set(lines: &mut Vec<String>, key: &str, value: &str, ending: &str) -> bool {
    let line = format!("{key}:{value}");
    let current = lines
        .iter()
        .rposition(|candidate| entry(candidate).is_some_and(|(name, _)| name == key));
    match current {
        Some(index) if content(&lines[index]) == line => return false,
        Some(index) => {
            let tail = lines[index][content(&lines[index]).len()..].to_string();
            lines[index] = format!("{line}{tail}");
        }
        None => {
            if let Some(last) = lines.last_mut()
                && !last.ends_with('\n')
            {
                last.push_str(ending);
            }
            lines.push(format!("{line}{ending}"));
        }
    }
    true
}

fn created(defaults: &BTreeMap<String, String>, gui_scale: Option<u32>) -> Option<String> {
    let mut text = format!("version:{DATA_VERSION}\n");
    for (key, value) in defaults {
        text.push_str(&format!("{key}:{value}\n"));
    }
    for key in UNBOUND_BY_DEFAULT {
        if !defaults.contains_key(key) {
            text.push_str(&format!("{key}:{UNPRESSED_KEY}\n"));
        }
    }
    if let Some(scale) = gui_scale
        && !defaults.contains_key(GUI_SCALE)
    {
        let unicode = defaults
            .get(FORCE_UNICODE_FONT)
            .is_some_and(|value| value == "true");
        text.push_str(&format!(
            "{GUI_SCALE}:{}\n",
            even_for_unicode(scale, unicode)
        ));
    }
    Some(text)
}

// forceUnicodeFont 이면 게임이 홀수 배율을 짝수로 올려 Vellum 이 넘친다. 1 은 무엇을 적어도 2 가 된다
fn even_for_unicode(scale: u32, unicode: bool) -> u32 {
    if unicode && scale > 1 && scale % 2 == 1 {
        scale - 1
    } else {
        scale
    }
}

fn content(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn entry(line: &str) -> Option<(&str, &str)> {
    content(line).split_once(':')
}

#[cfg(test)]
mod tests;
