# 하이퍼팜 launcher

[하이퍼팜](https://hyperf.kr) 웹사이트의 "게임 시작" 버튼(`hyperfarm://launch?ticket=...`)으로 켜져서 Microsoft
로그인, 마인크래프트·Fabric·모드 설치를 마치고 `hyperf.kr` 에 바로 접속시키는 프로그램이다. Tauri 2 로 만들었다.

- 토큰은 플레이어 PC 의 OS 자격 증명 저장소(macOS Keychain, Windows Credential Manager)에만 둔다.
- 웹이 넘기는 것은 일회용 launch ticket 뿐이다. launcher 는 그것을 웹 백엔드에서 `{playerId, loginHint}` 로
  바꿔, 브라우저의 Microsoft 세션에서 `prompt=none` 으로 조용히 로그인한다. ticket 을 바꾸지 못해도 실행은
  이어 간다.
- 게임 폴더는 `.minecraft` 와 따로 둔다. macOS 는 `~/Library/Application Support/HyperFarm`, Windows 는
  `%APPDATA%\HyperFarm` 이다.
- 설치할 판과 모드는 pack manifest(`pack/manifest.json`)가 정한다. 공개 모드와 쉐이더팩은 Modrinth 의 주소와
  sha512 로 플레이어 PC 가 직접 받고, 우리는 다시 올리지 않는다. manifest 에 없는 `mods/*.jar` 는 지운다.

## 받기

[GitHub Releases](https://github.com/vjh0107/hyperfarm-launcher/releases/latest) 에서 macOS(`.dmg`, Apple 실리콘·Intel)와
Windows(`-setup.exe`)용을 받는다. 같은 파일을 [하이퍼팜 접속 안내](https://hyperf.kr/guide) 에서도 받을 수 있다. 설치한
런처는 켜질 때 스스로 최신 판으로 바뀐다. Windows 판의 코드 서명은 아래 [Code signing policy](#code-signing-policy)
를 따른다.

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
`R2_ENDPOINT`, `R2_BUCKET`, `R2_ACCESS_KEY`, `R2_SECRET_KEY`. 이 환경은 main 과 `v*` 태그에서만 열린다.

macOS 앱은 서명하지 않았다. 처음 열 때 Finder 에서 앱을 우클릭해 "열기" 를 누르거나, 시스템 설정의 개인정보
보호 및 보안에서 "그래도 열기" 를 누른다. Windows 코드 서명은 아직 없어 처음 설치할 때 SmartScreen 경고가 뜬다.

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by
[SignPath Foundation](https://signpath.org/).

- Committers and reviewers: [@vjh0107](https://github.com/vjh0107)
- Approvers: [@vjh0107](https://github.com/vjh0107)

Every Windows release is built from a `v*` tag by [`release.yml`](.github/workflows/release.yml) on GitHub Actions, and each
signing request is approved by hand. See [Privacy](#privacy) for what the program sends over the network.

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

The launcher has no analytics or telemetry. The game itself connects to the HyperFarm server, whose
[privacy policy](https://hyperf.kr/privacy) applies there.

## 라이선스

코드는 [GPL-3.0](LICENSE), 로고·배경 그림·아이콘은 [CC BY 4.0](LICENSES/CC-BY-4.0.txt), 글꼴(Pretendard,
Paperlogy)은 [SIL Open Font License 1.1](LICENSES/OFL-1.1.txt) 이다. "하이퍼팜" 이름은 어느 라이선스로도 주지
않으므로 고친 빌드를 그 이름으로 배포하지 않는다. 자세한 것은 [NOTICE](NOTICE) 에 있다.
