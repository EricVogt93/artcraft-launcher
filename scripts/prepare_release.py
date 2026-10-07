#!/usr/bin/env python3
"""Validate complete native build artifacts before collecting release assets."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil

PACKAGES = {
    "linux-x86_64": [".tar.gz", ".deb", ".rpm"],
    "linux-aarch64": [".tar.gz", ".deb", ".rpm"],
    "windows-x86": [".zip", "-setup.exe"],
    "windows-x86_64": [".zip", "-setup.exe"],
    "windows-aarch64": [".zip", "-setup.exe"],
    "macos-universal": [".tar.gz", ".dmg"],
    "freebsd-x86_64": [".tar.gz"],
}


def prepare(artifacts: Path, output: Path, version: str):
    expected = {f"craftlauncher-{target}" for target in PACKAGES}
    if {p.name for p in artifacts.iterdir()} != expected:
        raise ValueError("A complete set of seven platform artifacts is required.")
    assets = {}
    for target, suffixes in PACKAGES.items():
        folder = artifacts / f"craftlauncher-{target}"
        if folder.is_symlink() or not folder.is_dir():
            raise ValueError("Invalid artifact directory.")
        files = list(folder.iterdir())
        if any(p.is_symlink() or not p.is_file() for p in files):
            raise ValueError("Release assets must be ordinary files.")
        stem = f"craftlauncher-{version}-{target}"
        sums = folder / f"{stem}-SHA256SUMS.txt"
        verified = set()
        for line in sums.read_text().splitlines():
            digest, name = line.split(maxsplit=1)
            name = name.removeprefix("*")
            if Path(name).name != name or "\\" in name or name in verified:
                raise ValueError("Invalid checksum path or duplicate entry.")
            asset = folder / name
            if asset not in files or hashlib.sha256(asset.read_bytes()).hexdigest() != digest:
                raise ValueError(f"Checksum mismatch: {name}")
            verified.add(name)
        if verified != {stem + suffix for suffix in suffixes}:
            raise ValueError(f"Missing or unexpected packages for {target}.")
        targets = ["macos-x86_64", "macos-aarch64"] if target == "macos-universal" else [target]
        manifests = {f"manifest-{native}-stable.json" for native in targets}
        if {p.name for p in files} != verified | manifests | {sums.name}:
            raise ValueError(f"Missing or unexpected assets for {target}.")
        archive = folder / (stem + (".zip" if target.startswith("windows") else ".tar.gz"))
        for native in targets:
            manifest = json.loads((folder / f"manifest-{native}-stable.json").read_text())
            if (manifest.get("schema"), manifest.get("version"), manifest.get("target"),
                manifest.get("channel"), manifest.get("archive"), manifest.get("bytes"),
                manifest.get("sha256")) != (1, version, native, "stable", archive.name,
                                          archive.stat().st_size, hashlib.sha256(archive.read_bytes()).hexdigest()):
                raise ValueError(f"Manifest does not describe the verified payload for {native}.")
        for asset in files:
            if asset.name in assets:
                raise ValueError(f"Duplicate release asset: {asset.name}")
            assets[asset.name] = asset
    output.mkdir(parents=True, exist_ok=False)
    for name, asset in assets.items():
        shutil.copyfile(asset, output / name)
    return sorted(assets)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    assets = prepare(args.artifacts, args.output, args.version)
    print(f"Verified {len(assets)} release assets across all native platforms.")
