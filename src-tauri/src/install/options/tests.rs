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
key_key.smoothCamera:key.keyboard.f24
key_key.spectatorOutlines:key.keyboard.f24
key_key.entityculling.toggle:key.keyboard.f24
key_key.entityculling.toggleBoxes:key.keyboard.f24
key_key.hyperfarm.voice.deafen:key.keyboard.f24
";

fn bound() -> String {
    UNBOUND_BY_DEFAULT
        .iter()
        .map(|key| format!("{key}:key.keyboard.f24\n"))
        .collect()
}

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
        ("QHD, Windows 100%", area(2560, 1392, 1.0), 3),
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
    assert_eq!(gui_scale(area(1920, 935, 1.0)), 2);
    assert_eq!(gui_scale(area(1920, 934, 1.0)), 1);
    assert_eq!(gui_scale(area(1257, 1080, 1.0)), 2);
    assert_eq!(gui_scale(area(1256, 1080, 1.0)), 1);
}

#[test]
fn adds_the_scale_at_the_end_when_the_game_wrote_none() {
    let without = WRITTEN_BY_THE_GAME.replace("guiScale:0\n", "");

    let edited = edit(Some(&without), &korean(), Some(2), false).unwrap();

    assert_eq!(edited, format!("{without}guiScale:2\n"));
}

#[test]
fn replaces_automatic_scale_in_place() {
    let edited = edit(Some(WRITTEN_BY_THE_GAME), &korean(), Some(2), false).unwrap();

    assert_eq!(
        edited,
        WRITTEN_BY_THE_GAME.replace("guiScale:0\n", "guiScale:2\n")
    );
}

#[test]
fn keeps_a_scale_the_player_chose() {
    let chosen = WRITTEN_BY_THE_GAME.replace("guiScale:0", "guiScale:3");

    assert_eq!(edit(Some(&chosen), &korean(), Some(2), false), None);
}

#[test]
fn treats_an_unreadable_scale_as_automatic() {
    let broken = WRITTEN_BY_THE_GAME.replace("guiScale:0", "guiScale:big");

    let edited = edit(Some(&broken), &korean(), Some(2), false).unwrap();

    assert!(edited.contains("\nguiScale:2\n"));
    assert!(!edited.contains("big"));
}

#[test]
fn keeps_windows_line_endings() {
    let windows = WRITTEN_BY_THE_GAME.replace('\n', "\r\n");
    let without = windows.replace("guiScale:0\r\n", "");

    assert_eq!(
        edit(Some(&windows), &korean(), Some(2), false).unwrap(),
        windows.replace("guiScale:0", "guiScale:2")
    );
    assert_eq!(
        edit(Some(&without), &korean(), Some(2), false).unwrap(),
        format!("{without}guiScale:2\r\n")
    );
}

#[test]
fn creates_the_file_with_the_data_version_defaults_and_scale() {
    assert_eq!(
        edit(None, &korean(), Some(2), false).unwrap(),
        format!("version:5023\nlang:ko_kr\n{}guiScale:2\n", bound())
    );
    assert_eq!(
        edit(None, &korean(), None, false).unwrap(),
        format!("version:5023\nlang:ko_kr\n{}", bound())
    );
    assert_eq!(
        edit(None, &BTreeMap::new(), None, false).unwrap(),
        format!("version:5023\n{}", bound())
    );
}

#[test]
fn leaves_an_existing_file_alone_without_a_monitor() {
    assert_eq!(
        edit(Some(WRITTEN_BY_THE_GAME), &korean(), None, false),
        None
    );
}

#[test]
fn lowers_an_odd_scale_the_unicode_font_would_round_up() {
    let unicode = WRITTEN_BY_THE_GAME.replace("forceUnicodeFont:false", "forceUnicodeFont:true");

    let edited = |scale| edit(Some(&unicode), &korean(), Some(scale), false).unwrap();

    assert!(edited(3).contains("\nguiScale:2\n"));
    assert!(edited(4).contains("\nguiScale:4\n"));
    assert!(edited(1).contains("\nguiScale:1\n"));
    assert!(
        edit(Some(WRITTEN_BY_THE_GAME), &korean(), Some(3), false)
            .unwrap()
            .contains("\nguiScale:3\n")
    );
    let unicode_default = BTreeMap::from([("forceUnicodeFont".to_string(), "true".to_string())]);
    assert!(
        edit(None, &unicode_default, Some(3), false)
            .unwrap()
            .ends_with("guiScale:2\n")
    );
}

