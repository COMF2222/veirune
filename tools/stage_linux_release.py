"""Stage Linux installers from the separate Linux desktop workflow."""

import argparse
import json
from pathlib import Path
import shutil

from release_brand import asset_prefix

ROOT = Path(__file__).resolve().parent.parent


def stage(source, destination, version):
    """Copy one Linux installer to its release name. Returns the saved path."""
    suffix = ".deb" if source.suffix == ".deb" else ".AppImage"
    if source.suffix not in (".deb", ".AppImage"):
        raise ValueError("Only .deb and .AppImage installers can be published")
    if not source.is_file() or source.stat().st_size == 0:
        raise ValueError(f"Linux installer is missing or empty: {source.name}")
    destination.mkdir(parents=True, exist_ok=True)
    target = destination / f"{asset_prefix(version)}_{version}_linux-x86_64{suffix}"
    shutil.copy2(source, target)
    print(f"Staged {target.name}")
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--destination", type=Path, default=ROOT / "desktop/release")
    arguments = parser.parse_args()
    version = json.loads((ROOT / "desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["version"]
    stage(arguments.source, arguments.destination, version)


if __name__ == "__main__":
    main()
