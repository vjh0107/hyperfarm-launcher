"""pack/manifest.json 의 모드와 쉐이더팩을 실제로 받아 크기와 sha512 가 맞는지 본다.

check.py 는 모양만 보므로 손으로 고친 해시나 같은 이름으로 덮어쓴 jar 를 지나친다. 그런 manifest 가 게시되면
모든 플레이어의 다음 실행이 CORRUPT_DOWNLOAD 로 멈춘다. pack.yml 이 게시하기 전에 돌린다.

pack.hyperf.kr 의 파일은 launcher 처럼 `?v=<sha512 앞 16자>` 를 붙여 받는다. 쿼리 없는 주소는 엣지에 남은 옛
판이 올 수 있어, 원본이 맞는데도 틀렸다고 하거나 원본이 틀렸는데도 맞다고 한다.
"""

import hashlib
import json
import sys
import urllib.request
from pathlib import Path

PACK = Path(__file__).resolve().parent
HEADERS = {"User-Agent": "hyperfarm-launcher pack verifier"}
HOSTED = "https://pack.hyperf.kr/"


def fetch_url(entry):
    url = entry["url"]
    if not url.startswith(HOSTED):
        return url
    separator = "&" if "?" in url else "?"
    return f"{url}{separator}v={entry['sha512'][:16]}"


def digest(url):
    sha512 = hashlib.sha512()
    size = 0
    with urllib.request.urlopen(urllib.request.Request(url, headers=HEADERS), timeout=60) as response:
        while chunk := response.read(1 << 20):
            sha512.update(chunk)
            size += len(chunk)
    return size, sha512.hexdigest()


def main():
    manifest = json.loads((PACK / "manifest.json").read_text())
    failures = []
    for group in ("mods", "shaderpacks"):
        for entry in manifest[group]:
            url = fetch_url(entry)
            try:
                size, sha512 = digest(url)
            except OSError as error:
                failures.append(f"{entry['fileName']}: cannot download {url}: {error}")
                continue
            if size != entry["size"]:
                failures.append(f"{entry['fileName']}: {url} is {size} bytes but the manifest says {entry['size']}")
            elif sha512 != entry["sha512"]:
                failures.append(f"{entry['fileName']}: {url} has sha512 {sha512} but the manifest says {entry['sha512']}")
            else:
                print(f"ok: {entry['fileName']}")
    if failures:
        sys.exit("pack/manifest.json:\n" + "\n".join(failures))


if __name__ == "__main__":
    main()
