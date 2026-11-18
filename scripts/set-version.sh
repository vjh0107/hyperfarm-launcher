#!/usr/bin/env bash
# 판은 package.json 과 src-tauri/Cargo.toml 두 곳에 있고 release.yml 이 태그와 셋을 대조한다
set -euo pipefail

version="${1:?usage: scripts/set-version.sh <x.y.z>}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "not a version: $version" >&2
    exit 1
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
sed -i.bak -E "1,/\"version\": \"[^\"]+\"/s/\"version\": \"[^\"]+\"/\"version\": \"$version\"/" package.json
sed -i.bak -E "1,/^version = \"[^\"]+\"/s/^version = \"[^\"]+\"/version = \"$version\"/" src-tauri/Cargo.toml
rm package.json.bak src-tauri/Cargo.toml.bak
(cd src-tauri && cargo update --workspace --quiet)
echo "version $version"
