#!/usr/bin/env bash
# 두 빌드 job 이 릴리스의 latest.json 하나를 각자 고쳐 올리므로 한쪽 플랫폼이 빠질 수 있다. 그 플랫폼의 설치된
# 런처는 업데이트 확인이 실패해 warn 한 줄만 남기고 옛 판으로 게임을 켜므로, 배포하는 세 키의 주소·서명을 모두 본다
set -euo pipefail

file="${1:?usage: scripts/check-latest.sh <latest.json>}"

missing=$(jq -r '
    ["windows-x86_64", "darwin-aarch64", "darwin-x86_64"][] as $platform
    | select((.platforms[$platform].url // "") == "" or (.platforms[$platform].signature // "") == "")
    | $platform' "$file")
if [ -n "$missing" ]; then
    echo "::error::$file has no url or signature for: $(echo "$missing" | paste -sd, -)"
    exit 1
fi
