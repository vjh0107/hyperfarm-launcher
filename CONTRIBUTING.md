# 기여하기

## 작업 흐름

main 에는 직접 올리지 않고 PR 로 들어옵니다. CI(`lint`, Linux·macOS·Windows `test`, `pack`)가 통과해야 머지되고,
머지는 squash 하나뿐이라 PR 제목과 설명이 main 의 커밋이 됩니다. 제목은 무엇이 달라지는지를 한 줄로 쓰고,
설명은 템플릿의 절을 채웁니다.

PR 에는 릴리스 노트의 분류가 되는 라벨을 하나 붙입니다 — `enhancement`, `bug`, `pack`(모드 구성),
`dependencies`. 노트에 남길 것이 없으면 `skip-changelog` 입니다.

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
빌드해 GitHub Release 와 `pack.hyperf.kr/launcher/` 에 게시합니다. 설치된 런처는 다음에 켜질 때 그 판으로
바뀝니다. 게시 job 은 `release` 환경에서만 돌고, 그 환경의 secrets 는 태그와 main 에서만 읽힙니다.
