#!/usr/bin/env python3
"""Exercise two real launcher builds, the helper and signed local feed.

Example: --old target/debug/craftlauncher --new /tmp/new-build/craftlauncher
--helper /tmp/new-build/craftlauncher-updater. The new build must have a newer
CRAFTLAUNCHER_BUILD_VERSION. Requires an existing desktop graphics session.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import time
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--old', type=Path, required=True)
    parser.add_argument('--new', type=Path, required=True)
    parser.add_argument('--helper', type=Path, required=True)
    parser.add_argument('--manifest-version', help='Override for the mismatch/rollback counterprobe')
    parser.add_argument('--expect-rollback', action='store_true')
    parser.add_argument('--tamper-helper', action='store_true', help='Prove a damaged prepared helper does not block the old UI')
    args = parser.parse_args()
    old, new, helper = args.old.resolve(), args.new.resolve(), args.helper.resolve()
    version = args.manifest_version or subprocess.check_output([str(new), '--version'], text=True).strip().split()[-1]
    with tempfile.TemporaryDirectory(prefix='craftlauncher-update-smoke-') as tmp:
        tmp = Path(tmp)
        root, feed = tmp / 'library', tmp / 'feed'
        feed.mkdir()
        keys = tmp / 'keys'
        subprocess.run([str(old), '--generate-update-key', str(keys)], check=True, stdout=subprocess.DEVNULL)
        windows = os.name == 'nt'
        extension = '.exe' if windows else ''
        archive = feed / ('launcher.zip' if windows else 'launcher.tar.gz')
        if windows:
            with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as out:
                out.write(new, 'craftlauncher.exe')
                out.write(helper, 'craftlauncher-updater.exe')
        else:
            with tarfile.open(archive, 'w:gz') as out:
                out.add(new, arcname='craftlauncher')
                out.add(helper, arcname='craftlauncher-updater')
        snapshot = json.loads(subprocess.check_output([str(old), '--data-dir', str(root), '--disable-startup-check'], text=True))
        target = f'{snapshot["platform"]}-{snapshot["arch"]}'
        manifest = feed / f'manifest-{target}-stable.json'
        manifest.write_text(json.dumps(dict(schema=1, version=version, channel='stable', target=target, archive=archive.name, sha256=hashlib.sha256(archive.read_bytes()).hexdigest(), bytes=archive.stat().st_size, executable='craftlauncher' + extension, helper='craftlauncher-updater' + extension)))
        subprocess.run([str(old), '--sign-manifest', str(manifest), '--signing-key', str(keys / 'update-private.key')], check=True, stdout=subprocess.DEVNULL)
        subprocess.run([str(old), '--data-dir', str(root), '--configure-local-feed', str(feed), '--public-key', (keys / 'update-public.key').read_text(), '--launcher-check'], check=True, stdout=subprocess.DEVNULL)
        prepared = json.loads((root / 'state.json').read_text())['launcher_update']
        active = root / prepared['executable']
        if args.tamper_helper:
            (root / prepared['helper']).write_bytes(b'damaged prepared helper')
            with (tmp / 'old-ui.log').open('w+') as log:
                result = subprocess.run([str(old), '--data-dir', str(root), '--exit-after', '3'], stdout=log, stderr=log, timeout=45)
                if result.returncode:
                    log.seek(0)
                    raise AssertionError(log.read())
            state = json.loads((root / 'state.json').read_text())
            assert state['launcher_update'] is None
            assert state['failed_launcher_version'] == version
            assert not (root / 'launcher/update-job.json').exists()
            assert any(e['action'] == 'launcher update' and e['error'] for e in state['activity'])
            print(f'PASS damaged prepared helper discarded; original native UI remains usable: {target}')
            return
        subprocess.run([str(old), '--data-dir', str(root), '--launcher-restart'], check=True, stdout=subprocess.DEVNULL)
        report = root / 'launcher/last-update.json'
        deadline = time.monotonic() + 60
        result = None
        while time.monotonic() < deadline:
            if report.exists():
                result = json.loads(report.read_text())
                break
            # The worker may already have consumed the report.
            state = json.loads((root / 'state.json').read_text())
            event = next((e for e in state['activity'] if e['action'] == 'launcher update'), None)
            if event:
                result = dict(version=version, success=not event['error'])
                break
            time.sleep(0.1)
        try:
            assert result and result['success'] != args.expect_rollback, f'Unexpected update result: {result}'
            pointer = json.loads((root / 'launcher/current.json').read_text())
            assert Path(pointer['executable']) == (old if args.expect_rollback else active)
            assert pointer['version'] == (subprocess.check_output([str(old), '--version'], text=True).strip().split()[-1] if args.expect_rollback else version)
            print(f'PASS signed real-process update {target}: {"rollback" if args.expect_rollback else version + " healthy"}')
        finally:
            # Only processes whose command line contains this unique fixture path.
            stop_fixture_processes(root)
            time.sleep(0.5)


def stop_fixture_processes(root):
    if os.name == 'nt':
        command = "$root=$env:CRAFT_SMOKE_ROOT; Get-CimInstance Win32_Process | Where-Object {$_.CommandLine -like ('*'+$root+'*') -and $_.Name -like 'craftlauncher*'} | ForEach-Object {Stop-Process -Id $_.ProcessId -Force}"
        subprocess.run(['powershell', '-NoProfile', '-Command', command], env={**os.environ, 'CRAFT_SMOKE_ROOT': str(root)}, check=True)
    else:
        output = subprocess.check_output(['ps', '-axo', 'pid=,args='], text=True)
        for line in output.splitlines():
            pid, _, command = line.strip().partition(' ')
            if str(root) in command and 'craftlauncher' in command:
                try:
                    os.kill(int(pid), 15)
                except ProcessLookupError:
                    pass


if __name__ == '__main__':
    main()
