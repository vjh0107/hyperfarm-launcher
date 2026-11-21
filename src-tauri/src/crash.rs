mod fabric;
mod reporting;

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Serialize, Serializer};
use sysinfo::System;

pub use reporting::{CrashReporter, LaunchContext};

const CRASH_REPORTS: &str = "crash-reports";
const JVM_REPORT_PREFIX: &str = "hs_err_pid";
// 게임 로그는 몇 시간을 돌면 수십 MB 가 된다. 찾는 오류는 프로세스가 끝나기 직전에 쓴 것이라 끝만 읽는다
const GAME_LOG_TAIL: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportKind {
    Minecraft,
    Jvm,
    GameLog,
}

#[derive(Debug, Clone)]
struct Report {
    path: PathBuf,
    kind: ReportKind,
}

impl Report {
    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn read(&self) -> Option<String> {
        let read = match self.kind {
            ReportKind::GameLog => tail(&self.path, GAME_LOG_TAIL),
            ReportKind::Minecraft | ReportKind::Jvm => std::fs::read(&self.path),
        };
        match read {
            Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Err(error) => {
                log::warn!(
                    "Could not read the crash report {}: {error}",
                    self.path.display()
                );
                None
            }
        }
    }

    fn summary(&self) -> Vec<String> {
        self.read()
            .map(|text| summarize(self.kind, &text))
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Crash {
    code: Option<i32>,
    report: Option<Report>,
    // 보내지 못해도 이 크래시를 가리키는 id 다. 나중에 플레이어가 보고하면 같은 id 로 Bugsink 의 이벤트에 닿는다
    event_id: String,
}

impl Crash {
    pub fn detect(
        code: Option<i32>,
        game_directory: &Path,
        game_log: &Path,
        started: SystemTime,
    ) -> Self {
        let report =
            find_report(game_directory, started).or_else(|| find_loader_error(game_log, started));
        log::warn!(
            "The game crashed with code {}; report: {}",
            describe_code(code, cfg!(windows)),
            report
                .as_ref()
                .map_or_else(|| "none".into(), |report| report.path.display().to_string())
        );
        Self {
            code,
            report,
            event_id: hex::encode(rand::random::<[u8; 16]>()),
        }
    }

    pub fn report_path(&self) -> Option<&Path> {
        self.report.as_ref().map(|report| report.path.as_path())
    }

    pub fn details(&self, launcher_version: &str) -> String {
        let mut lines = vec![
            format!("런처: {launcher_version}"),
            format!("OS: {}", operating_system()),
            format!("종료 코드: {}", describe_code(self.code, cfg!(windows))),
        ];
        if let Some(report) = &self.report {
            lines.push(format!("보고서: {}", report.file_name()));
            let summary = report.summary();
            if !summary.is_empty() {
                lines.push(String::new());
                lines.extend(summary);
            }
        }
        redact_home(&lines.join("\n"), dirs::home_dir().as_deref())
    }
}

impl Serialize for Crash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct View {
            code: Option<i32>,
            hex: Option<String>,
            report: Option<String>,
        }
        View {
            code: self.code,
            hex: self.code.filter(|_| cfg!(windows)).map(hex),
            report: self.report.as_ref().map(Report::file_name),
        }
        .serialize(serializer)
    }
}

// Windows 의 종료 코드는 NTSTATUS 라 음수로 읽힌다. 0xC0000005 처럼 써야 검색으로 원인에 닿는다
fn hex(code: i32) -> String {
    format!("0x{code:08X}")
}

fn describe_code(code: Option<i32>, windows: bool) -> String {
    match code {
        None => "알 수 없음".into(),
        Some(code) if windows => format!("{code} ({})", hex(code)),
        Some(code) => code.to_string(),
    }
}

