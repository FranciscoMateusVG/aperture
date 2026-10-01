#!/usr/bin/env python3
"""Build a new, local AppKit launcher bundle. No install/start/stop/overwrite."""
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys


def build(destination, server, node):
    root = Path(__file__).resolve().parent.parent
    dest = Path(destination)
    for path in (Path(server), Path(node)):
        if not path.is_absolute() or not path.is_file() or path.is_symlink() or not os.access(path, os.X_OK):
            raise ValueError("server and node must be absolute executable files, not shims or symlinks")
    if not dest.is_absolute() or dest.suffix != '.app':
        raise ValueError("destination must be an absolute .app path")
    dest.mkdir(mode=0o700)  # exclusive, even an existing symlink fails
    for sub in ('Contents', 'Contents/MacOS', 'Contents/Resources'):
        (dest / sub).mkdir(mode=0o700)
    sources = [root / 'macos-launcher' / name for name in ('LauncherCore.swift', 'NativeControl.swift', 'main.swift')]
    executable = dest / 'Contents/MacOS/ApertureWeb'
    subprocess.run(['/usr/bin/swiftc', '-O', *map(str, sources), '-o', str(executable), '-framework', 'AppKit'], check=True, timeout=120)
    executable.chmod(0o700)
    resources = dest / 'Contents/Resources'
    shutil.copyfile(root / 'src-tauri/icons/icon.icns', resources / 'AppIcon.icns')
    (resources / 'Launcher.json').write_text(json.dumps({'server': server, 'node': node}) + '\n')
    with (dest / 'Contents/Info.plist').open('wb') as out:
        plistlib.dump({'CFBundleExecutable': 'ApertureWeb', 'CFBundleIdentifier': 'org.programaincluir.aperture.web-launcher',
                      'CFBundleName': 'Aperture Web', 'CFBundleDisplayName': 'Aperture Web',
                      'CFBundlePackageType': 'APPL', 'CFBundleVersion': '1', 'CFBundleShortVersionString': '1.0',
                      'CFBundleIconFile': 'AppIcon', 'NSHighResolutionCapable': True,
                      'LSMinimumSystemVersion': '13.0', 'LSMultipleInstancesProhibited': True}, out)
    for file in (resources / 'AppIcon.icns', resources / 'Launcher.json', dest / 'Contents/Info.plist'):
        file.chmod(0o600)
    subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(dest)], check=True, timeout=30)
    subprocess.run(['/usr/bin/codesign', '--verify', '--strict', str(dest)], check=True, timeout=30)
    print(dest)


if __name__ == '__main__':
    if len(sys.argv) != 4:
        raise SystemExit('usage: build.py /new/Aperture\ Web.app /package/bin/aperture-server /absolute/node')
    build(*sys.argv[1:])
