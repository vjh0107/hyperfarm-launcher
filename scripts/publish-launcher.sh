#!/usr/bin/env bash
# 릴리스 파일을 releases/<판>/ 에 남기고 그 판을 지금 판으로 승격한다.
# updater 는 latest.json 이 가리키는 주소에서 받으므로, 주소를 GitHub 이 아니라 판별 경로로 바꿔 둔다
set -euo pipefail

files="${1:?usage: scripts/publish-launcher.sh <release-files-dir> <version>}"
version="${2:?usage: scripts/publish-launcher.sh <release-files-dir> <version>}"
: "${R2:?}" "${ENDPOINT:?}" "${PUBLIC_BASE:?}"

scripts="$(cd "$(dirname "$0")" && pwd)"
release="releases/$version"
put() {
    aws s3 cp "$1" "$R2/$2" --endpoint-url "$ENDPOINT" --only-show-errors \
        --cache-control "public, max-age=31536000, immutable"
}

# 판별 경로는 immutable 로 올리므로 다시 빌드한 설치 파일로 덮으면 엣지와 이미 받은 사람에게는 옛 파일이 남고,
# latest.json 의 서명은 새 파일의 것이라 updater 가 서명 불일치로 받지 못한다. latest.json 을 맨 나중에 올리므로
# 그것이 있으면 그 판은 다 올라간 것이다. aws s3 ls 는 아무것도 없을 때만 1 로 끝난다
status=0
listing=$(aws s3 ls "$R2/$release/latest.json" --endpoint-url "$ENDPOINT") || status=$?
case "$status" in
    0)
        if grep -q ' latest\.json$' <<<"$listing"; then
            echo "$release is already published; release a new version, or run Rollback to promote $version again" >&2
            exit 1
        fi
        ;;
    1) ;;
    *)
        echo "cannot check whether $release is published" >&2
        exit 1
        ;;
esac

cd "$files"
dmg=(./*_universal.dmg)
setup=(./*_x64-setup.exe)
put "${dmg[0]}" "$release/HyperFarm.dmg"
put "${setup[0]}" "$release/HyperFarm-setup.exe"
put ./HyperFarm_universal.app.tar.gz "$release/HyperFarm.app.tar.gz"

# tauri-action 은 latest.json 의 notes 를 비워 둔다. 릴리스 본문(notes.md)이 있으면 그것을 실어 업데이트 화면에 보인다
notes=""
if [ -f notes.md ]; then
    notes="$(cat notes.md)"
fi
jq --arg base "$PUBLIC_BASE/$release" --arg notes "$notes" '
    .platforms |= with_entries(
        .value.url = $base + (if (.key | startswith("windows")) then "/HyperFarm-setup.exe" else "/HyperFarm.app.tar.gz" end)
    ) | if $notes != "" then .notes = $notes else . end' latest.json > latest.published.json
put latest.published.json "$release/latest.json"

"$scripts/promote.sh" launcher "$version"
