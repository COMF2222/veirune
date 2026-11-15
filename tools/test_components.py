import gzip
import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from prepare_components import prepare
from release_manifest import manifests
import stage_release_installer
import stage_linux_release
import stage_macos_release
import prepare_macos_clap


class ComponentsTest(unittest.TestCase):
    def test_macos_preparation_creates_bundle_config_on_a_clean_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # Stop before installing dependencies; config creation must happen
            # on every preparation run, including a cached worker run.
            with patch.object(prepare_macos_clap, "ROOT", root), \
                 patch.object(prepare_macos_clap.sys, "platform", "darwin"), \
                 patch.object(prepare_macos_clap.subprocess, "run", side_effect=RuntimeError("stop before downloads")):
                with self.assertRaisesRegex(RuntimeError, "stop before downloads"):
                    prepare_macos_clap.main()
            config = root / "desktop/src-tauri/tauri.private.conf.json"
            resources = json.loads(config.read_text(encoding="utf-8"))["bundle"]["resources"]
            self.assertEqual(resources, [
                "resources/clap/worker-lite/fastcloud-clap/**/*",
                "resources/clap/model/*.json",
                "resources/clap/model/*.txt",
                "resources/clap/model/onnx/audio_model_quantized.onnx",
                "resources/clap/model/onnx/text_model_quantized.onnx",
                "resources/clap/licenses/*.txt",
            ])

    def resources(self, root):
        files = ["worker-lite/fastcloud-clap/fastcloud-clap.exe", "model/config.json",
                 "model/tokenizer.json", "model/onnx/audio_model_quantized.onnx",
                 "model/onnx/text_model_quantized.onnx"]
        for index, name in enumerate(files):
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(f"synthetic file {index}".encode())
        return files

    def test_unchanged_files_have_identical_assets_across_releases(self):
        with tempfile.TemporaryDirectory() as directory:
            root, dist = Path(directory) / "resources", Path(directory) / "release"
            self.resources(root)
            first = prepare(root, dist, "0.2.8")
            contents = {path.name: path.read_bytes() for path in dist.iterdir()}
            second = prepare(root, dist, "0.2.9")
            self.assertEqual(first["id"], second["id"])
            self.assertEqual(first["files"], second["files"])
            self.assertEqual(contents, {path.name: path.read_bytes() for path in dist.iterdir()})
            for entry in first["files"]:
                raw = gzip.decompress(contents[f"clap-{entry['sha256']}.gz"])
                self.assertEqual(hashlib.sha256(raw).hexdigest(), entry["sha256"])
                self.assertEqual(len(raw), entry["size"])

    def test_one_changed_file_creates_only_one_new_asset(self):
        with tempfile.TemporaryDirectory() as directory:
            root, dist = Path(directory) / "resources", Path(directory) / "release"
            self.resources(root)
            first = prepare(root, dist, "0.2.8")
            (root / "model/config.json").write_bytes(b"changed configuration")
            second = prepare(root, dist, "0.2.9")
            self.assertNotEqual(first["id"], second["id"])
            self.assertEqual(len(list(dist.iterdir())), len(first["files"]) + 1)
            unchanged = [a for a, b in zip(first["files"], second["files"]) if a == b]
            self.assertEqual(len(unchanged), len(first["files"]) - 1)

    def test_incomplete_model_cannot_be_published(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                prepare(Path(directory) / "missing", Path(directory) / "release", "0.2.8")

    def test_lettered_component_tag_uses_the_actual_release_name(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"GITHUB_REF_NAME": "v0.2.1a"}):
            root, dist = Path(directory) / "resources", Path(directory) / "release"
            self.resources(root)
            self.assertEqual(prepare(root, dist, "0.2.1-a", "v0.2.1a")["tag"], "v0.2.1a")

    def test_staging_keeps_both_installers_and_their_matching_signatures(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(stage_release_installer, "ROOT", Path(directory)):
            root = Path(directory)
            config = root / "desktop/src-tauri/tauri.conf.json"
            config.parent.mkdir(parents=True)
            config.write_text('{"version":"0.2.8"}')
            source = root / "desktop/src-tauri/target/release/bundle/nsis/Fastcloud_0.2.8_x64-setup.exe"
            source.parent.mkdir(parents=True)
            for kind in ("full", "light"):
                source.write_bytes(kind.encode())
                source.with_suffix(".exe.sig").write_text(f"signature-{kind}")
                stage_release_installer.stage(kind)
            for suffix, kind in (("setup", "full"), ("update", "light")):
                staged = root / f"desktop/release/Fastcloud_0.2.8_x64-{suffix}.exe"
                self.assertEqual(staged.read_bytes(), kind.encode())
                self.assertEqual(staged.with_suffix(".exe.sig").read_text(), f"signature-{kind}")

    def test_both_client_generations_get_their_own_signed_installer(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"GITHUB_REF_NAME": "v0.2.8"}):
            dist = Path(directory)
            for suffix in ("setup", "update"):
                (dist / f"Fastcloud_0.2.8_x64-{suffix}.exe").write_bytes(suffix.encode())
                (dist / f"Fastcloud_0.2.8_x64-{suffix}.exe.sig").write_text(f"signature-{suffix}")
            manifests(dist, "0.2.8")
            for filename, suffix in (("latest.json", "setup"), ("latest-light.json", "update")):
                manifest = json.loads((dist / filename).read_text())
                entry = manifest["platforms"]["windows-x86_64"]
                self.assertTrue(entry["url"].endswith(f"/v0.2.8/Fastcloud_0.2.8_x64-{suffix}.exe"))
                self.assertEqual(entry["signature"], f"signature-{suffix}")

    def test_macos_components_accept_native_worker_without_exe(self):
        with tempfile.TemporaryDirectory() as directory:
            root, dist = Path(directory) / "resources", Path(directory) / "release"
            self.resources(root)
            worker = root / "worker-lite/fastcloud-clap/fastcloud-clap.exe"
            worker.rename(worker.with_suffix(""))
            manifest = prepare(root, dist, "0.4.10")
            self.assertIn("worker-lite/fastcloud-clap/fastcloud-clap", [file["path"] for file in manifest["files"]])

    def test_windows_and_both_mac_architectures_use_matching_signed_archives(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"GITHUB_REF_NAME":"v0.4.10"}):
            dist = Path(directory)
            for suffix in ("setup","update"):
                (dist / f"Fastcloud_0.4.10_x64-{suffix}.exe").write_bytes(b"windows")
                (dist / f"Fastcloud_0.4.10_x64-{suffix}.exe.sig").write_text("windows signature")
                for arch in ("aarch64","x86_64"):
                    archive = dist / f"Fastcloud_0.4.10_macos-{arch}-{suffix}.app.tar.gz"
                    archive.write_bytes(b"macos")
                    archive.with_suffix(".gz.sig").write_text(f"{arch}-{suffix}-signature")
            manifests(dist,"0.4.10")
            for name,suffix in (("latest.json","setup"),("latest-light.json","update")):
                data=json.loads((dist/name).read_text())
                self.assertEqual(set(data["platforms"]),{"windows-x86_64","darwin-aarch64","darwin-x86_64"})
                for arch in ("aarch64","x86_64"):
                    entry=data["platforms"][f"darwin-{arch}"]
                    self.assertTrue(entry["url"].endswith(f"macos-{arch}-{suffix}.app.tar.gz"))
                    self.assertEqual(entry["signature"],f"{arch}-{suffix}-signature")

    def test_linux_staging_publishes_the_deb_and_appimage_under_release_names(self):
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(stage_linux_release, "ROOT", Path(directory)):
            root = Path(directory)
            (root / "desktop/src-tauri/tauri.conf.json").parent.mkdir(parents=True)
            (root / "desktop/src-tauri/tauri.conf.json").write_text('{"version":"0.4.32"}')
            bundle = root / "desktop/src-tauri/target/release/bundle"
            (bundle / "deb").mkdir(parents=True)
            (bundle / "appimage").mkdir(parents=True)
            deb = bundle / "deb/Veirune_0.4.32_amd64.deb"
            appimage = bundle / "appimage/Veirune_0.4.32_amd64.AppImage"
            deb.write_bytes(b"deb")
            appimage.write_bytes(b"appimage")
            destination = root / "desktop/release"
            (destination / "x86_64").mkdir(parents=True, exist_ok=True)
            target = stage_linux_release.stage(deb, destination, "0.4.32")
            stage_linux_release.stage(appimage, destination, "0.4.32")
            self.assertEqual(target.name, "Veirune_0.4.32_linux-x86_64.deb")
            self.assertEqual((destination / "Veirune_0.4.32_linux-x86_64.deb").read_bytes(), b"deb")
            self.assertEqual((destination / "Veirune_0.4.32_linux-x86_64.AppImage").read_bytes(), b"appimage")
            empty = bundle / "deb/empty.deb"
            empty.write_bytes(b"")
            with self.assertRaises(ValueError): stage_linux_release.stage(empty, destination, "0.4.32")
            # A release must never be overwritten with a different build, while
            # an identical rerun is allowed.
            conflict = destination / "veirune-test"
            conflict.mkdir()
            (conflict / "Veirune_0.4.32_linux-x86_64.deb").write_bytes(b"other")
            with self.assertRaises(ValueError): stage_linux_release.stage(deb, conflict, "0.4.32")
            stage_linux_release.stage(deb, destination, "0.4.32")

    def test_macos_staging_keeps_full_and_light_archives_and_the_dmg(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(stage_macos_release,"ROOT",Path(directory)):
            root=Path(directory)
            config=root/"desktop/src-tauri/tauri.conf.json"
            config.parent.mkdir(parents=True)
            config.write_text('{"version":"0.4.10"}')
            for arch in ("aarch64","x86_64"):
                bundle=root/f"desktop/src-tauri/target/{arch}-apple-darwin/release/bundle"
                archive=bundle/"macos/Fastcloud.app.tar.gz"
                archive.parent.mkdir(parents=True)
                (bundle/"dmg").mkdir()
                (bundle/"dmg/Fastcloud.dmg").write_bytes(b"installer")
                for kind in ("full","light"):
                    archive.write_bytes(kind.encode())
                    archive.with_suffix(".gz.sig").write_text(f"signature-{kind}")
                    stage_macos_release.stage(kind,arch)
                dist=root/"desktop/release"
                self.assertEqual((dist/f"Fastcloud_0.4.10_macos-{arch}.dmg").read_bytes(),b"installer")
                for suffix,kind in (("setup","full"),("update","light")):
                    saved=dist/f"Fastcloud_0.4.10_macos-{arch}-{suffix}.app.tar.gz"
                    self.assertEqual(saved.read_bytes(),kind.encode())
                    self.assertEqual(saved.with_suffix(".gz.sig").read_text(),f"signature-{kind}")


if __name__ == "__main__":
    unittest.main()
