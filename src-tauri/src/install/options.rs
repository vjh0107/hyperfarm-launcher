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
    match write(file, &text).await {
        Ok(()) => {
            if let (Some(area), Some(scale)) = (work_area, scale) {
                log::info!(
                    "Set the default GUI scale for a {}x{} work area at {}x to {scale}",
                    area.width,
                    area.height,
                    area.scale_factor
                );
            }
        }
        Err(error) => log::warn!("Could not write {}: {error}", file.display()),
    }
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
    let scale = gui_scale?;
    let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();
    let last_index = |key: &str| {
        lines
            .iter()
            .rposition(|line| entry(line).is_some_and(|(name, _)| name == key))
    };
    let value = |index: Option<usize>| {
        index
            .and_then(|index| entry(&lines[index]))
            .map(|(_, value)| value)
    };
    let current = last_index(GUI_SCALE);
    if value(current)
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|chosen| chosen > 0)
    {
        return None;
    }
    let unicode = value(last_index(FORCE_UNICODE_FONT)) == Some("true");
    let line = format!("{GUI_SCALE}:{}", even_for_unicode(scale, unicode));
    match current {
        Some(index) => {
            let ending = lines[index][content(&lines[index]).len()..].to_string();
            lines[index] = format!("{line}{ending}");
        }
        None => {
            let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
            if let Some(last) = lines.last_mut()
                && !last.ends_with('\n')
            {
                last.push_str(ending);
            }
            lines.push(format!("{line}{ending}"));
        }
    }
    Some(lines.concat())
}

fn created(defaults: &BTreeMap<String, String>, gui_scale: Option<u32>) -> Option<String> {
    if defaults.is_empty() && gui_scale.is_none() {
        return None;
    }
    let mut text = format!("version:{DATA_VERSION}\n");
    for (key, value) in defaults {
        text.push_str(&format!("{key}:{value}\n"));
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
