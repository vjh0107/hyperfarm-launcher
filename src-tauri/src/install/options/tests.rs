use std::collections::BTreeMap;

use super::*;

const WRITTEN_BY_THE_GAME: &str = "version:5023
ao:true
forceUnicodeFont:false
fov:0.0
fullscreen:false
guiScale:0
debugGuiScale:0
maxFps:120
lang:ko_kr
";

fn korean() -> BTreeMap<String, String> {
    BTreeMap::from([("lang".to_string(), "ko_kr".to_string())])
}

fn area(width: u32, height: u32, scale_factor: f64) -> WorkArea {
    WorkArea {
        width,
        height,
        scale_factor,
    }
}

#[test]
fn picks_the_largest_scale_that_fits_vellum_in_a_maximized_window() {
    let cases = [
        ("FHD, Windows 100%", area(1920, 1032, 1.0), 2),
        ("FHD, Windows 125%", area(1920, 1020, 1.25), 2),
        ("FHD 16:10", area(1920, 1152, 1.0), 2),
        ("QHD, Windows 100%", area(2560, 1392, 1.0), 2),
        ("4K, Windows 100%", area(3840, 2112, 1.0), 4),
        ("4K, Windows 150%", area(3840, 2088, 1.5), 4),
        ("1366x768", area(1366, 728, 1.0), 1),
        ("1600x900", area(1600, 852, 1.0), 1),
        ("1280x720", area(1280, 672, 1.0), 1),
        ("MacBook Air 13 Retina", area(2940, 1838, 2.0), 3),
        ("tiny", area(640, 400, 1.0), 1),
    ];
    for (name, work_area, expected) in cases {
        assert_eq!(gui_scale(work_area), expected, "{name}");
    }
}

#[test]
fn needs_every_vellum_row_and_column_after_rounding_up() {
    assert_eq!(gui_scale(area(1920, 943, 1.0)), 2);
    assert_eq!(gui_scale(area(1920, 942, 1.0)), 1);
    assert_eq!(gui_scale(area(1257, 1080, 1.0)), 2);
    assert_eq!(gui_scale(area(1256, 1080, 1.0)), 1);
}

#[test]
fn adds_the_scale_at_the_end_when_the_game_wrote_none() {
    let without = WRITTEN_BY_THE_GAME.replace("guiScale:0\n", "");

    let edited = edit(Some(&without), &korean(), Some(2)).unwrap();

    assert_eq!(edited, format!("{without}guiScale:2\n"));
}

#[test]
fn replaces_automatic_scale_in_place() {
    let edited = edit(Some(WRITTEN_BY_THE_GAME), &korean(), Some(2)).unwrap();

    assert_eq!(
        edited,
        WRITTEN_BY_THE_GAME.replace("guiScale:0\n", "guiScale:2\n")
    );
}

#[test]
fn keeps_a_scale_the_player_chose() {
    let chosen = WRITTEN_BY_THE_GAME.replace("guiScale:0", "guiScale:3");

    assert_eq!(edit(Some(&chosen), &korean(), Some(2)), None);
}

#[test]
fn treats_an_unreadable_scale_as_automatic() {
    let broken = WRITTEN_BY_THE_GAME.replace("guiScale:0", "guiScale:big");

    let edited = edit(Some(&broken), &korean(), Some(2)).unwrap();

    assert!(edited.contains("\nguiScale:2\n"));
    assert!(!edited.contains("big"));
}

#[test]
fn keeps_windows_line_endings() {
    let windows = WRITTEN_BY_THE_GAME.replace('\n', "\r\n");
    let without = windows.replace("guiScale:0\r\n", "");

    assert_eq!(
        edit(Some(&windows), &korean(), Some(2)).unwrap(),
        windows.replace("guiScale:0", "guiScale:2")
    );
    assert_eq!(
        edit(Some(&without), &korean(), Some(2)).unwrap(),
        format!("{without}guiScale:2\r\n")
    );
}

#[test]
fn creates_the_file_with_the_data_version_defaults_and_scale() {
    assert_eq!(
        edit(None, &korean(), Some(2)).as_deref(),
        Some("version:5023\nlang:ko_kr\nguiScale:2\n")
    );
    assert_eq!(
        edit(None, &korean(), None).as_deref(),
        Some("version:5023\nlang:ko_kr\n")
    );
    assert_eq!(edit(None, &BTreeMap::new(), None), None);
}

#[test]
fn leaves_an_existing_file_alone_without_a_monitor() {
    assert_eq!(edit(Some(WRITTEN_BY_THE_GAME), &korean(), None), None);
}

#[test]
fn lowers_an_odd_scale_the_unicode_font_would_round_up() {
    let unicode = WRITTEN_BY_THE_GAME.replace("forceUnicodeFont:false", "forceUnicodeFont:true");

    let edited = |scale| edit(Some(&unicode), &korean(), Some(scale)).unwrap();

    assert!(edited(3).contains("\nguiScale:2\n"));
    assert!(edited(4).contains("\nguiScale:4\n"));
    assert!(edited(1).contains("\nguiScale:1\n"));
    assert!(
        edit(Some(WRITTEN_BY_THE_GAME), &korean(), Some(3))
            .unwrap()
            .contains("\nguiScale:3\n")
    );
    let unicode_default = BTreeMap::from([("forceUnicodeFont".to_string(), "true".to_string())]);
    assert!(
        edit(None, &unicode_default, Some(3))
            .unwrap()
            .ends_with("guiScale:2\n")
    );
}

#[tokio::test]
async fn fills_the_scale_once_and_then_leaves_the_players_choice() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("options.txt");
    let fhd = Some(area(1920, 1032, 1.0));

    prepare(&file, &korean(), fhd).await;
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "version:5023\nlang:ko_kr\nguiScale:2\n"
    );

    std::fs::write(&file, "version:5023\nlang:en_us\nguiScale:3\n").unwrap();
    prepare(&file, &korean(), fhd).await;
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "version:5023\nlang:en_us\nguiScale:3\n"
    );
    assert!(!root.path().join("options.txt.part").exists());
}
