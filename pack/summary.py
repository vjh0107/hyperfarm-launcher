"""두 manifest 를 견주어 바뀐 모드와 쉐이더팩을 PR 본문용 마크다운으로 찍는다.

    python3 pack/summary.py <이전 manifest> <새 manifest>
"""

import json
import sys


def files(manifest, group):
    return {entry["id"]: entry["fileName"] for entry in manifest.get(group, [])}


def main():
    before, after = (json.loads(open(path).read()) for path in sys.argv[1:3])
    lines = []
    for key in ("minecraftVersion", "fabricLoaderVersion"):
        if before.get(key) != after.get(key):
            lines.append(f"| {key} | `{before.get(key)}` | `{after.get(key)}` |")
    for group in ("mods", "shaderpacks"):
        old, new = files(before, group), files(after, group)
        for name in sorted(old.keys() | new.keys()):
            if old.get(name) != new.get(name):
                lines.append(f"| {name} | `{old.get(name, '-')}` | `{new.get(name, '-')}` |")
    if not lines:
        print("바뀐 것이 없다.")
        return
    print("| 항목 | 이전 | 새 판 |\n|---|---|---|")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
