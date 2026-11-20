# CLAUDE.md

구성과 돌리는 법은 [README](README.md), 작업 흐름과 릴리스는 [CONTRIBUTING](CONTRIBUTING.md) 에 있다. 이 문서에는
코드를 읽어서는 알 수 없는 것만 적는다.

## PR 리뷰

`.github/workflows/pr-review.yml`(`scripts/pr-review.sh`)이 PR 의 새 head 마다 이 절을 기준으로 리뷰한다. 기준은
PR 의 것이 아니라 대상 브랜치의 이 파일이다.

**한국어로 쓴다.** 이 저장소의 커밋과 PR 과 문서가 전부 한국어다. 코드 식별자와 기술 용어는 영어를 유지한다.

**PR 설명이 이미 이유를 댄 선택은 되묻지 않는다.** `고르지 않은 것` 에 적힌 대안을 다시 권하지 않는다. 다만
코드가 그 설명과 어긋나면 짚는다.

### 먼저 볼 것

런처는 플레이어 PC 에서 돈다. 잘못 나간 판은 다음 실행에서 모든 플레이어에게 닿고, 되돌리는 길은 Rollback 으로
옛 판을 다시 올리는 것뿐이다. 아래는 그 경계이고 SECURITY.md 가 비공개 보고를 받는 범위와 같다.

**받은 파일을 해시 없이 쓰는 자리.** `DownloadTask` 의 `hash` 가 `None` 이면 `is_satisfied` 는 그 자리에 파일이
있기만 하면 통과시키고 받을 때도 검증하지 않는다. 내용으로 이름을 정하는 서버 리소스팩처럼 받은 뒤에 따로 보는
것이 아니면 새 다운로드는 해시와 크기를 든다. `content_addressed` 는 경로가 곧 해시인 에셋만을 위한 것이라,
다른 파일에 켜면 크기만 같은 깨진 파일이 그대로 남는다.

**`pack/manifest.json` 이 바뀐 자리.** manifest 는 `pack/resolve.py` 가 `sources.json` 에서 풀어 쓰는 것이고,
main 에 올라가면 `pack.yml` 이 곧바로 게시해 모든 플레이어의 다음 실행이 그 구성으로 맞춰진다. `pack/check.py` 는
모양과 항목 수만 보고 해시가 실제 파일과 맞는지는 보지 않는다. 손으로 고친 해시·주소, `sources.json` 과 어긋난
항목, 하이퍼팜이 올린 jar 를 같은 이름으로 덮어쓴 것은 CI 를 지나 플레이어 PC 에서 `CORRUPT_DOWNLOAD` 로 터지거나
옛 파일을 그대로 쓴다. manifest 에 없는 `mods/*.jar` 는 `prune_mods` 가 지우므로, 그 폴더나 지우는 조건을 바꾸는
변경은 플레이어가 둔 파일을 지울 수 있다.

**업데이트 경로.** 설치된 런처는 자기 안에 든 `tauri.conf.json` 의 `updater.pubkey` 로 새 판의 서명을 본다. 공개키나
서명 키를 바꾸면 이미 깔린 런처는 새 판을 받지 못하고 warn 로그 한 줄만 남긴 채 옛 판으로 게임을 켠다. 아무도
모르는 채 업데이트가 영영 멈춘다. `latest.json` 의 판이 지금 판과 다르기만 하면 받으므로(`version_comparator`),
게시 순서도 뜻이 있다 — `promote.sh` 는 설치 파일을 먼저, `latest.json` 을 나중에 올린다.

**토큰과 launch ticket.** Microsoft refresh token 은 OS 자격 증명 저장소에만 둔다. 설정 파일·로그·화면 이벤트에
토큰, ticket, 로그인 코드가 실리는 변경을 짚는다. 게임 실행 인자에는 액세스 토큰이 들어 있으므로 실행 명령줄을
통째로 로그에 남기지 않는다. `hyperfarm://` 는 `launch?ticket=<43자>` 하나만 받고 나머지는 버린다. 링크로 할 수
있는 동작을 늘리거나 ticket 검사를 넓히면 웹 페이지 하나가 플레이어 PC 에서 그 동작을 일으킨다.

**Tauri 권한 경계.** 창이 부를 수 있는 것은 `capabilities/default.json` 의 권한과 `invoke_handler` 의 명령이다.
파일 경로나 URL 을 화면에서 받아 열거나 지우는 명령, 네트워크에서 받은 주소를 `opener` 로 여는 자리, CSP 를
넓히는 변경은 그 경계를 넓힌다.

**워크플로의 secrets.** 서명 키와 R2 키는 `release` 환경에만 있고 main 과 `v*` 태그에서만 열린다. 그 환경을 다른
ref 에서 열거나, `pull_request_target` 처럼 PR 의 코드를 secrets 와 함께 돌리는 트리거를 더하거나, job 권한을
넓히는 변경을 짚는다. action 은 커밋 SHA 와 판 주석으로 고정한다.