// crash-reports 는 게임이 --gameDir 에, hs_err 는 JVM 이 작업 디렉터리에 쓴다. launcher 는 둘을 같은 게임 폴더로 준다
fn find_report(game_directory: &Path, since: SystemTime) -> Option<Report> {
    let minecraft = files(&game_directory.join(CRASH_REPORTS))
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .map(|path| (path, ReportKind::Minecraft));
    let jvm = files(game_directory)
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy())
                .is_some_and(|name| name.starts_with(JVM_REPORT_PREFIX) && name.ends_with(".log"))
        })
        .map(|path| (path, ReportKind::Jvm));
    minecraft
        .chain(jvm)
        .filter_map(|(path, kind)| {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            (modified >= since).then_some((modified, Report { path, kind }))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, report)| report)
}

// 모드가 시작 단계에서 실패하면 loader 는 crash report 를 쓰지 않고 게임 로그에만 남긴다
fn find_loader_error(game_log: &Path, since: SystemTime) -> Option<Report> {
    let modified = std::fs::metadata(game_log).ok()?.modified().ok()?;
    if modified < since {
        return None;
    }
    let report = Report {
        path: game_log.to_path_buf(),
        kind: ReportKind::GameLog,
    };
    fabric::LoaderError::find(&report.read()?)?;
    Some(report)
}

fn tail(path: &Path, bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(bytes)))?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}

fn files(directory: &Path) -> impl Iterator<Item = PathBuf> + use<> {
    std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
}

fn summarize(kind: ReportKind, text: &str) -> Vec<String> {
    match kind {
        ReportKind::Minecraft => minecraft_summary(text),
        ReportKind::Jvm => jvm_summary(text),
        ReportKind::GameLog => fabric::LoaderError::find(text)
            .map(|error| error.summary())
            .unwrap_or_default(),
    }
}

fn minecraft_summary(text: &str) -> Vec<String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("Description:"));
    let Some(description) = lines.next() else {
        return Vec::new();
    };
    let exception = lines.find(|line| !line.is_empty());
    std::iter::once(description)
        .chain(exception)
        .map(str::to_string)
        .collect()
}

fn jvm_summary(text: &str) -> Vec<String> {
    let comments: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix('#'))
        .map(str::trim)
        .collect();
    let exception = comments
        .iter()
        .find(|line| line.starts_with("EXCEPTION_") || line.starts_with("SIG"));
    let frame = comments
        .iter()
        .position(|line| *line == "Problematic frame:")
        .and_then(|index| comments.get(index + 1))
        .filter(|line| !line.is_empty());
    exception
        .into_iter()
        .chain(frame)
        .map(|line| (*line).to_string())
        .collect()
}

// 같은 원인의 크래시를 한 묶음으로 센다. 주소·pid·메시지처럼 크래시마다 바뀌는 것은 넣지 않는다
fn fingerprint(kind: ReportKind, text: &str) -> Option<Vec<String>> {
    match kind {
        ReportKind::Minecraft => minecraft_fingerprint(text),
        ReportKind::Jvm => jvm_fingerprint(text),
        ReportKind::GameLog => fabric::LoaderError::find(text).map(|error| error.fingerprint()),
    }
}

fn minecraft_fingerprint(text: &str) -> Option<Vec<String>> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("Description:"))
        .skip(1)
        .skip_while(|line| line.is_empty());
    let exception = lines.next()?;
    let class = exception.split(':').next().unwrap_or(exception).trim();
    if class.is_empty() || class.contains(char::is_whitespace) {
        return None;
    }
    // 뒤에 붙는 ~[client-intermediary.jar:?] 같은 jar 이름은 판마다 바뀌어, 두면 같은 크래시가 판마다 따로 묶인다
    let frame = lines
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix("at "))
        .map(|frame| frame.find(')').map_or(frame, |end| &frame[..=end]));
    Some(
        std::iter::once(class)
            .chain(frame)
            .map(str::to_string)
            .collect(),
    )
}

