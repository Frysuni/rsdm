"""Update the stable Nix package after publishing both release archives."""

import base64
import hashlib
import json
from pathlib import Path
import re
import sys


def update_stable(tag, artifacts, manifest):
    match = re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag)
    if not match:
        raise ValueError("expected a stable release tag vMAJOR.MINOR.PATCH")

    version = tag[1:]
    current = json.loads(manifest.read_text())
    if tuple(map(int, match.groups())) < tuple(map(int, current["version"].split("."))):
        raise ValueError("refusing to downgrade the stable channel")

    hashes = {}
    for system in ("x86_64-linux", "aarch64-linux"):
        arch = system.removesuffix("-linux")
        archive = artifacts / f"rsdm-{version}-{arch}-unknown-linux-gnu.tar.gz"
        with archive.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").digest()
        hashes[system] = "sha256-" + base64.b64encode(digest).decode("ascii")

    updated = {"version": version, "hashes": hashes}
    if version == current["version"] and updated != current:
        raise ValueError("refusing to replace archives for an existing stable version")
    manifest.write_text(json.dumps(updated, indent=2) + "\n")


if __name__ == "__main__":
    update_stable(sys.argv[1], Path(sys.argv[2]), Path(__file__).with_name("stable.json"))
