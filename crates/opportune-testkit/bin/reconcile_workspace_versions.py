#!/usr/bin/env python3
"""Move intra-workspace dependency requirements to the current release train."""

from __future__ import annotations

import argparse
import re
from pathlib import Path

_VERSION = re.compile(r"^(?P<major>\d+)\.(?P<minor>\d+)\.(?P<patch>\d+)$")


def requirement(version: str) -> str:
    """Return the workspace requirement for `version` and the next minor release."""
    match = _VERSION.fullmatch(version)
    if match is None:
        raise ValueError(f"not a semantic version: {version!r}")
    major = int(match["major"])
    minor = int(match["minor"])
    return f">={version}, <{major}.{minor + 1}.0"


def reconcile(root: Path, previous: str) -> bool:
    """Replace the previous release train in every crate manifest; return whether it moved."""
    current = (root / "version.txt").read_text(encoding="utf-8").strip()
    old = requirement(previous)
    new = requirement(current)
    if old == new:
        return False

    changed = False
    for manifest in sorted((root / "crates").glob("*/Cargo.toml")):
        text = manifest.read_text(encoding="utf-8")
        if old not in text:
            continue
        manifest.write_text(text.replace(old, new), encoding="utf-8")
        changed = True
    return changed


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--previous", required=True, help="version before this release")
    args = parser.parse_args()
    changed = reconcile(Path.cwd(), args.previous)
    print("workspace requirements reconciled" if changed else "workspace requirements already current")


if __name__ == "__main__":
    main()
