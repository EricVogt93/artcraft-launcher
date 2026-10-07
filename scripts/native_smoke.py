#!/usr/bin/env python3
"""Prove native first-frame rendering, shutdown and persisted settings.

Uses the existing graphics session. On Linux: dbus-run-session -- xvfb-run -a
python3 scripts/native_smoke.py --binary target/debug/craftlauncher
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--timeout', type=int, default=45)
    args = parser.parse_args()
    binary = args.binary.resolve()
    version = subprocess.check_output([str(binary), '--version'], text=True).strip().split()[-1]
    with tempfile.TemporaryDirectory(prefix='craftlauncher-smoke-') as tmp:
        root = Path(tmp) / 'library'
        health = root / 'launcher/health-smoke'
        env = os.environ.copy()
        if os.name != 'nt' and not env.get('XDG_RUNTIME_DIR'):
            runtime = Path(tmp) / 'runtime'
            runtime.mkdir(mode=0o700)
            env['XDG_RUNTIME_DIR'] = str(runtime)
        subprocess.run([str(binary), '--data-dir', str(root), '--disable-startup-check', '--theme', 'dark'], check=True, stdout=subprocess.DEVNULL, env=env)
        with (Path(tmp) / 'native.log').open('w+') as log:
            process = subprocess.Popen([str(binary), '--data-dir', str(root), '--update-health', str(health), '--expected-version', version, '--exit-after', '3'], env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + args.timeout
                while not health.exists() and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.1)
                assert health.exists() and health.read_text() == version, 'Native UI never acknowledged a rendered first frame'
                assert process.wait(timeout=args.timeout) == 0, 'Native UI did not shut down cleanly'
                snapshot = json.loads(subprocess.check_output([str(binary), '--data-dir', str(root), '--list'], env=env, text=True))
                assert snapshot['state']['settings']['theme'] == 'dark', 'Settings did not survive UI reload'
                assert snapshot['state']['settings']['check_on_startup'] is False
                print(f'PASS native first frame, clean shutdown, persisted reload: {snapshot["platform"]}/{snapshot["arch"]}, version {version}')
            except BaseException:
                log.seek(0)
                print(log.read())
                raise
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()


if __name__ == '__main__':
    main()