### 짝이 갈라진 자리

한쪽만 고치면 컴파일도 테스트도 지나가고 화면이나 플레이어 PC 에서 조용히 어긋나는 쌍이다. 한쪽만 바꾼 변경이면
짝을 찾아보고, 못 찾으면 그것을 짚는다.

- Rust 의 `Problem`(`SCREAMING_SNAKE_CASE`)·`LaunchEvent`·`UpdateEvent` 와 `src/events.ts` 의 타입. TypeScript 는
  Rust 가 보내는 모양을 보지 않으므로 이름이 갈리면 화면이 `undefined` 를 그리거나 그 상태를 놓친다.
- `invoke_handler` 의 명령 이름과 `src` 의 `invoke("...")` 문자열. 없는 명령은 promise 가 거절될 뿐이고 화면은
  `void invoke(...)` 로 그 거절을 버리므로 버튼이 아무 일도 하지 않는다.
- 화면이 쓰는 창 API 와 `capabilities/default.json` 의 권한. 빠지면 같은 방식으로 아무 일도 일어나지 않는다.
- CSP 의 `img-src` 와 화면이 그리는 그림의 호스트.
- `PackManifest` 의 필드와 `pack/resolve.py` 가 쓰는 키, `pack/check.py` 가 보는 키.
- `serverResourcePack` 과 hyperfarm 게임서버 차트의 `storage.resourcepackCdn`·`resourcepackUploadPath`.
- `release.yml`·`publish-launcher.sh`·`promote.sh` 의 고정 이름(`HyperFarm.dmg`, `HyperFarm-setup.exe`,
  `latest.json`)과 `tauri.conf.json` 의 updater endpoint, hyperfarm-web 의 `src/lib/launcher.ts`.
- 새로 붙는 네트워크 상대와 README 의 `## Privacy`. 이 목록 밖으로 무엇을 보내면 그 문단이 거짓이 된다.
- CI job 이름과 main ruleset 의 required status checks(`lint`, `pack`, `test (<os>)`). 이름을 바꾸면 필수 검사가
  영영 채워지지 않아 모든 PR 이 머지되지 않는다.

### 조용히 실패하는 자리

이 런처는 일부러 멈추지 않는 자리가 많다. ticket 을 바꾸지 못해도, 업데이트를 확인하지 못해도, 서버 리소스팩을
미리 두지 못해도 게임은 켠다. 그 자리마다 warn 로그가 남는 것이 유일한 자국이다. 새로 이어 가는 길을 만들면
로그 한 줄이라도 남는지 보고, `.ok()`·`let _ =`·`unwrap_or_default` 로 오류를 버리는 자리를 짚는다. 반대로
플레이어가 알아야 하는 실패를 이어 가는 쪽에 넣으면 플레이어는 이유를 모른 채 다른 증상만 본다.

플레이어에게 보이는 문구는 `LauncherError` 의 한국어 문장이고("~어요"), 로그는 영어다. 문구를 고르는 `Problem` 이
실제 원인과 다르면 화면이 엉뚱한 해결책을 안내한다.

### 운영체제마다 갈리는 자리

배포는 macOS(universal)와 Windows 뿐이고 `clippy` 는 Linux 에서만 돈다. `#[cfg(windows)]`·`#[cfg(target_os =
"macos")]` 아래의 코드는 테스트 job 이 컴파일하지만 lint 는 받지 않으므로 사람이 더 본다. 콘솔 창을 숨기는
`creation_flags`, classpath 구분자, 실행 파일 경로, `hyperfarm://` 등록(macOS 는 번들이, Windows 는
`register_all` 이 한다)이 그렇다. macOS 에서 새 OS 권한(마이크 같은)을 쓰는 기능은 그 사용 목적 문구가 번들에
함께 있어야 한다. 게임은 런처가 띄운 자식 프로세스라 권한은 런처의 번들로 매겨진다.

### 넘길 것

**도구가 이미 잡는 것.** `cargo fmt --check`, `cargo clippy -D warnings`, `pnpm build` 의 `tsc`, actionlint(워크플로의
`run:` 에 shellcheck 가 붙는다), `scripts/*.sh` 의 shellcheck, `cargo audit`·`pnpm audit`, `pack/check.py`, 그리고
`release.yml` 이 태그와 `package.json`·`Cargo.toml` 의 판을 대조하는 것이다.

**주석이 없다는 지적.** 주석은 달지 않는 것이 기본이고, 있는 `//` 는 그 줄을 바꾸면 조용히 깨지는 이유를 적은
것이다. 그것이 구현과 어긋났는지는 볼 값어치가 있고, 무엇을 하는지 풀어 쓴 주석은 짚는다.

**Dependabot 의 PR.** secrets 가 오지 않아 리뷰가 돌지 않는다. 의존성 판올림은 CI 와 audit 이 본다.

### 코멘트

발견한 줄에 인라인으로 남긴다. **Claude 를 남기지 않는다** — `Generated with`, 서명, 세션 링크 전부.
