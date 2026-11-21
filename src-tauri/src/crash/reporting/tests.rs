use std::path::Path;

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const HOME: &str = r"C:\Users\minsu";
const NAME: &str = "Steve_kr";
const PLAYER_ID: &str = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
const XUID: &str = "2535428504476914";
const HOST: &str = "DESKTOP-MINSU01";
const TOKEN: &str = "eyJraWQiOiJhY2Nlc3MtdG9rZW4ifQ";

const JVM_REPORT: &str = r"#
# A fatal error has been detected by the Java Runtime Environment:
#
#  EXCEPTION_ACCESS_VIOLATION (0xc0000005) at pc=0x00007ffb1a2b3c4d, pid=18244, tid=9876
#
# JRE version: OpenJDK Runtime Environment (25.0.1+8)
# Problematic frame:
# C  [atio6axx.dll+0x1512fe5]
#
---------------  S U M M A R Y ------------

Command Line: -Xmx6144M net.fabricmc.loader.impl.launch.knot.KnotClient --username Steve_kr --uuid 069a79f444e94726a5befca90e38aaf5 --accessToken eyJraWQiOiJhY2Nlc3MtdG9rZW4ifQ --xuid 2535428504476914 --gameDir C:\Users\minsu\AppData\Roaming\HyperFarm

Host: DESKTOP-MINSU01, AMD Ryzen 5 5600X 6-Core Processor, 12 cores, 31G

Environment Variables:
USERNAME=minsu
PATH=C:\Users\minsu\AppData\Local\Programs
";

fn minecraft_report() -> String {
    format!(
        "---- Minecraft Crash Report ----\n\
         Time: 2026-11-20 21:13:05\n\
         Description: Unexpected error\n\
         \n\
         java.io.IOException: steve_KR could not read {HOME}\\AppData\\Roaming\\HyperFarm\\{PLAYER_ID}.dat from 203.0.113.7 or [2001:db8:85a3::8a2e:370:7334] on {HOST}\n\
         \tat knot//net.minecraft.client.Foo.bar(Foo.java:12) ~[client-intermediary.jar:?]\n\
         \tat knot//net.minecraft.client.Main.main(Main.java:3) ~[client-intermediary.jar:?]\n\
         \n\
         -- Player being rendered --\n\
         Entity's Name: {NAME}\n\
         [CHAT] <{NAME}> hello from my house\n\
         xuid: {XUID}\n"
    )
}

fn crash(directory: &Path, file: &str, text: &str, code: i32) -> Crash {
    let started = SystemTime::now() - Duration::from_secs(60);
    let report = directory.join(file);
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(&report, text).unwrap();
    Crash::detect(Some(code), directory, started)
}

fn context(options: PathBuf, dsn: Option<String>) -> LaunchContext {
    LaunchContext {
        dsn,
        display_adapters: Some("Advanced Micro Devices, Inc.\r\nIntel Corporation".into()),
        hyperfarm_mod: Some("0.1.10".into()),
        options,
        player: vec![NAME.into(), PLAYER_ID.into(), XUID.into()],
    }
}

fn machine() -> Machine {
    Machine {
        home: Some(PathBuf::from(HOME)),
        host: Some(HOST.into()),
        graphics_backend: Some("opengl".into()),
        windows: true,
    }
}

#[test]
fn reads_the_envelope_address_and_public_key_from_a_dsn() {
    let dsn = Dsn::parse("https://0123abcd@crash.hyperf.kr/1").unwrap();
    assert_eq!(
        dsn.envelope.as_str(),
        "https://crash.hyperf.kr/api/1/envelope/"
    );
    assert_eq!(dsn.key, "0123abcd");
    assert_eq!(
        Dsn::parse("https://key@crash.hyperf.kr:8443/bugsink/7/")
            .unwrap()
            .envelope
            .as_str(),
        "https://crash.hyperf.kr:8443/bugsink/api/7/envelope/"
    );
    for broken in [
        "https://crash.hyperf.kr/1",
        "https://key@crash.hyperf.kr/",
        "ftp://key@crash.hyperf.kr/1",
        "crash.hyperf.kr",
    ] {
        assert_eq!(Dsn::parse(broken), None, "{broken}");
    }
}

#[test]
fn leaves_out_who_crashed_and_everything_but_the_summary() {
    let game = tempfile::tempdir().unwrap();
    let options = game.path().join("options.txt");
    let minecraft = crash(
        game.path(),
        "crash-reports/crash-2026-11-20_21.13.05-client.txt",
        &minecraft_report(),
        1,
    );
    let jvm = tempfile::tempdir().unwrap();
    let jvm = crash(
        jvm.path(),
        "hs_err_pid18244.log",
        JVM_REPORT,
        -1_073_741_819,
    );

    for crash in [minecraft, jvm] {
        let event = event(
            &crash,
            &context(options.clone(), None),
            &machine(),
            SystemTime::now(),
        );
        let object = event.as_object().unwrap();
        for key in ["user", "server_name", "request", "breadcrumbs"] {
            assert!(!object.contains_key(key), "{key} was sent");
        }
        let sent = event.to_string().to_ascii_lowercase();
        for secret in [
            "minsu",
            "steve_kr",
            "069a79f4",
            "069a79f444e9",
            XUID,
            "203.0.113.7",
            "2001:db8",
            "hello from my house",
            TOKEN,
            "username",
            "path=",
        ] {
            assert!(
                !sent.contains(&secret.to_ascii_lowercase()),
                "{secret} was sent: {sent}"
            );
        }
    }

    let minecraft = crash(
        game.path(),
        "crash-reports/crash-2026-11-20_21.13.06-client.txt",
        &minecraft_report(),
        1,
    );
    let message = event(
        &minecraft,
        &context(options, None),
        &machine(),
        SystemTime::now(),
    )["message"]["formatted"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.starts_with("Description: Unexpected error\njava.io.IOException: <player> could not read ~\\AppData\\Roaming\\HyperFarm\\<uuid>.dat from <ip> or [<ip>] on <host>"),
        "{message}"
    );
}

