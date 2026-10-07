#!/usr/bin/env python3
"""Build a temporary second version and run real helper health/rollback checks.

Restores the original executables after both probes so packaging retains its version.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target')
    parser.add_argument('--profile', choices=['debug', 'release'], default='release')
    args = parser.parse_args()
    suffix = '.exe' if os.name == 'nt' else ''
    binaries = Path('target') / (args.target or '') / args.profile
    original = subprocess.check_output([str((binaries / ('craftlauncher' + suffix)).resolve()), '--version'], text=True).strip().split()[-1]
    base = [int(n) for n in original.split('-')[0].split('.')]
    base[1] += 1
    newer = '.'.join(str(n) for n in base)
    base[1] += 1
    mismatch = '.'.join(str(n) for n in base)
    names = ['craftlauncher', 'craftlauncher-updater', 'craftlauncher-bootstrap']
    with tempfile.TemporaryDirectory(prefix='craftlauncher-qualification-') as tmp:
        saved = Path(tmp)
        for name in names:
            shutil.copy2(binaries / (name + suffix), saved / (name + suffix))
        try:
            command = ['cargo', 'build', '--locked', '--bins']
            if args.target:
                command.extend(['--target', args.target])
            if args.profile == 'release':
                command.append('--release')
            subprocess.run(command, env={**os.environ, 'CRAFTLAUNCHER_BUILD_VERSION': newer}, check=True)
            smoke = ['python3' if os.name != 'nt' else 'python', 'scripts/update_smoke.py', '--old', str(saved / ('craftlauncher' + suffix)), '--new', str(binaries / ('craftlauncher' + suffix)), '--helper', str(binaries / ('craftlauncher-updater' + suffix))]
            subprocess.run(smoke, check=True)
            subprocess.run(smoke + ['--manifest-version', mismatch, '--expect-rollback'], check=True)
            subprocess.run(smoke + ['--tamper-helper'], check=True)
        finally:
            for name in names:
                shutil.copy2(saved / (name + suffix), binaries / (name + suffix))


if __name__ == '__main__':
    main()
