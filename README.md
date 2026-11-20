# 하이퍼팜 launcher

[하이퍼팜](https://hyperf.kr) 웹사이트의 "게임 시작" 버튼(`hyperfarm://launch?ticket=...`)으로 켜져서 Microsoft
로그인, 마인크래프트·Fabric·모드 설치를 마치고 `hyperf.kr` 에 접속시키는 프로그램이다. Tauri 2 로 만들었다.

- 설치까지 마치면 "게임 시작" 버튼에서 기다린다. 그 사이 톱니바퀴로 메모리를 바꾸면 시작할 때 그대로 쓴다. 준비한 지
  한 시간이 지나 누르면 로그인과 설치 확인부터 다시 한다. 계정이 웹과 달라 "이 계정으로 시작"을 고르거나, 꺼진 게임을
  "다시 시작"하면 버튼에서 기다리지 않는다.

- 토큰은 플레이어 PC 의 OS 자격 증명 저장소(macOS Keychain, Windows Credential Manager)에만 둔다.
- 웹이 넘기는 것은 일회용 launch ticket 뿐이다. launcher 는 그것을 웹 백엔드에서 `{playerId, loginHint}` 로
  바꿔, 브라우저의 Microsoft 세션에서 `prompt=none` 으로 조용히 로그인한다. ticket 을 바꾸지 못해도 실행은
  이어 간다.
- 게임 폴더는 `.minecraft` 와 따로 둔다. macOS 는 `~/Library/Application Support/HyperFarm`, Windows 는
  `%APPDATA%\HyperFarm` 이다.
- 설치할 판과 모드는 pack manifest(`pack/manifest.json`)가 정한다. 공개 모드와 쉐이더팩은 Modrinth 의 주소와
  sha512 로 플레이어 PC 가 직접 받고, 우리는 다시 올리지 않는다. launcher 는 자기가 깐 jar 를
  `mods/.hyperfarm-installed.json` 에 적어 두고, 그 가운데 manifest 에서 빠진 것만 지운다. 플레이어가 둔 jar 와, 그 기록이
  없는 폴더의 jar 는 지우지 않는다.
- 게임을 켜기 직전에 `options.txt` 의 `guiScale` 이 없거나 자동(0)이면, 주 모니터의 작업 영역에서 창을 최대화했을 때
  서버의 Vellum 화면(GUI 좌표 629×452)이 잘리지 않는 가장 큰 배율을 넣는다. FHD 는 2, 1366×768 은 1, 4K 는 4 다.
  `forceUnicodeFont` 가 켜져 있으면 게임이 홀수를 짝수로 올리므로 한 칸 내린다. 플레이어가 고른 1 이상의 값은
  건드리지 않고, 모니터를 알지 못하면 아무것도 쓰지 않는다. 파일이 없으면 manifest 의 `defaultOptions` 와 함께 만든다.
- 같은 때에 키가 정해지지 않은(`key.keyboard.unknown`) 조작은 모두 F24 에 묶는다. 26.3 은 그런 조작을 Fn·IME 전환
  같은 알 수 없는 키에도 일으켜, 기본으로 키가 없는 시네마틱 카메라가 저절로 켜진다([MC-311949](https://bugs.mojang.com/browse/MC-311949)).
  플레이어가 실제 키를 정한 조작은 건드리지 않는다.

## 받기

[GitHub Releases](https://github.com/vjh0107/hyperfarm-launcher/releases/latest) 에서 macOS(`.dmg`, Apple 실리콘·Intel)와
Windows(`-setup.exe`)용을 받는다. 같은 파일을 [하이퍼팜 접속 안내](https://hyperf.kr/guide) 에서도 받을 수 있다. 설치한
런처는 켜질 때 스스로 최신 판으로 바뀐다.

## 구성

| 자리 | 무엇 |
|---|---|
| `src-tauri/src/auth` | Microsoft(PKCE, loopback) → Xbox Live → XSTS → Minecraft 로그인과 refresh token 보관 |
| `src-tauri/src/install` | Mojang·Fabric 메타데이터를 합쳐 클라이언트·라이브러리·에셋·Java 런타임·모드를 받는다 |
| `src-tauri/src/launch.rs` | 실행 인자를 만들고 게임 창이 뜰 때까지 기다린다 |
| `src-tauri/src/flow.rs` | ticket → 로그인 → 설치 → 실행을 잇고 화면에 상태를 보낸다 |
| `src` | 창 화면(vanilla TypeScript) |
| `pack` | 모드 구성의 원본(`sources.json`)과 그것을 푼 `manifest.json` |

## 돌리기

[mise](https://mise.jdx.dev) 가 `.mise.toml` 의 Rust·Node·pnpm 판을 맞춘다.

```bash
pnpm install
pnpm tauri dev
```

바꿀 수 있는 환경 변수:

| 변수 | 기본값 |
|---|---|
| `HYPERFARM_PACK` | `https://pack.hyperf.kr/launcher/manifest.json` (`file://` 도 받는다) |
| `HYPERFARM_WEB` | `https://hyperf.kr` |
| `HYPERFARM_HOME` | 위의 게임 폴더 |

## 모드 구성

`pack/sources.json` 에 Modrinth 프로젝트를 적고 다음을 돌려 `pack/manifest.json` 을 다시 쓴다. 판을 적지 않은
모드는 그 마인크래프트 판의 최신판으로 풀리되, 목록 안의 다른 모드가 판까지 지정해 요구하면 그 판을 따른다.
Iris 가 Sodium 최신 beta 를 거부해 게임이 뜨지 않았던 것이 그 이유다. 바꾼 뒤에는 아래의 실제 설치 테스트로
게임이 뜨는지 본다. main 에 올라가면 `pack.yml` 이 R2 에 게시한다.

```bash
python3 pack/resolve.py
```

하이퍼팜이 직접 만든 모드는 hyperfarm-mod 의 태그가 `pack.hyperf.kr/mods/` 에 판이 든 이름으로 올리고
(`hyperfarm-0.1.0.jar`), `sources.json` 의 목록에 `{"id": "hyperfarm", "url": "https://pack.hyperf.kr/mods/hyperfarm-0.1.0.jar"}`
처럼 적는다. `resolve.py` 가 내려받아 해시와 크기를 박는다. launcher 는 파일 이름으로 바뀐 것을 가리므로 같은
이름을 덮어쓰지 않는다. 게시 job 도 이미 있는 판을 다시 올리지 않는다. launcher 는 `pack.hyperf.kr` 의 파일을
`?v=<sha512 앞 16자>` 를 붙여 받아, Cloudflare 엣지에 남은 옛 판과 섞지 않는다.

Iris 는 공식 판 대신 쉐이더를 켰을 때 TTF 글자가 깨지는 것을 고친 판을 싣는다. 업스트림 26.3 브랜치(671a5c8)에
아직 머지되지 않은 [IrisShaders/Iris#3442](https://github.com/IrisShaders/Iris/pull/3442) 를 그대로 얹은 것이고, 원본은
[Myeong2Namul00/Iris `hyperfarm-ttf-fix`](https://github.com/Myeong2Namul00/Iris/tree/hyperfarm-ttf-fix) 다. LGPL-3.0 에 따라
소스는 jar 옆 `pack.hyperf.kr/launcher/mods/src/` 에도 둔다. 업스트림이 그 PR 을 담아 내면 `{"slug": "iris"}` 로 돌아가고
Sodium 의 판 고정을 걷는다 — 지금은 Modrinth 의 Iris 가 Sodium 판을 끌어오지 않으므로 직접 고정했다.

`serverResourcePackUrl` 은 운영 게임서버가 접속 때 보내는 리소스팩의 공개 주소다. launcher 는 실행할 때마다 그 주소의
ETag(없으면 Last-Modified)와 크기를 보고 바뀌었으면 받아, 클라이언트가 서버 팩을 찾는 `downloads/<팩 UUID>/<SHA-1>` 에
둔다. 게임서버 파드마다 같은 팩을 다시 올려 Last-Modified 만 바뀌는 것은 바뀐 것으로 보지 않는다. 옛 키
`serverResourcePack` 은 0.1.5 까지의 launcher 가 읽고, 엣지에 옛 팩이 남아 있으면 그 launcher 는 실행을 멈추므로 다시
쓰지 않는다. 팩 UUID 는
CraftEngine 이 정하는 대로 SHA-1 16진 문자열의 `UUID.nameUUIDFromBytes` 다. 그러면 접속할 때 클라이언트가
`Returning cached file since actual hash matches requested` 를 남기고 받지 않는다. 주소가 404 이거나 받지 못하면 서버가
보내는 것을 그대로 받는다. 게임서버 차트의 `storage.resourcepackCdn`·`resourcepackUploadPath` 를 바꾸면 이 값도 함께
바꾼다.

`pack-update.yml` 이 매주 월요일 새벽에 `sources.json` 을 다시 풀어, 바뀐 모드가 있으면 이전·새 판을 표로 적은
`pack/update` PR 을 연다. CI 는 manifest 의 모양만 보므로, 게임이 뜨고 쉐이더를 켠 화면이 깨지지 않는지는 사람이
확인한 뒤 머지한다.

## 테스트

```bash
cd src-tauri && cargo test
```

실제로 게임을 받아 Fabric 클라이언트 창이 뜨는지는 따로 켠다. 처음에는 약 850MB 를 받는다.

```bash
cd src-tauri && HYPERFARM_TEST_LIVE_HOME=/tmp/hyperfarm-live cargo test installs_and_opens -- --ignored
```

## 배포

`v*` 태그를 올리면 `release.yml` 이 태그와 판을 대조하고, macOS(universal dmg)와 Windows(NSIS, 사용자 단위
설치)를 만들어 PR 라벨로 나눈 릴리스 노트와 함께 GitHub Release 에 붙인 뒤, `pack.hyperf.kr/launcher/` 에
`HyperFarm.dmg`, `HyperFarm-setup.exe`, `latest.json` 을 올린다. launcher 는 켜질 때 `latest.json` 을 보고 새 판을
받아 스스로 바꾼다. 판은 `package.json` 과 `src-tauri/Cargo.toml` 에 있고 `scripts/set-version.sh` 가 둘을 함께
바꾼다. 작업 흐름과 릴리스 순서는 [CONTRIBUTING](CONTRIBUTING.md) 에 있다.

`release` 환경 secrets: `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`(업데이트 서명),
`MACOS_SIGNING_CERTIFICATE`(p12 base64), `MACOS_SIGNING_CERTIFICATE_PASSWORD`(macOS 앱 서명),
`R2_ENDPOINT`, `R2_BUCKET`, `R2_ACCESS_KEY`, `R2_SECRET_KEY`. 이 환경은 main 과 `v*` 태그에서만 열린다.

macOS 앱은 `HyperFarm Launcher` 자체 서명 인증서로 서명하고 notarization 은 하지 않는다. 서명이 하는 일은 TCC 가
마이크 허용을 그 인증서에 묶어 기억하게 하는 것뿐이라 Gatekeeper 경고는 그대로다. 처음 열 때 Finder 에서 앱을
우클릭해 "열기" 를 누르거나, 시스템 설정의 개인정보 보호 및 보안에서 "그래도 열기" 를 누른다. **인증서를 바꾸면
모든 플레이어가 마이크 권한을 다시 받는다** — 바꿀 때는 `smoke.yml` 의 `SIGNING_CERTIFICATE_SHA1` 도 함께 바꾼다.
Windows 코드 서명은 아직 없어 처음 설치할 때 SmartScreen 경고가 뜬다.

## Privacy

This program does not send any information to other networked systems except what is needed to sign the player in and
start the game they asked to play:

- **Microsoft, Xbox Live and Minecraft services** receive the player's sign-in to obtain a Minecraft session, as every
  Minecraft launcher does. The Microsoft refresh token is stored only in the operating system's credential store on the
  player's computer.
- **hyperf.kr** receives the one-time launch ticket from the website's "Start game" button, to learn which account the
  player signed in with on the website, and serves the player's skin head shown in the launcher window.
- **Mojang, Fabric, Modrinth and pack.hyperf.kr** serve the game, mod and update files. The launcher downloads them
  without sending account data.
- **Discord**, if it is running on the player's computer, is told over its local connection that the player is playing,
  with the start time, so that it can show this on the player's profile. Nothing is sent when Discord is not running.

The launcher has no analytics or telemetry. The game itself connects to the HyperFarm server, whose
[privacy policy](https://hyperf.kr/privacy) applies there.

## 라이선스

코드는 [GPL-3.0](LICENSE), 로고·배경 그림·아이콘은 [CC BY 4.0](LICENSES/CC-BY-4.0.txt), 글꼴(Pretendard,
Paperlogy)은 [SIL Open Font License 1.1](LICENSES/OFL-1.1.txt) 이다. "하이퍼팜" 이름은 어느 라이선스로도 주지
않으므로 고친 빌드를 그 이름으로 배포하지 않는다. 자세한 것은 [NOTICE](NOTICE) 에 있다.
