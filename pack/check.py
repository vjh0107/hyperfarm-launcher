"""pack/manifest.json 이 launcher 가 읽을 수 있는 모양인지, sources.json 과 같은 판을 가리키는지 본다.

launcher 는 manifest 가 깨져 있어도 모든 플레이어에게서 "모드 구성 정보를 읽지 못했어요" 로만 멈춘다.
main 에 올라가면 곧바로 게시되므로 그 전에 여기서 막는다.
"""

import json
import re
import sys
from pathlib import Path

PACK = Path(__file__).resolve().parent
SHA512 = re.compile(r"^[0-9a-f]{128}$")


def fail(message):
    sys.exit(f"pack/manifest.json: {message}")


manifest = json.loads((PACK / "manifest.json").read_text())
sources = json.loads((PACK / "sources.json").read_text())

for key in ("serverResourcePack", "serverResourcePackUrl"):
    if key in manifest and not manifest[key].startswith("https://"):
        fail(f"{key} must be an https URL")
    if manifest.get(key) != sources.get(key):
        fail(f"{key} differs from sources.json; run pack/resolve.py")

for key in ("minecraftVersion", "fabricLoaderVersion", "server"):
    if not isinstance(manifest.get(key), str) or not manifest[key]:
        fail(f"{key} is missing")
    if key in sources and sources[key] != manifest[key]:
        fail(f"{key} is {manifest[key]} but sources.json says {sources[key]}; run pack/resolve.py")

for group in ("mods", "shaderpacks"):
    files = manifest.get(group)
    if not isinstance(files, list):
        fail(f"{group} is missing")
    if len(files) != len(sources.get(group, [])):
        fail(f"{group} has {len(files)} entries but sources.json has {len(sources.get(group, []))}; run pack/resolve.py")
    names = set()
    for entry in files:
        name = entry.get("fileName", "")
        if not name or "/" in name or "\\" in name or name.startswith("."):
            fail(f"{group} has an unsafe file name {name!r}")
        if name in names:
            fail(f"{group} lists {name} twice")
        names.add(name)
        if not entry.get("url", "").startswith("https://"):
            fail(f"{name} is not served over https")
        if not SHA512.match(entry.get("sha512", "")):
            fail(f"{name} has no sha512")
        if not isinstance(entry.get("size"), int) or entry["size"] <= 0:
            fail(f"{name} has no size")

print(f"ok: {len(manifest['mods'])} mods, {len(manifest['shaderpacks'])} shaderpacks for {manifest['minecraftVersion']}")
