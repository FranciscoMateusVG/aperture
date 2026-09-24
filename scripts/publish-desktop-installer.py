#!/usr/bin/env python3
"""Bounded publication of an already-built .command; never runs the installer.

Canonical replacement for the release handoff's inline Desktop copy/fsync block.
Only the new writer child may be signalled, only after PID/birth revalidation.
No retries, overwrites, removal of partial files, installer execution or daemons.
"""
import argparse
import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import time

DEADLINE_SECONDS = 60
CAP = 256 * 1024
NAME = re.compile(r"Atualizar Aperture - [0-9a-f]{7,40}(?:-v[1-9][0-9]*)?\.command\Z")
STAGES = (
    "directory_validate", "directory_open", "create_exclusive", "write",
    "file_fsync", "readback", "executable_mode", "executable_fsync",
    "directory_fsync", "published_readback", "close", "complete",
)


class PublicationError(Exception):
    pass


class _Info(C.Structure):
    _fields_ = [(n, C.c_uint32) for n in (
        "flags", "status", "xstatus", "pid", "ppid", "uid", "gid", "ruid",
        "rgid", "svuid", "svgid", "reserved",
    )] + [("comm", C.c_char * 16), ("name", C.c_char * 32)] + [
        (n, C.c_uint32) for n in ("nfiles", "pgid", "pjobc", "tdev", "tpgid")
    ] + [("nice", C.c_int32), ("sec", C.c_uint64), ("usec", C.c_uint64)]


def _identity(pid):
    lib = C.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    lib.proc_pidinfo.argtypes = [C.c_int, C.c_int, C.c_uint64, C.c_void_p, C.c_int]
    value = _Info()
    count = lib.proc_pidinfo(pid, 3, 0, C.byref(value), C.sizeof(value))
    if count != C.sizeof(value) or value.pid != pid:
        raise PublicationError("E_WRITER_IDENTITY_UNAVAILABLE")
    return (value.pid, value.sec * 1_000_000 + value.usec, value.ppid, value.uid)


def _require(ok, code):
    if not ok:
        raise PublicationError(code)


def _directory(path):
    # Metadata only until the single open below; no symlink/canonicalize fallback.
    for component in [*reversed(path.parents), path]:
        s = component.lstat()
        _require(stat.S_ISDIR(s.st_mode), "E_DIRECTORY_UNSAFE")
    s = path.lstat()
    _require(s.st_uid == os.geteuid() and not s.st_mode & 0o022, "E_DIRECTORY_UNSAFE")
    return s


def _same_file(fd, directory, name, expected, mode):
    s = os.fstat(fd)
    named = os.stat(name, dir_fd=directory, follow_symlinks=False)
    _require(stat.S_ISREG(s.st_mode) and s.st_nlink == 1 and s.st_uid == os.geteuid()
             and stat.S_IMODE(s.st_mode) == mode
             and (s.st_dev, s.st_ino) == (named.st_dev, named.st_ino), "E_READBACK_IDENTITY")
    os.lseek(fd, 0, os.SEEK_SET)
    data = bytearray()
    while len(data) <= CAP:
        part = os.read(fd, min(65536, CAP + 1 - len(data)))
        if not part:
            break
        data.extend(part)
    _require(len(data) <= CAP and hashlib.sha256(data).hexdigest() == expected, "E_READBACK_HASH")


def _write(data, path, name, expected, emit):
    directory = file = None
    try:
        emit("directory_validate")
        before = _directory(path)
        emit("directory_open")
        directory = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        opened = os.fstat(directory)
        _require((before.st_dev, before.st_ino) == (opened.st_dev, opened.st_ino), "E_DIRECTORY_DRIFT")
        emit("create_exclusive")
        file = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                       0o600, dir_fd=directory)
        emit("write")
        remaining = memoryview(data)
        while remaining:
            written = os.write(file, remaining)
            _require(written > 0, "E_WRITE_INCOMPLETE")
            remaining = remaining[written:]
        emit("file_fsync")
        os.fsync(file)
        emit("readback")
        _same_file(file, directory, name, expected, 0o600)
        emit("executable_mode")
        os.fchmod(file, 0o700)
        emit("executable_fsync")
        os.fsync(file)
        emit("directory_fsync")
        os.fsync(directory)
        emit("published_readback")
        _same_file(file, directory, name, expected, 0o700)
        current = path.lstat()
        _require(stat.S_ISDIR(current.st_mode) and current.st_uid == os.geteuid()
                 and not current.st_mode & 0o022
                 and (current.st_dev, current.st_ino) == (opened.st_dev, opened.st_ino), "E_DIRECTORY_DRIFT")
        emit("close")
        os.close(file)
        file = None
        os.close(directory)
        directory = None
        emit("complete")
    finally:
        if file is not None:
            os.close(file)
        if directory is not None:
            os.close(directory)


def _error(error):
    if isinstance(error, PublicationError):
        return str(error)
    if isinstance(error, FileExistsError):
        return "E_DESTINATION_EXISTS"
    if isinstance(error, PermissionError):
        return "E_IO_PERMISSION"
    return "E_IO_FAILURE"


def _signal_owned(pid, identity, identity_fn, signal_fn):
    # The parent has not reaped this child. No PID reuse is possible between
    # this check and kill while it remains our unreaped child. No group signal.
    _require(identity is not None and identity[0] == pid
             and identity[2:] == (os.getpid(), os.geteuid()), "E_WRITER_IDENTITY_UNAVAILABLE")
    _require(identity_fn(pid) == identity, "E_WRITER_IDENTITY_CHANGED")
    signal_fn(pid, signal.SIGKILL)


