"""Prepare the same pinned offline CLAP model with a native Linux worker."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from verify_clap_worker import verify

ROOT = Path(__file__).resolve().parent.parent
BUNDLE = ROOT / "desktop/src-tauri/resources/clap"
REVISION = "e9fd5ac1dbf3280936a7fc3ec8a020453ff184db"
HASHES = {
    "audio_model_quantized.onnx": "021dd4fb962b4ed20cc3a6730b09e0ccf9f9c49931047032118a3b64828513a6",
    "text_model_quantized.onnx": "8f9f29c5f6adee917553d4b3a70729c731c0d18b88efca0ae67c1a1fc278f3b6",
}


def prepare_bundle_config(root):
    config = root / "desktop/src-tauri/tauri.private.conf.json"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text(json.dumps({"bundle": {"resources": [
        "resources/clap/worker-lite/fastcloud-clap/**/*",
        "resources/clap/model/*.json",
        "resources/clap/model/*.txt",
        "resources/clap/model/onnx/audio_model_quantized.onnx",
        "resources/clap/model/onnx/text_model_quantized.onnx",
        "resources/clap/licenses/*.txt",
    ]}}, indent=2) + "\n", encoding="utf-8")


def main():
    if sys.platform != "linux":
        raise SystemExit("Run this preparation on a Linux runner")
    prepare_bundle_config(ROOT)
    build = Path(os.environ.get("RUNNER_TEMP", "/tmp")) / "fastcloud-clap-build"
    python = build / "venv/bin/python"
    files = ["config.json", "preprocessor_config.json", "tokenizer.json", "tokenizer_config.json",
             "special_tokens_map.json", "vocab.json", "merges.txt", *["onnx/" + name for name in HASHES]]
    worker = BUNDLE / "worker-lite/fastcloud-clap/fastcloud-clap"
    model_ready = all((BUNDLE / "model" / name).is_file() for name in files)
    if not model_ready or not worker.is_file():
        if not python.is_file():
            subprocess.run(["uv", "venv", "--python", "3.12", str(build / "venv")], check=True)
        subprocess.run(["uv", "pip", "install", "--python", str(python),
                        "numpy==1.26.4", "onnxruntime==1.22.0", "tokenizers==0.22.2", "pyinstaller==6.22.3",
                        "huggingface_hub"], check=True)
    if not model_ready:
        code = ("from huggingface_hub import snapshot_download; snapshot_download("
                "'Xenova/larger_clap_music_and_speech', revision=" + repr(REVISION) +
                ", allow_patterns=" + repr(files) + ", local_dir=" + repr(str(BUNDLE / "model")) + ")")
        subprocess.run([str(python), "-c", code], check=True)
    for name, expected in HASHES.items():
        with (BUNDLE / "model/onnx" / name).open("rb") as source:
            if hashlib.file_digest(source, "sha256").hexdigest() != expected:
                raise SystemExit(f"CLAP model hash mismatch: {name}")
    licenses = BUNDLE / "licenses"
    licenses.mkdir(parents=True, exist_ok=True)
    for name in ["NOTICE-CLAP.txt", "LICENSE-APACHE-2.0.txt"]:
        shutil.copy2(ROOT / "assets/licenses" / name, licenses / name)
    worker = BUNDLE / "worker-lite/fastcloud-clap/fastcloud-clap"
    if not worker.is_file():
        subprocess.run([str(python), "-m", "PyInstaller", "--noconfirm", "--onedir",
                        "--name", "fastcloud-clap", "--distpath", str(BUNDLE / "worker-lite"),
                        "--workpath", str(build / "work"), "--specpath", str(build),
                        str(ROOT / "desktop/clap_worker.py")], check=True)
    # Installer resources keep the executable bit, but a worker taken from a
    # downloaded artifact archive may lose it. PyInstaller already produces an
    # executable here; make the requirement explicit for cached copies.
    worker.chmod(0o755)
    # Fail packaging if native dependencies or frozen imports do not work.
    verify(worker, BUNDLE / "model")
    print("Native Linux CLAP worker and pinned models verified")


if __name__ == "__main__":
    main()
