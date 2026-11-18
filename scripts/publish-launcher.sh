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

cd "$files"
dmg=(./*_universal.dmg)
setup=(./*_x64-setup.exe)
put "${dmg[0]}" "$release/HyperFarm.dmg"
put "${setup[0]}" "$release/HyperFarm-setup.exe"
put ./HyperFarm_universal.app.tar.gz "$release/HyperFarm.app.tar.gz"

jq --arg base "$PUBLIC_BASE/$release" '
    .platforms |= with_entries(
        .value.url = $base + (if (.key | startswith("windows")) then "/HyperFarm-setup.exe" else "/HyperFarm.app.tar.gz" end)
    )' latest.json > latest.published.json
put latest.published.json "$release/latest.json"

"$scripts/promote.sh" launcher "$version"
