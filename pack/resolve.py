"""pack/sources.json 의 Modrinth 프로젝트를 풀어 해시를 박은 pack/manifest.json 을 쓴다.

판을 적지 않은 항목은 그 마인크래프트 판에 맞는 최신판으로 풀리되, 목록 안의 다른 모드가 판까지 지정해
요구하면 그 판을 따른다. Iris 가 Sodium 최신 beta 를 거부해 게임이 뜨지 않았던 것이 그 이유다.
결과를 커밋해 두면 launcher 는 manifest 에 적힌 파일만 받으므로, 모드 판을 올리는 일은 이 스크립트를
다시 돌려 diff 를 보는 일이다.
"""

import json
import sys
import urllib.parse
import urllib.request
from pathlib import Path

PACK = Path(__file__).resolve().parent
HEADERS = {"User-Agent": "hyperfarm-launcher pack resolver"}


def modrinth(path, **params):
    query = urllib.parse.urlencode({key: json.dumps(value) for key, value in params.items()})
    url = f"https://api.modrinth.com/v2/{path}" + (f"?{query}" if query else "")
    with urllib.request.urlopen(urllib.request.Request(url, headers=HEADERS)) as response:
        return json.loads(response.read().decode(), strict=False)


def pick(entry, loaders, game_version):
    slug = entry["slug"]
    if "version" in entry:
        versions = [v for v in modrinth(f"project/{slug}/version", loaders=loaders) if v["version_number"] == entry["version"]]
    else:
        versions = modrinth(f"project/{slug}/version", loaders=loaders, game_versions=[game_version])
    if not versions:
        sys.exit(f"{slug}: no {loaders} version for {game_version}")
    return versions[0]


def pinned_by_dependents(versions):
    by_project = {version["project_id"]: slug for slug, version in versions.items()}
    pins = {}
    for slug, version in versions.items():
        for dependency in version["dependencies"]:
            target = by_project.get(dependency.get("project_id"))
            if dependency["dependency_type"] == "required" and target and dependency.get("version_id"):
                previous = pins.setdefault(target, (dependency["version_id"], slug))
                if previous[0] != dependency["version_id"]:
                    sys.exit(f"{target}: {previous[1]} and {slug} require different versions")
    return pins


def file_entry(slug, version):
    primary = next((f for f in version["files"] if f.get("primary")), version["files"][0])
    return {
        "id": slug,
        "fileName": primary["filename"],
        "url": primary["url"],
        "sha512": primary["hashes"]["sha512"],
        "size": primary["size"],
    }


def resolve(entries, loaders, game_version):
    versions = {entry["slug"]: pick(entry, loaders, game_version) for entry in entries}
    explicit = {entry["slug"] for entry in entries if "version" in entry}
    for slug, (version_id, dependent) in pinned_by_dependents(versions).items():
        if slug not in explicit and versions[slug]["id"] != version_id:
            versions[slug] = modrinth(f"version/{version_id}")
            print(f"{slug}: {versions[slug]['version_number']} (required by {dependent})")
    return [file_entry(entry["slug"], versions[entry["slug"]]) for entry in entries]


def main():
    sources = json.loads((PACK / "sources.json").read_text())
    game_version = sources["minecraftVersion"]
    manifest = {
        "minecraftVersion": game_version,
        "fabricLoaderVersion": sources["fabricLoaderVersion"],
        "server": sources["server"],
        "mods": resolve(sources["mods"], ["fabric"], game_version),
        "shaderpacks": resolve(sources["shaderpacks"], ["iris"], game_version),
        "defaultOptions": sources["defaultOptions"],
    }
    (PACK / "manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