def _publish(data, path, name, expected, *, _seconds=DEADLINE_SECONDS,
             _writer=_write, _identity_fn=_identity, _signal_fn=os.kill):
    """Private test seams; CLI exposes no destination/deadline/signal override."""
    _require(NAME.fullmatch(name) and 0 < len(data) <= CAP
             and hashlib.sha256(data).hexdigest() == expected, "E_SOURCE_INVALID")
    gate_read, gate_write = os.pipe()
    events_read, events_write = os.pipe()
    deadline = time.monotonic() + _seconds
    pid = os.fork()
    if pid == 0:
        os.close(gate_write)
        os.close(events_read)

        def emit(stage, code=None):
            os.write(events_write, (json.dumps({"stage": stage, "code": code}) + "\n").encode())

        try:
            # No Desktop access until parent captured this exact child's identity.
            if os.read(gate_read, 1) != b"1":
                os._exit(2)
            _writer(data, path, name, expected, emit)
        except BaseException as error:
            emit("error", _error(error))
            os._exit(1)
        os._exit(0)
    os.close(gate_read)
    os.close(events_write)
    identity = None
    last = "identity"
    error = None
    buffer = b""
    stages = []
    status = None
    sent = False
    try:
        try:
            identity = _identity_fn(pid)
            _require(identity[0] == pid and identity[2:] == (os.getpid(), os.geteuid()), "E_WRITER_IDENTITY_UNAVAILABLE")
            os.write(gate_write, b"1")
        except (PublicationError, OSError) as failure:
            error = _error(failure)
        finally:
            os.close(gate_write)
        os.set_blocking(events_read, False)
        while time.monotonic() < deadline:
            select.select([events_read], [], [], min(0.05, max(0, deadline - time.monotonic())))
            try:
                chunk = os.read(events_read, 4096)
            except BlockingIOError:
                chunk = b""
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                event = json.loads(line)
                if event["stage"] == "error":
                    error = event["code"]
                else:
                    _require(event["stage"] in STAGES, "E_WRITER_PROTOCOL")
                    last = event["stage"]
                    stages.append(last)
            got, child_status = os.waitpid(pid, os.WNOHANG)
            if got:
                status = child_status
                # Drain queued final events on the next iteration before deciding.
                while True:
                    tail = os.read(events_read, 4096)
                    if not tail:
                        break
                    buffer += tail
                for line in buffer.splitlines():
                    event = json.loads(line)
                    if event["stage"] == "error":
                        error = event["code"]
                    else:
                        last = event["stage"]
                        stages.append(last)
                break
        if status is None:
            # Recheck child exit before any signal; never act on another process.
            error = error or "E_DESKTOP_IO_TIMEOUT"
            got, child_status = os.waitpid(pid, os.WNOHANG)
            if got:
                status = child_status
            else:
                error = "E_DESKTOP_IO_TIMEOUT"
                try:
                    _signal_owned(pid, identity, _identity_fn, _signal_fn)
                    sent = True
                except (PublicationError, OSError):
                    error = "E_DESKTOP_IO_PENDING_IDENTITY"
                cleanup_until = time.monotonic() + 2
                while time.monotonic() < cleanup_until:
                    got, child_status = os.waitpid(pid, os.WNOHANG)
                    if got:
                        status = child_status
                        break
                    time.sleep(0.02)
                if status is None:
                    error = "E_DESKTOP_IO_PENDING_IDENTITY" if not sent else "E_DESKTOP_IO_PENDING_EXIT"
        exit_code = os.waitstatus_to_exitcode(status) if status is not None else None
        ready = error is None and exit_code == 0 and stages == list(STAGES)
        return {"ready": ready, "code": "PUBLISHED" if ready else error or "E_WRITER_INCOMPLETE",
                "stage": last, "pid": pid, "birth_us": identity[1] if identity else None,
                "child_reaped": status is not None, "signal_sent": sent, "exit_code": exit_code,
                "sha256": expected if ready else None, "stages": stages, "retry": False}
    finally:
        os.close(events_read)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True)
    parser.add_argument("--sha256", required=True)
    args = parser.parse_args()
    try:
        source = Path(args.source)
        _require(source.parent == Path("/private/tmp") and NAME.fullmatch(source.name), "E_SOURCE_PATH")
        fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, "rb") as file:
            s = os.fstat(file.fileno())
            _require(stat.S_ISREG(s.st_mode) and s.st_uid == os.geteuid() and s.st_nlink == 1
                     and stat.S_IMODE(s.st_mode) == 0o700 and 0 < s.st_size <= CAP, "E_SOURCE_UNSAFE")
            data = file.read(CAP + 1)
        _require(hashlib.sha256(data).hexdigest() == args.sha256, "E_SOURCE_HASH")
        # Parse bytes, never execute .command or ask zsh to open Desktop itself.
        syntax = subprocess.run(["/bin/zsh", "-f", "-n"], input=data, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, timeout=5)
        _require(syntax.returncode == 0, "E_SOURCE_SYNTAX")
        result = _publish(data, Path.home() / "Desktop", source.name, args.sha256)
    except (PublicationError, OSError, subprocess.TimeoutExpired) as error:
        result = {"ready": False, "code": _error(error), "retry": False}
    print(json.dumps(result), flush=True)
    return 0 if result["ready"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
