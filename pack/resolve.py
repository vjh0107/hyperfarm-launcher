"""pack/sources.json 의 Modrinth 프로젝트와 우리가 올린 모드를 풀어 해시를 박은 pack/manifest.json 을 쓴다.

판을 적지 않은 항목은 그 마인크래프트 판에 맞는 최신판으로 풀리되, 목록 안의 다른 모드가 판까지 지정해
요구하면 그 판을 따른다. Iris 가 Sodium 최신 beta 를 거부해 게임이 뜨지 않았던 것이 그 이유다.
결과를 커밋해 두면 launcher 는 manifest 에 적힌 파일만 받으므로, 모드 판을 올리는 일은 이 스크립트를
다시 돌려 diff 를 보는 일이다.

`{"id": ..., "url": ...}` 항목은 우리가 pack.hyperf.kr 에 올린 jar 다. 내려받아 해시와 크기를 박는다.
launcher 는 파일 이름으로 바뀐 것을 가리므로, 새 판은 같은 주소를 덮어쓰지 않고 판이 든 새 이름으로 올린다.
"""

import hashlib
import json
import sys
import urllib.parse
import urllib.request
from pathlib import Path

PACK = Path(__file__).resolve().parent
HEADERS = {"User-Agent": "hyperfarm-launcher pack resolver"}
HOSTED = "https://pack.hyperf.kr/"


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


def hosted_entry(entry):
    url = entry["url"]
    if not url.startswith(HOSTED):
        sys.exit(f"{entry['id']}: our mods are served from {HOSTED}, not {url}")
    with urllib.request.urlopen(urllib.request.Request(url, headers=HEADERS)) as response:
        content = response.read()
    return {
        "id": entry["id"],
        "fileName": urllib.parse.unquote(url.rsplit("/", 1)[1]),
        "url": url,
        "sha512": hashlib.sha512(content).hexdigest(),
        "size": len(content),
    }


def resolve(entries, loaders, game_version):
    published = [entry for entry in entries if "slug" in entry]
    versions = {entry["slug"]: pick(entry, loaders, game_version) for entry in published}
    explicit = {entry["slug"] for entry in published if "version" in entry}
    for slug, (version_id, dependent) in pinned_by_dependents(versions).items():
        if slug not in explicit and versions[slug]["id"] != version_id:
            versions[slug] = modrinth(f"version/{version_id}")
            print(f"{slug}: {versions[slug]['version_number']} (required by {dependent})")
    return [hosted_entry(entry) if "url" in entry else file_entry(entry["slug"], versions[entry["slug"]]) for entry in entries]


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
    if "serverResourcePack" in sources:
        manifest["serverResourcePack"] = sources["serverResourcePack"]
    (PACK / "manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
