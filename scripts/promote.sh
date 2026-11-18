#!/usr/bin/env bash
# 판별 경로에 남긴 것을 웹 단추·updater·런처가 보는 고정 이름으로 올린다. 되돌리기도 옛 판을 다시 올리는 것이다.
#   scripts/promote.sh launcher <판>          releases/<판>/ → HyperFarm.dmg, HyperFarm-setup.exe, latest.json
#   scripts/promote.sh pack <커밋 SHA>        packs/<SHA>.json → manifest.json
set -euo pipefail

kind="${1:?usage: scripts/promote.sh launcher|pack <version>}"
version="${2:?usage: scripts/promote.sh launcher|pack <version>}"
: "${R2:?}" "${ENDPOINT:?}"

copy() {
    aws s3 cp "$R2/$1" "$R2/$2" --endpoint-url "$ENDPOINT" --only-show-errors \
        --metadata-directive REPLACE --cache-control "no-cache" --content-type "$3"
}
exists() {
    aws s3 ls "$R2/$1" --endpoint-url "$ENDPOINT" >/dev/null || {
        echo "$1 is not published" >&2
        exit 1
    }
}

case "$kind" in
    launcher)
        exists "releases/$version/latest.json"
        copy "releases/$version/HyperFarm.dmg" HyperFarm.dmg application/x-apple-diskimage
        copy "releases/$version/HyperFarm-setup.exe" HyperFarm-setup.exe application/vnd.microsoft.portable-executable
        # 설치 파일을 먼저 올려야 updater 가 아직 없는 판을 받으려다 실패하지 않는다
        copy "releases/$version/latest.json" latest.json application/json
        ;;
    pack)
        exists "packs/$version.json"
        copy "packs/$version.json" manifest.json application/json
        ;;
    *)
        echo "unknown kind: $kind" >&2
        exit 1
        ;;
esac
echo "promoted $kind $version"