fn jvm_fingerprint(text: &str) -> Option<Vec<String>> {
    let frame = text
        .lines()
        .filter_map(|line| line.strip_prefix('#'))
        .map(str::trim)
        .skip_while(|line| *line != "Problematic frame:")
        .nth(1)?;
    let module = frame.split_once('[')?.1.split_once(']')?.0;
    let (name, offset) = module.split_once('+')?;
    (!name.is_empty() && !name.starts_with("0x") && offset.starts_with("0x"))
        .then(|| vec![module.to_string()])
}

fn operating_system() -> String {
    let name = System::long_os_version().unwrap_or_else(|_| std::env::consts::OS.to_string());
    match System::kernel_version() {
        Ok(kernel) => format!("{name} ({kernel}, {})", std::env::consts::ARCH),
        Err(_) => format!("{name} ({})", std::env::consts::ARCH),
    }
}

// 예외 메시지에 든 경로가 홈 디렉터리째 나오면 Windows 계정 이름이 함께 붙여 넣어진다
fn redact_home(text: &str, home: Option<&Path>) -> String {
    let Some(home) = home.map(|home| home.to_string_lossy().into_owned()) else {
        return text.to_string();
    };
    text.replace(&home, "~")
        .replace(&home.replace('\\', "/"), "~")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn write_at(path: &Path, modified: SystemTime) {
        write_with(path, "", modified);
    }

    fn write_with(path: &Path, contents: &str, modified: SystemTime) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[test]
    fn picks_the_newest_report_written_after_the_game_started() {
        let game = tempfile::tempdir().unwrap();
        let started = SystemTime::now() - Duration::from_secs(600);
        let reports = game.path().join(CRASH_REPORTS);
        write_at(
            &reports.join("crash-old-client.txt"),
            started - Duration::from_secs(60),
        );
        write_at(
            &game.path().join("hs_err_pid1.log"),
            started - Duration::from_secs(30),
        );
        assert!(find_report(game.path(), started).is_none());

        write_at(
            &reports.join("crash-new-client.txt"),
            started + Duration::from_secs(10),
        );
        write_at(
            &game.path().join("hs_err_pid2.log"),
            started + Duration::from_secs(20),
        );
        write_at(
            &reports.join("crash-new-client.log"),
            started + Duration::from_secs(30),
        );
        let report = find_report(game.path(), started).unwrap();
        assert_eq!(report.file_name(), "hs_err_pid2.log");
        assert_eq!(report.kind, ReportKind::Jvm);
    }

    #[test]
    fn falls_back_to_the_game_log_when_a_mod_crashed_on_startup() {
        let game = tempfile::tempdir().unwrap();
        let started = SystemTime::now() - Duration::from_secs(600);
        let log = game.path().join("logs").join("launcher-game.log");
        let startup_crash = include_str!("crash/fabric/startup-crash.xml");
        write_with(&log, startup_crash, started - Duration::from_secs(60));
        assert!(
            Crash::detect(Some(1), game.path(), &log, started)
                .report
                .is_none()
        );

        write_with(&log, startup_crash, started + Duration::from_secs(5));
        let crash = Crash::detect(Some(1), game.path(), &log, started);
        assert_eq!(
            serde_json::to_value(&crash).unwrap()["report"],
            "launcher-game.log"
        );
        let details = crash.details("0.1.14");
        assert!(
            details.contains("from mod qa_failmod\nmod: qa_failmod"),
            "{details}"
        );
        assert!(!details.contains("secret chat"), "{details}");

        write_at(
            &game.path().join(CRASH_REPORTS).join("crash-client.txt"),
            started + Duration::from_secs(1),
        );
        assert_eq!(
            Crash::detect(Some(1), game.path(), &log, started)
                .report
                .unwrap()
                .kind,
            ReportKind::Minecraft
        );
    }

    #[test]
    fn summarizes_a_minecraft_crash_report_by_its_description_and_first_exception() {
        let report = "---- Minecraft Crash Report ----\n// Who set us up the TNT?\n\nTime: 2026-11-20 21:13:05\nDescription: Rendering overlay\n\njava.lang.IllegalStateException: Atlas is too big\n\tat knot//net.minecraft.client.Foo.bar(Foo.java:1)\n\nCaused by: java.lang.RuntimeException: inner\n";
        assert_eq!(
            summarize(ReportKind::Minecraft, report),
            [
                "Description: Rendering overlay",
                "java.lang.IllegalStateException: Atlas is too big",
            ]
        );
        assert!(summarize(ReportKind::Minecraft, "no description here").is_empty());
    }

    #[test]
    fn summarizes_a_jvm_fatal_error_log_by_its_exception_and_problematic_frame() {
        let report = "#\n# A fatal error has been detected by the Java Runtime Environment:\n#\n#  EXCEPTION_ACCESS_VIOLATION (0xc0000005) at pc=0x00007ffb1a2b3c4d, pid=18244, tid=9876\n#\n# JRE version: OpenJDK Runtime Environment (25.0.1+8)\n# Problematic frame:\n# C  [atio6axx.dll+0x1a2b3c]\n#\n";
        assert_eq!(
            summarize(ReportKind::Jvm, report),
            [
                "EXCEPTION_ACCESS_VIOLATION (0xc0000005) at pc=0x00007ffb1a2b3c4d, pid=18244, tid=9876",
                "C  [atio6axx.dll+0x1a2b3c]",
            ]
        );
    }

    #[test]
    fn groups_a_minecraft_crash_by_its_exception_class_and_first_frame() {
        let report = "Description: Rendering overlay\n\njava.lang.IllegalStateException: Atlas 4096x8192 is too big\n\tat knot//net.minecraft.client.Foo.bar(Foo.java:12) ~[client-intermediary-26.3.jar:?]\n\tat knot//net.minecraft.client.Main.main(Main.java:3)\n";
        assert_eq!(
            fingerprint(ReportKind::Minecraft, report).unwrap(),
            [
                "java.lang.IllegalStateException",
                "knot//net.minecraft.client.Foo.bar(Foo.java:12)",
            ]
        );
        let bare = "Description: Ticking entity\n\njava.lang.NullPointerException\n\n-- Head --\n\tat a.B.c(B.java:1)\n";
        assert_eq!(
            fingerprint(ReportKind::Minecraft, bare).unwrap(),
            ["java.lang.NullPointerException"]
        );
        assert!(fingerprint(ReportKind::Minecraft, "no description here").is_none());
    }

    #[test]
    fn groups_a_jvm_fatal_error_by_the_module_and_offset_of_its_problematic_frame() {
        let report = "#  EXCEPTION_ACCESS_VIOLATION (0xc0000005) at pc=0x00007ffb1a2b3c4d, pid=18244, tid=9876\n# Problematic frame:\n# C  [atio6axx.dll+0x1512fe5]\n#\n";
        assert_eq!(
            fingerprint(ReportKind::Jvm, report).unwrap(),
            ["atio6axx.dll+0x1512fe5"]
        );
        let compiled = "# Problematic frame:\n# J 4567 c2 net.minecraft.Foo.bar(I)V (120 bytes) @ 0x000001f2a3b4c5d6 [0x000001f2a3b4c400+0x00000000000001d6]\n";
        assert!(fingerprint(ReportKind::Jvm, compiled).is_none());
    }

    #[test]
    fn writes_a_negative_ntstatus_as_hex_and_a_missing_code_as_unknown() {
        assert_eq!(hex(-1_073_741_819), "0xC0000005");
        assert_eq!(
            describe_code(Some(-1_073_741_819), true),
            "-1073741819 (0xC0000005)"
        );
        assert_eq!(describe_code(Some(1), false), "1");
        assert_eq!(describe_code(None, true), "알 수 없음");
    }

    #[test]
    fn replaces_the_home_directory_in_copied_paths() {
        let text = r"at C:\Users\민수\AppData\Roaming\HyperFarm and C:/Users/민수/x";
        assert_eq!(
            redact_home(text, Some(Path::new(r"C:\Users\민수"))),
            r"at ~\AppData\Roaming\HyperFarm and ~/x"
        );
    }
}
