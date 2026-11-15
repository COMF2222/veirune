"""Require signed Windows and macOS updates and both Linux installers before publishing."""
from release_brand import asset_prefix
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Release flows stage into subdirectories (Linux packages under x86_64/).


def validate(dist, version):
    signed = [f"{asset_prefix(version)}_{version}_x64-{kind}.exe" for kind in ("setup", "update")]
    installers = []
    for arch in ("aarch64", "x86_64"):
        installers.append(f"{asset_prefix(version)}_{version}_macos-{arch}.dmg")
        signed.extend(f"{asset_prefix(version)}_{version}_macos-{arch}-{kind}.app.tar.gz" for kind in ("setup", "update"))
    # Linux packages are built by their own workflow and cannot be attached
    # later without editing a published release, so they gate publication too.
    installers.extend(f"{asset_prefix(version)}_{version}_linux-x86_64{suffix}" for suffix in (".deb", ".AppImage"))
    for name in signed + installers:
        found = next((candidate for candidate in (dist / name, dist / "x86_64" / name) if candidate.is_file()), None)
        if found is None or found.stat().st_size == 0:
            raise ValueError(f"Missing or empty release asset: {name}")
    for name in signed:
        signature = dist / (name + ".sig")
        if not signature.is_file() or not signature.read_text(encoding="utf-8").strip():
            raise ValueError(f"Missing updater signature: {name}")


def main():
    version = json.loads((ROOT / "desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["version"]
    # Validation reads the same paths that actions/download-artifact populates.
    validate(ROOT / "desktop/release", version)
    print("Windows, Apple Silicon, Intel and Linux installers and update signatures verified")


if __name__ == "__main__":
    main()
