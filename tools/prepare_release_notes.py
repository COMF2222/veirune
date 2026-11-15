"""Create readable bilingual GitHub notes, also consumed by the website."""

from release_brand import asset_prefix
import argparse
import json
import re
from pathlib import Path

from check_release_version import release_tag

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "docs/release-notes.json"


def validate(data):
    if data.get("schema") != 1 or not isinstance(data.get("releases"), list):
        raise ValueError("Expected release notes schema 1")
    seen = set()
    for note in data["releases"]:
        version = note.get("version")
        if not isinstance(version, str) or version in seen:
            raise ValueError("Missing or duplicate release version")
        # Validate naming without applying the current CI tag to historical notes.
        if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[a-z])?", version):
            raise ValueError("Invalid release version")
        seen.add(version)
        for lang in ("ru", "en"):
            value = note.get(lang, {})
            strings = [value.get("title"), *(value.get("changes") or [])]
            if not isinstance(value.get("changes"), list) or not value["changes"]:
                raise ValueError(f"Missing {lang} changes for {version}")
            if any(not isinstance(s, str) or not s.strip() or len(s) > 1000 or "\n" in s or "<!--" in s or "-->" in s for s in strings):
                raise ValueError(f"Invalid {lang} text for {version}")
    return data["releases"]


def render(note, tag):
    version = note["version"]
    numbers = tuple(map(int, version.split('-')[0].split('.')))
    prefix = asset_prefix(version)
    base = f"https://github.com/COMF2222/veirune/releases/download/{tag}"
    url = f"{base}/{prefix}_{version}_x64-setup.exe"
    lines = [f"[**Скачать для Windows x64 / Download for Windows x64**]({url})", ""]
    if numbers >= (0, 4, 10):
        lines += [f"[macOS · Apple Silicon]({base}/{prefix}_{version}_macos-aarch64.dmg) · [macOS · Intel]({base}/{prefix}_{version}_macos-x86_64.dmg)", "",
                  "macOS 13+: открой DMG и перенеси Veirune в Applications. При первом запуске разреши открытие в «Конфиденциальность и безопасность».",
                  "macOS 13+: open the DMG and drag Veirune to Applications. Allow the first launch in Privacy & Security.", ""]
    if numbers >= (0, 4, 33):
        lines += [f"[Linux · AppImage]({base}/{prefix}_{version}_linux-x86_64.AppImage) · [Linux · .deb]({base}/{prefix}_{version}_linux-x86_64.deb)", "",
                  "Linux: AppImage подходит для большинства дистрибутивов (в том числе Arch и CachyOS), `.deb` — для Ubuntu 24.04 и совместимых систем.",
                  "Linux: the AppImage works on most distributions (including Arch and CachyOS); the `.deb` targets Ubuntu 24.04 and compatible systems.", ""]
    for lang, heading in (("ru", "Русский"), ("en", "English")):
        lines += [f"## {heading} — {note[lang]['title']}", ""]
        lines += [f"- {text}" for text in note[lang]["changes"]]
        lines += [""]
    lines += ["<details>", "<summary>О файлах релиза / About release assets</summary>", "",
              f"Для установки Windows нужен файл `{prefix}_*_x64-setup.exe`, для macOS — `.dmg` своей архитектуры, для Linux — `.AppImage` или `.deb`. Файлы `clap-*.gz`, подписи и манифесты используются автоматическим обновлением; скачивать их вручную не нужно.", "",
              f"For Windows, use `{prefix}_*_x64-setup.exe`; for macOS, use the `.dmg` for your architecture; for Linux, use the `.AppImage` or `.deb`. The `clap-*.gz` files, signatures and manifests are used by automatic updates; you do not need to download them manually.", "", "</details>", "",
              "<!-- fastcloud-notes:v1 " + json.dumps(note, ensure_ascii=False, separators=(",", ":")) + " -->", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    notes = validate(json.loads(SOURCE.read_text(encoding="utf-8")))
    if args.check:
        print(f"Validated {len(notes)} bilingual release descriptions")
        return
    version = json.loads((ROOT / "desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["version"]
    note = next((item for item in notes if item["version"] == version), None)
    if not note:
        raise SystemExit(f"Add Russian and English notes for {version} to docs/release-notes.json before releasing")
    target = ROOT / "desktop/release-notes.md"
    target.write_text(render(note, release_tag(version)), encoding="utf-8")
    print(f"Prepared Russian and English notes for {version}")


if __name__ == "__main__":
    main()
