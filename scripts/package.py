#!/usr/bin/env python3
"""Build native distribution archives and unsigned local-feed manifests.

Never publishes. Signing is a separate explicit step with craftlauncher --sign-manifest.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import zipfile


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binaries', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--target', required=True, choices=['linux-x86_64', 'linux-aarch64', 'freebsd-x86_64', 'windows-x86', 'windows-x86_64', 'windows-aarch64', 'macos-universal'])
    parser.add_argument('--channel', choices=['stable', 'preview'], default='stable')
    parser.add_argument('--native-installer', action='store_true')
    parser.add_argument('--appimagetool', type=Path)
    args = parser.parse_args()
    if not all(c.isalnum() or c in '.+-' for c in args.version):
        parser.error('Invalid version')
    args.output.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parents[1]
    names = ['craftlauncher', 'craftlauncher-updater', 'craftlauncher-bootstrap']
    windows = args.target.startswith('windows')
    mac = args.target.startswith('macos')
    suffix = '.exe' if windows else ''
    stem = f'craftlauncher-{args.version}-{args.target}'
    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp)
        binary_dir = stage / 'CraftLauncher.app/Contents/MacOS' if mac else stage
        binary_dir.mkdir(parents=True, exist_ok=True)
        for name in names:
            shutil.copy2(args.binaries / (name + suffix), binary_dir / (name + suffix))
        if mac:
            contents = stage / 'CraftLauncher.app/Contents'
            resources = contents / 'Resources'
            resources.mkdir()
            iconset = stage / 'CraftLauncher.iconset'
            iconset.mkdir()
            for size in [16, 32, 128, 256, 512]:
                for scale in [1, 2]:
                    file = iconset / f'icon_{size}x{size}{"@2x" if scale == 2 else ""}.png'
                    run('sips', '-z', size * scale, size * scale, source / 'assets/craftlauncher-logo.png', '--out', file)
            run('iconutil', '-c', 'icns', iconset, '-o', resources / 'CraftLauncher.icns')
            shutil.rmtree(iconset)
            with (contents / 'Info.plist').open('wb') as stream:
                plistlib.dump(dict(CFBundleIdentifier='dev.craftlauncher.desktop', CFBundleName='CraftLauncher', CFBundleDisplayName='CraftLauncher', CFBundleExecutable='craftlauncher-bootstrap', CFBundlePackageType='APPL', CFBundleShortVersionString=args.version, CFBundleVersion=args.version, CFBundleIconFile='CraftLauncher.icns', LSMinimumSystemVersion='11.0', NSHighResolutionCapable=True), stream)
        else:
            shutil.copy2(source / 'assets/craftlauncher-logo.png', stage / 'craftlauncher.png')
            shutil.copy2(source / 'README.md', stage / 'README.md')
            shutil.copy2(source / 'LICENSE', stage / 'LICENSE')
            if not windows:
                (stage / 'craftlauncher.desktop').write_text('[Desktop Entry]\nType=Application\nName=CraftLauncher\nExec=craftlauncher-bootstrap\nIcon=craftlauncher\nTerminal=false\nCategories=Graphics;Utility;\nStartupWMClass=craftlauncher\n')
        license_dir = stage / 'CraftLauncher.app/Contents/Resources/licenses' if mac else stage / 'licenses'
        license_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / 'LICENSE', license_dir / 'LICENSE')
        shutil.copy2(source / 'NOTICE', license_dir / 'NOTICE')
        shutil.copy2(source / 'CHANGELOG.md', license_dir / 'CHANGELOG.md')
        for name in ['archivo-OFL.txt', 'instrumentserif-OFL.txt', 'ATTRIBUTION.md']:
            shutil.copy2(source / 'assets' / name, license_dir / name)
        if mac:
            run('codesign', '--force', '--deep', '--sign', '-', stage / 'CraftLauncher.app')
            run('codesign', '--verify', '--deep', '--strict', stage / 'CraftLauncher.app')
            if args.native_installer:
                run('hdiutil', 'create', '-volname', 'CraftLauncher', '-srcfolder', stage / 'CraftLauncher.app', '-ov', '-format', 'UDZO', args.output / (stem + '.dmg'))
        archive = args.output / (stem + ('.zip' if windows else '.tar.gz'))
        if windows:
            with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as out:
                for file in sorted(stage.rglob('*')):
                    if file.is_file():
                        out.write(file, file.relative_to(stage))
        else:
            with tarfile.open(archive, 'w:gz', dereference=False) as out:
                for file in sorted(stage.iterdir()):
                    out.add(file, arcname=file.name)
        if args.native_installer and windows:
            run('makensis', f'-DSOURCE={stage}', f'-DOUTPUT={args.output.resolve() / (stem + "-setup.exe")}', f'-DVERSION={args.version}', source / 'packaging/windows.nsi')
        if args.native_installer and args.target.startswith('linux'):
            package_deb(stage, args, source)
            package_rpm(stage, args)
        if args.appimagetool and args.target.startswith('linux'):
            appdir = stage / 'CraftLauncher.AppDir'
            appdir.mkdir()
            for name in names:
                shutil.copy2(stage / name, appdir / name)
            shutil.copytree(license_dir, appdir / 'licenses')
            shutil.copy2(stage / 'craftlauncher.png', appdir / 'craftlauncher.png')
            shutil.copy2(stage / 'craftlauncher.desktop', appdir / 'craftlauncher.desktop')
            (appdir / 'AppRun').write_text('#!/bin/sh\napp_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexec "$app_dir/craftlauncher-bootstrap" "$@"\n')
            (appdir / 'AppRun').chmod(0o755)
            run(args.appimagetool.resolve(), '--appimage-extract-and-run', appdir, args.output.resolve() / (stem + '.AppImage'))
        targets = ['macos-x86_64', 'macos-aarch64'] if mac else [args.target]
        relative = 'CraftLauncher.app/Contents/MacOS/' if mac else ''
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        for target in targets:
            manifest = dict(schema=1, version=args.version, channel=args.channel, target=target, archive=archive.name, sha256=digest, bytes=archive.stat().st_size, executable=relative + 'craftlauncher' + suffix, helper=relative + 'craftlauncher-updater' + suffix)
            (args.output / f'manifest-{target}-{args.channel}.json').write_text(json.dumps(manifest, indent=2) + '\n')
        sums = []
        for file in sorted(args.output.glob(stem + '*')):
            if file.is_file() and not file.name.endswith('-SHA256SUMS.txt'):
                sums.append(f'{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.name}')
        (args.output / f'{stem}-SHA256SUMS.txt').write_text('\n'.join(sums) + '\n')
    print(f'Packaged {stem} into {args.output}. Manifests still require a signature.')


def install_tree(stage, root):
    opt = root / 'opt/craftlauncher'
    opt.mkdir(parents=True)
    for name in ['craftlauncher', 'craftlauncher-updater', 'craftlauncher-bootstrap']:
        shutil.copy2(stage / name, opt / name)
    shutil.copytree(stage / 'licenses', opt / 'licenses')
    (root / 'usr/bin').mkdir(parents=True)
    (root / 'usr/bin/craftlauncher').symlink_to('/opt/craftlauncher/craftlauncher-bootstrap')
    applications = root / 'usr/share/applications'
    applications.mkdir(parents=True)
    (applications / 'craftlauncher.desktop').write_text((stage / 'craftlauncher.desktop').read_text().replace('Exec=craftlauncher-bootstrap', 'Exec=/opt/craftlauncher/craftlauncher-bootstrap'))
    icons = root / 'usr/share/icons/hicolor/256x256/apps'
    icons.mkdir(parents=True)
    shutil.copy2(stage / 'craftlauncher.png', icons / 'craftlauncher.png')


def package_deb(stage, args, source):
    root = stage / 'deb-root'
    install_tree(stage, root)
    control = root / 'DEBIAN'
    control.mkdir()
    arch = 'amd64' if args.target.endswith('x86_64') else 'arm64'
    (control / 'control').write_text(f'Package: craftlauncher\nVersion: {args.version}\nArchitecture: {arch}\nMaintainer: CraftLauncher Contributors\nSection: graphics\nPriority: optional\nDepends: libc6 (>= 2.35), libx11-6, libx11-xcb1, libxcursor1, libxi6, libxrandr2, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0\nRecommends: xdg-desktop-portal, libvulkan1, libgl1\nDescription: Native Rust toolbox for ArtCraft creative apps\n')
    run('dpkg-deb', '--root-owner-group', '--build', root, args.output / f'craftlauncher-{args.version}-{args.target}.deb')


def package_rpm(stage, args):
    top = stage / 'rpm'
    for name in ['BUILD', 'RPMS', 'SOURCES', 'SPECS', 'SRPMS', 'BUILDROOT']:
        (top / name).mkdir(parents=True)
    root = stage / 'rpm-files'
    install_tree(stage, root)
    arch = 'x86_64' if args.target.endswith('x86_64') else 'aarch64'
    spec = top / 'SPECS/craftlauncher.spec'
    spec.write_text(f'''Name: craftlauncher
Version: {args.version}
Release: 1
Summary: Native toolbox for ArtCraft creative apps
License: Apache-2.0
BuildArch: {arch}
AutoReqProv: no
Requires: glibc >= 2.35, libX11.so.6()(64bit), libX11-xcb.so.1()(64bit), libXcursor.so.1()(64bit), libXi.so.6()(64bit), libXrandr.so.2()(64bit), libxkbcommon.so.0()(64bit), libxkbcommon-x11.so.0()(64bit), libwayland-client.so.0()(64bit)
%description
Native Rust creative app launcher.
%install
mkdir -p %{{buildroot}}
cp -a "{root}/." %{{buildroot}}/
%files
/opt/craftlauncher
/usr/bin/craftlauncher
/usr/share/applications/craftlauncher.desktop
/usr/share/icons/hicolor/256x256/apps/craftlauncher.png
''')
    run('rpmbuild', '--define', f'_topdir {top}', '-bb', spec)
    for file in (top / 'RPMS').rglob('*.rpm'):
        shutil.copy2(file, args.output / f'craftlauncher-{args.version}-{args.target}.rpm')


if __name__ == '__main__':
    main()