#[test]
fn binds_every_unbound_key_to_one_nobody_presses() {
    let unbound = WRITTEN_BY_THE_GAME.replace(
        "smoothCamera:key.keyboard.f24",
        "smoothCamera:key.keyboard.unknown",
    ) + "key_key.examplemod.toggle:key.keyboard.unknown\nkey_key.jump:key.keyboard.space\n";

    assert_eq!(
        edit(Some(&unbound), &korean(), None, false).unwrap(),
        format!(
            "{WRITTEN_BY_THE_GAME}key_key.examplemod.toggle:key.keyboard.f24\nkey_key.jump:key.keyboard.space\n"
        )
    );
}

#[test]
fn keeps_a_key_the_player_chose_for_the_cinematic_camera() {
    let chosen = WRITTEN_BY_THE_GAME.replace(
        "smoothCamera:key.keyboard.f24",
        "smoothCamera:key.keyboard.equal",
    );

    assert_eq!(edit(Some(&chosen), &korean(), None, false), None);
}

#[test]
fn adds_a_key_that_is_unbound_by_default_when_the_file_has_none() {
    let deafen = "key_key.hyperfarm.voice.deafen:key.keyboard.f24\n";
    let without = WRITTEN_BY_THE_GAME.replace(deafen, "");

    assert_eq!(
        edit(Some(&without), &korean(), None, false).unwrap(),
        WRITTEN_BY_THE_GAME
    );
}

#[test]
fn binds_unbound_keys_with_windows_line_endings() {
    let windows = WRITTEN_BY_THE_GAME
        .replace(
            "smoothCamera:key.keyboard.f24",
            "smoothCamera:key.keyboard.unknown",
        )
        .replace('\n', "\r\n");

    assert_eq!(
        edit(Some(&windows), &korean(), None, false).unwrap(),
        WRITTEN_BY_THE_GAME.replace('\n', "\r\n")
    );
}

#[test]
fn binds_unbound_keys_and_fills_the_scale_together() {
    let both = WRITTEN_BY_THE_GAME.replace(
        "smoothCamera:key.keyboard.f24",
        "smoothCamera:key.keyboard.unknown",
    );

    assert_eq!(
        edit(Some(&both), &korean(), Some(2), false).unwrap(),
        WRITTEN_BY_THE_GAME.replace("guiScale:0\n", "guiScale:2\n")
    );
}

#[test]
fn leaves_unbound_values_outside_key_bindings_alone() {
    let text = format!("{WRITTEN_BY_THE_GAME}lastServer:key.keyboard.unknown\n");

    assert_eq!(edit(Some(&text), &korean(), None, false), None);
}

#[tokio::test]
async fn fills_the_scale_once_and_then_leaves_the_players_choice() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("options.txt");
    let fhd = Some(area(1920, 1032, 1.0));

    prepare(&file, &korean(), fhd, false).await;
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("version:5023\nlang:ko_kr\n{}guiScale:2\n", bound())
    );

    let chosen = format!("version:5023\nlang:en_us\nguiScale:3\n{}", bound());
    std::fs::write(&file, &chosen).unwrap();
    prepare(&file, &korean(), fhd, false).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), chosen);
    assert!(!root.path().join("options.txt.part").exists());
}

#[test]
fn moves_an_amd_pc_off_the_default_backend() {
    let default = format!("{WRITTEN_BY_THE_GAME}preferredGraphicsBackend:\"default\"\n");
    let vulkan = format!("{WRITTEN_BY_THE_GAME}preferredGraphicsBackend:\"vulkan\"\n");

    assert_eq!(edit(Some(&default), &korean(), None, true).unwrap(), vulkan);
    assert_eq!(
        edit(Some(WRITTEN_BY_THE_GAME), &korean(), None, true).unwrap(),
        vulkan
    );
    assert!(
        edit(None, &korean(), None, true)
            .unwrap()
            .ends_with("preferredGraphicsBackend:\"vulkan\"\n")
    );
}

#[test]
fn keeps_a_backend_the_player_chose() {
    let opengl = format!("{WRITTEN_BY_THE_GAME}preferredGraphicsBackend:\"opengl\"\n");

    assert_eq!(edit(Some(&opengl), &korean(), None, true), None);
    assert_eq!(
        edit(Some(WRITTEN_BY_THE_GAME), &korean(), None, false),
        None
    );
}

#[test]
fn finds_an_amd_adapter_among_the_present_ones() {
    let others = "Intel Corporation\r\nNVIDIA\r\n";

    assert!(!mentions_amd(others));
    assert!(mentions_amd(&format!(
        "{others}Advanced Micro Devices, Inc.\r\n"
    )));
    assert!(!mentions_amd("ATI Technologies Inc.\r\n"));
}