#[test]
fn groups_by_exit_code_when_there_is_no_report() {
    let game = tempfile::tempdir().unwrap();
    let crash = Crash::detect(Some(-1_073_741_819), game.path(), SystemTime::now());
    let event = event(
        &crash,
        &context(game.path().join("options.txt"), None),
        &machine(),
        SystemTime::now(),
    );
    assert_eq!(event["fingerprint"], json!(["exit code", "0xC0000005"]));
    assert_eq!(event["tags"]["exit_code"], "0xC0000005");
    assert_eq!(event["tags"]["report"], "none");
    assert_eq!(
        event["message"]["formatted"],
        "The game exited with code 0xC0000005"
    );
}

#[test]
fn keeps_an_event_from_a_huge_report_under_the_bugsink_limit() {
    let game = tempfile::tempdir().unwrap();
    let huge = format!(
        "Description: Rendering overlay\n\njava.lang.IllegalStateException: {}\n\tat a.B.c(B.java:1)\n",
        "가".repeat(1024 * 1024)
    );
    let crash = crash(game.path(), "crash-reports/crash-huge-client.txt", &huge, 1);
    let event = event(
        &crash,
        &context(game.path().join("options.txt"), None),
        &machine(),
        SystemTime::now(),
    );
    assert!(envelope(&crash.event_id, &event).len() < 256 * 1024);
}

async fn bugsink() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":"x"}"#))
        .mount(&server)
        .await;
    server
}

fn dsn_of(server: &MockServer) -> String {
    let address = server.address();
    format!("http://public-key@{}:{}/42", address.ip(), address.port())
}

#[tokio::test]
async fn posts_one_envelope_whose_event_carries_the_crash_id_and_its_tags() {
    let server = bugsink().await;
    let game = tempfile::tempdir().unwrap();
    let options = game.path().join("options.txt");
    std::fs::write(
        &options,
        "version:5023\npreferredGraphicsBackend:\"opengl\"\n",
    )
    .unwrap();
    let crash = crash(
        game.path(),
        "hs_err_pid18244.log",
        JVM_REPORT,
        -1_073_741_819,
    );
    let reporter = CrashReporter::new(Http::new());
    reporter.launching(context(options, Some(dsn_of(&server))));

    reporter.report(&crash, true);
    reporter.sent(Duration::from_secs(10)).await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/api/42/envelope/");
    let auth = requests[0].headers["X-Sentry-Auth"].to_str().unwrap();
    assert!(auth.starts_with("Sentry sentry_version=7"), "{auth}");
    assert!(auth.contains("sentry_key=public-key"), "{auth}");
    let body = String::from_utf8(requests[0].body.clone()).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 3, "{body}");
    let header: Value = serde_json::from_str(lines[0]).unwrap();
    let item: Value = serde_json::from_str(lines[1]).unwrap();
    let event: Value = serde_json::from_str(lines[2]).unwrap();
    assert_eq!(header["event_id"], crash.event_id.as_str());
    assert_eq!(item["type"], "event");
    assert_eq!(item["length"], lines[2].len());
    assert_eq!(event["event_id"], crash.event_id.as_str());
    assert_eq!(event["fingerprint"], json!(["atio6axx.dll+0x1512fe5"]));
    assert_eq!(event["release"], RELEASE);
    assert_eq!(
        event["tags"]["gpu"],
        "Advanced Micro Devices, Inc. / Intel Corporation"
    );
    assert_eq!(event["tags"]["graphics_backend"], "opengl");
    assert_eq!(event["tags"]["mod"], "0.1.10");
    assert_eq!(event["tags"]["report"], "hs_err");
    assert_eq!(
        event["tags"]["exit_code"],
        if cfg!(windows) {
            "0xC0000005"
        } else {
            "-1073741819"
        }
    );
}

#[tokio::test]
async fn sends_nothing_when_turned_off_or_when_the_pack_names_no_address() {
    let server = bugsink().await;
    let game = tempfile::tempdir().unwrap();
    let crash = crash(game.path(), "hs_err_pid1.log", JVM_REPORT, 1);
    let options = game.path().join("options.txt");

    let reporter = CrashReporter::new(Http::new());
    reporter.launching(context(options.clone(), Some(dsn_of(&server))));
    reporter.report(&crash, false);
    reporter.sent(Duration::from_secs(10)).await;

    let reporter = CrashReporter::new(Http::new());
    reporter.launching(context(options, None));
    reporter.report(&crash, true);
    reporter.sent(Duration::from_secs(10)).await;

    assert!(server.received_requests().await.unwrap().is_empty());
}
