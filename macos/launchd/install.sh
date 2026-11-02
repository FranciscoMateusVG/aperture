#!/bin/bash
# No bootstrap, bootout, enable, disable, kill, or application launch here.
# Even installation waits for the operator-approved cutover window.
set -euo pipefail
exec /usr/bin/python3 - "$(cd "$(dirname "$0")" && pwd)" "$@" <<'PY'
import argparse
import datetime
import os
from pathlib import Path
import plistlib
import pwd
import shlex
import shutil
import subprocess
import sys
import tempfile

source = Path(sys.argv.pop(1))
parser = argparse.ArgumentParser(description="Aperture LaunchAgent: check/install/uninstall, never activate")
parser.add_argument("action", choices=("check", "install", "uninstall"))
parser.add_argument("--repo", type=Path, help="absolute main checkout; mandatory for check/install")
args = parser.parse_args()

def fail(message):
    raise SystemExit("Aperture: " + message)

if sys.platform != "darwin" or os.getuid() == 0:
    fail("run as the macOS login user, never sudo/root")
os.umask(0o077)
user = pwd.getpwuid(os.getuid())
home = Path(user.pw_dir)
label = "com.aperture.server"
domain = "gui/" + str(os.getuid())
support = home / "Library/Application Support/Aperture/launchd"
logs = home / "Library/Logs/Aperture"
plist_path = home / "Library/LaunchAgents" / (label + ".plist")
script_path = support / "server.sh"

def reject_symlinks(path):
    for item in (path, *path.parents):
        if item.is_symlink():
            fail("refusing symlink in installation path: " + str(item))

def safe_file(path):
    reject_symlinks(path)
    if path.exists() and (not path.is_file() or path.stat().st_uid != os.getuid()):
        fail("not a regular file owned by this user: " + str(path))

def safe_directory(path):
    reject_symlinks(path)
    if path.exists() and (not path.is_dir() or path.stat().st_uid != os.getuid()):
        fail("not a directory owned by this user: " + str(path))

def archive():
    existing = [p for p in (plist_path, script_path) if p.exists()]
    if not existing:
        return None
    backup_root = support / "backups"
    safe_directory(backup_root)
    backup_root.mkdir(parents=True, exist_ok=True, mode=0o700)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    dest = Path(tempfile.mkdtemp(prefix=stamp + "-", dir=backup_root))
    for path in existing:
        shutil.copy2(path, dest / path.name)
    print("Previous files preserved:", dest)
    return dest

def write_atomic(path, content, mode):
    fd, name = tempfile.mkstemp(prefix="." + path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as out:
            out.write(content)
            os.fchmod(out.fileno(), mode)
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)

if args.action != "uninstall":
    if args.repo is None or not args.repo.is_absolute():
        fail("pass --repo with the absolute MAIN checkout, not this task's worktree")
    repo = args.repo.resolve()
    for name in ("mix.exs", "scripts/up.sh", "macos/Aperture.app/Contents/MacOS/Aperture"):
        if not (repo / name).is_file():
            fail("not an Aperture checkout: " + str(repo))
    branch = subprocess.check_output(["/usr/bin/git", "-C", str(repo), "branch", "--show-current"], text=True).strip()
    if branch != "main":
        fail("target checkout must be on main; found " + repr(branch))
    values = {
        "__APERTURE_HOME__": str(home), "__APERTURE_USER__": user.pw_name,
        "__APERTURE_REPO__": str(repo), "__APERTURE_UID__": str(os.getuid()),
    }
    script = (source / "server.sh").read_text()
    for token, value in values.items():
        script = script.replace(token, shlex.quote(value))
    config = plistlib.loads((source / (label + ".plist")).read_bytes())
    def render(value):
        if isinstance(value, str):
            for token, text in values.items():
                value = value.replace(token, text)
            return value
        if isinstance(value, list):
            return [render(item) for item in value]
        if isinstance(value, dict):
            return {key: render(item) for key, item in value.items()}
        return value
    plist = plistlib.dumps(render(config), sort_keys=False)
    subprocess.run(["/bin/bash", "-n"], input=script, text=True, check=True)
    subprocess.run(["/usr/bin/plutil", "-lint", "-"], input=plist, check=True)
    # Real tools under the exact empty-environment boundary; no file writes,
    # server, tmux session, credentials, or auth status are touched by --check.
    subprocess.run(["/usr/bin/env", "-i", "/bin/bash", "--noprofile", "--norc",
                    "-c", script, "aperture-preflight", "--check"], check=True)
    if args.action == "check":
        print("Check only: no installed files or launchd state changed.")
        sys.exit(0)

# Refuse changing files under an already loaded job, even if it is not running.
loaded = subprocess.run(["/bin/launchctl", "print", domain + "/" + label],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
if loaded.returncode == 0:
    fail("job is loaded; arrange the window, disable and bootout before changing files")
for directory in (support, plist_path.parent, logs):
    safe_directory(directory)
for path in (plist_path, script_path, logs / "server.stdout.log", logs / "server.stderr.log"):
    safe_file(path)

if args.action == "uninstall":
    if archive() is not None:
        for path in (plist_path, script_path):
            if path.exists():
                path.unlink()
    print("Uninstalled definition/script (backups, logs, data, Dock app and auth preserved).")
    print("No launchctl state changed; an existing disabled override remains disabled.")
else:
    for directory in (support, plist_path.parent, logs):
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(support, 0o700)
    os.chmod(logs, 0o700)
    archive()
    for path in (logs / "server.stdout.log", logs / "server.stderr.log"):
        path.touch(mode=0o600, exist_ok=True)
        os.chmod(path, 0o600)
    write_atomic(script_path, script.encode(), 0o700)
    write_atomic(plist_path, plist, 0o600)
    print("Installed, NOT loaded:", plist_path)
    print("Bootstrap only in the agreed window. Definition is eligible at next GUI login.")
PY
