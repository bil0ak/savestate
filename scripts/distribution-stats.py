#!/usr/bin/env python3
"""Report aggregate package-download activity without client telemetry."""

import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

GITHUB_RELEASES_URL = "https://api.github.com/repos/bil0ak/savestate/releases"
CRATES_URL = "https://crates.io/api/v1/crates/savestate"
USER_AGENT = "savestate-maintainer-stats/0.1 (https://savestatecli.dev)"
BINARY_ASSET = re.compile(
    r"^savestate-v.+?-(macos|linux|windows)-.+\.(?:tar\.gz|zip)$"
)


def fetch_json(url, github_token=None):
    headers = {
        "Accept": "application/vnd.github+json"
        if url.startswith("https://api.github.com/")
        else "application/json",
        "User-Agent": USER_AGENT,
    }
    if github_token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {github_token}"

    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.load(response)
    except (urllib.error.URLError, json.JSONDecodeError) as error:
        raise RuntimeError(f"request failed for {url}: {error}") from error


def github_downloads(github_token=None):
    binary_downloads = {"macos": 0, "linux": 0, "windows": 0}
    uninstall_downloads = {"uninstall.sh": 0, "uninstall.ps1": 0}
    page = 1

    while True:
        query = urllib.parse.urlencode({"per_page": 100, "page": page})
        releases = fetch_json(f"{GITHUB_RELEASES_URL}?{query}", github_token)
        if not isinstance(releases, list):
            raise RuntimeError("GitHub returned an unexpected releases response")

        for release in releases:
            if release.get("draft"):
                continue
            for asset in release.get("assets", []):
                name = asset.get("name", "")
                downloads = int(asset.get("download_count", 0))
                match = BINARY_ASSET.match(name)
                if match:
                    binary_downloads[match.group(1)] += downloads
                elif name in uninstall_downloads:
                    uninstall_downloads[name] += downloads

        if len(releases) < 100:
            break
        page += 1

    return binary_downloads, uninstall_downloads


def cargo_downloads():
    payload = fetch_json(CRATES_URL)
    try:
        return int(payload["crate"]["downloads"])
    except (KeyError, TypeError, ValueError) as error:
        raise RuntimeError("crates.io returned an unexpected crate response") from error


def main():
    try:
        binary, uninstall = github_downloads(os.environ.get("GITHUB_TOKEN"))
        cargo = cargo_downloads()
    except RuntimeError as error:
        print(f"distribution stats: {error}", file=sys.stderr)
        return 1

    binary_total = sum(binary.values())
    uninstall_total = sum(uninstall.values())

    print("Savestate distribution activity")
    print(f"GitHub binary download attempts: {binary_total}")
    print(f"  macOS:                         {binary['macos']}")
    print(f"  Linux:                         {binary['linux']}")
    print(f"  Windows:                       {binary['windows']}")
    print(f"Cargo crate downloads:           {cargo}")
    print(f"Uninstall script requests:       {uninstall_total}")
    print(f"  macOS / Linux:                 {uninstall['uninstall.sh']}")
    print(f"  Windows:                       {uninstall['uninstall.ps1']}")
    print()
    print("Counts are aggregate download activity, not unique or confirmed installs.")
    print("Uninstall requests do not prove that removal completed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
