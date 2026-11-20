# 기여하기

## 작업 흐름

main 에는 직접 올리지 않고 PR 로 들어옵니다. CI(`lint`, Linux·macOS·Windows `test`, `pack`)가 통과해야 머지되고,
머지는 squash 하나뿐이라 PR 제목과 설명이 main 의 커밋이 됩니다. 제목은 무엇이 달라지는지를 한 줄로 쓰고,
설명은 템플릿의 절을 채웁니다.

PR 에는 릴리스 노트의 분류가 되는 라벨을 하나 붙입니다 — `enhancement`, `bug`, `pack`(모드 구성),
`dependencies`. 노트에 남길 것이 없으면 `skip-changelog` 입니다.

## 리뷰

PR 의 새 head 마다 `pr-review.yml` 이 [CLAUDE.md](CLAUDE.md) 의 `## PR 리뷰` 를 기준으로 리뷰해 지적을 줄에 달고, 상태
댓글 하나를 고쳐 씁니다. 두 번째부터는 지난 리뷰 뒤로 바뀐 파일만 보고, 그때 나온 low 는 상태 댓글의 참고에만
적습니다. draft 에서는 돌지 않고 Ready 로 바꾸면 돕니다. draft 를 미리 보려면 Actions 의 **PR review** 를 PR
번호로 손으로 돌립니다. `dry-run` 으로 돌리면 PR 에 아무것도 쓰지 않고 올리려던 것과 사용량을 아티팩트에
남깁니다.

리뷰는 필수 검사가 아니어서 실패해도 머지를 막지 않습니다. 대신 main ruleset 이 스레드 resolve 를 요구하므로, 달린
지적에는 반영했으면 `반영 <SHA>`, 고치지 않으면 `고치지 않는다: <이유>` 를 답으로 달고 resolve 합니다. 다음 리뷰가
그 답을 읽고 되묻지 않습니다. 리뷰가 끝나기 전에 머지하면 지적을 받지 못하므로, 상태 댓글이 지금 head 를
가리키는지 보고 머지합니다.

저장소 secret `CLAUDE_CODE_OAUTH_TOKEN` 으로 돌고, 모델·effort·턴 상한은 저장소 변수 `REVIEW_MODEL`·
`REVIEW_EFFORT`·`REVIEW_MAX_TURNS` 로 바꿉니다. 바꾸기 전에는 같은 PR 을 `dry-run` 으로 돌려 사용량을 견줍니다.
fork 와 Dependabot 의 PR 에는 secrets 가 오지 않아 돌지 않습니다.

## 확인

```bash
pnpm build
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

설치·실행 경로를 바꿨으면 실제로 게임을 받아 창이 뜨는지 봅니다. 처음에는 약 850MB 를 받습니다.

```bash
cd src-tauri && HYPERFARM_TEST_LIVE_HOME=/tmp/hyperfarm-live cargo test installs_and_opens -- --ignored
```

## 릴리스

```bash
scripts/set-version.sh 0.2.0
```

이 변경을 PR 로 머지한 뒤 main 에서 `v0.2.0` 태그를 올리면 `release.yml` 이 판을 대조하고, macOS·Windows 를
빌드해 GitHub Release 에 붙인 뒤 `pack.hyperf.kr/launcher/releases/0.2.0/` 에 남기고 그 판을 지금 판으로 올립니다.
웹의 받기 단추(`HyperFarm.dmg`, `HyperFarm-setup.exe`)와 updater(`latest.json`)가 보는 것은 그 고정 이름이고,
설치된 런처는 다음에 켜질 때 그 판으로 바뀝니다. 게시 job 은 `release` 환경에서만 돌고, 그 환경의 secrets 는
태그와 main 에서만 읽힙니다.

## 되돌리기

Actions 의 **Rollback** 을 손으로 돌립니다.

| 되돌릴 것 | `kind` | `version` | 일어나는 일 |
|---|---|---|---|
| 런처 | `launcher` | 되돌아갈 판(`0.1.1`) | 고정 이름이 그 판을 가리키고, 설치된 런처는 다음에 켜질 때 그 판으로 내려갑니다 |
| 모드 구성 | `pack` | 그 manifest 를 게시한 main 커밋 SHA | `manifest.json` 이 그 판을 가리키고, 다음 실행에서 모드가 그 구성으로 맞춰집니다 |

되돌린 뒤에는 원인을 고친 판을 새로 내야 합니다. main 에 남은 것을 고치지 않고 다음 판을 내면 같은 문제가 다시
나갑니다.
