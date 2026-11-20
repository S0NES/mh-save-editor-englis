# -*- coding: utf-8 -*-
"""Packages a distributable zip: builds the release binaries and bundles them
with README/LICENSE. The result lands in dist/.

Usage: python scripts/package.py [--skip-build] [--target <triple>]
"""
import argparse
import re
import shutil
import subprocess
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--skip-build", action="store_true", help="reuse an existing release build")
    parser.add_argument("--target", default=None, help="release target triple; defaults to a host build")
    args = parser.parse_args()

    if not args.skip_build:
        print("== cargo build --release ==")
        subprocess.run(["cargo", "build", "--release"], cwd=ROOT, check=True)

    version = re.search(
        r'^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M
    ).group(1)

    target = args.target or ""
    release_dir = ROOT / "target" / (target or "release")
    if not release_dir.is_dir():
        raise SystemExit(f"missing build output directory {release_dir}")

    if "windows" in target or not target:
        binaries = ["mhrise-save-editor.exe", "mhrise-save-editor-gui.exe"]
        suffix = "win64"
    elif "linux" in target:
        binaries = ["mhrise-save-editor", "mhrise-save-editor-gui"]
        suffix = "linux-x64"
    else:
        raise SystemExit(f"unsupported release target: {target}")

    for name in binaries:
        if not (release_dir / name).is_file():
            raise SystemExit(f"missing {name}; build first")

    stage = ROOT / "dist" / f"mhrise-save-editor-v{version}-{suffix}"
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    for name in binaries:
        shutil.copy2(release_dir / name, stage / name)
    for name in ["README.md", "LICENSE"]:
        shutil.copy2(ROOT / name, stage / name)

    zip_path = ROOT / "dist" / f"mhrise-save-editor-v{version}-{suffix}.zip"
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
        for file in sorted(stage.iterdir()):
            bundle.write(file, arcname=f"{stage.name}/{file.name}")

    print(f"\n== Packaging done: {zip_path} ({zip_path.stat().st_size / 1048576:.1f} MiB) ==")
    for file in sorted(stage.iterdir()):
        print(f"  {file.name}  {file.stat().st_size / 1048576:.2f} MiB")
    print("\nOnly this zip needs to be released; everything else under target/ is a build intermediate.")


if __name__ == "__main__":
    main()
