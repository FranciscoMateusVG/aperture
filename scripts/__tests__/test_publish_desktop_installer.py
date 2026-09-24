"""Hermetic publication tests: fake Desktop, inert scripts, only own children."""
import ctypes
import hashlib
import importlib.util
import os
from pathlib import Path
import stat
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "writer", Path(__file__).resolve().parents[1] / "publish-desktop-installer.py")
W = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(W)
DATA = b"#!/bin/zsh\nprint 'fixture never executed'\n"
HASH = hashlib.sha256(DATA).hexdigest()
NAME = "Atualizar Aperture - 123abcd.command"


@unittest.skipUnless(sys.platform == "darwin", "native Darwin PID/birth oracle")
class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = Path(self.tmp.name).resolve() / "fake-Desktop"
        self.path.mkdir(mode=0o700)

    def tearDown(self):
        self.tmp.cleanup()

    def publish(self, **kw):
        return W._publish(DATA, self.path, NAME, HASH, _seconds=1, **kw)

    def test_success_has_one_directory_open_exit_zero_and_durable_bytes(self):
        def writer(*args):
            opened = 0
            original_open = os.open
            def checked_open(path, *a, **kw):
                nonlocal opened
                if path == self.path:
                    opened += 1
                return original_open(path, *a, **kw)
            with patch.object(os, "open", checked_open):
                W._write(*args)
            if opened != 1:
                raise W.PublicationError("E_DIRECTORY_OPEN_COUNT")
        result = self.publish(_writer=writer)
        self.assertTrue(result["ready"])
        self.assertEqual((result["exit_code"], result["stage"]), (0, "complete"))
        self.assertTrue(result["child_reaped"])
        self.assertFalse(result["signal_sent"])
        self.assertEqual((self.path / NAME).read_bytes(), DATA)
        self.assertEqual(stat.S_IMODE((self.path / NAME).stat().st_mode), 0o700)
        with self.assertRaises(ChildProcessError):
            os.waitpid(result["pid"], os.WNOHANG)

    def test_open_timeout_reaps_only_new_writer_before_any_file(self):
        def writer(*args):
            original_open = os.open
            def hang_directory(path, *a, **kw):
                if path == self.path:
                    time.sleep(30)
                return original_open(path, *a, **kw)
            with patch.object(os, "open", hang_directory):
                W._write(*args)
        before = time.monotonic()
        r = W._publish(DATA, self.path, NAME, HASH, _seconds=.12, _writer=writer)
        self.assertLess(time.monotonic() - before, 3)
        self.assertEqual((r["ready"], r["code"], r["stage"]),
                         (False, "E_DESKTOP_IO_TIMEOUT", "directory_open"))
        self.assertTrue(r["signal_sent"] and r["child_reaped"])
        self.assertFalse((self.path / NAME).exists())

    def test_directory_fsync_timeout_preserves_file_but_never_readiness(self):
        def writer(*args):
            original = os.fsync
            def hang_directory(fd):
                if stat.S_ISDIR(os.fstat(fd).st_mode):
                    time.sleep(30)
                return original(fd)
            with patch.object(os, "fsync", hang_directory):
                W._write(*args)
        r = W._publish(DATA, self.path, NAME, HASH, _seconds=.12, _writer=writer)
        self.assertFalse(r["ready"])
        self.assertEqual(r["stage"], "directory_fsync")
        self.assertEqual(r["code"], "E_DESKTOP_IO_TIMEOUT")
        self.assertTrue(r["signal_sent"] and r["child_reaped"])
        self.assertEqual((self.path / NAME).read_bytes(), DATA)

    def test_file_fsync_error_keeps_nonexecutable_partial_and_phase(self):
        def writer(*args):
            with patch.object(os, "fsync", side_effect=OSError("must not leak raw error")):
                W._write(*args)
        r = self.publish(_writer=writer)
        self.assertEqual((r["ready"], r["code"], r["stage"]),
                         (False, "E_IO_FAILURE", "file_fsync"))
        self.assertEqual((self.path / NAME).read_bytes(), DATA)
        self.assertEqual(stat.S_IMODE((self.path / NAME).stat().st_mode), 0o600)
        self.assertNotIn("must not leak", str(r))

    def test_readback_corruption_fails_not_ready(self):
        def writer(data, path, name, expected, emit):
            def stage(value):
                if value == "readback":
                    (path / name).write_bytes(b"corrupt")
                emit(value)
            W._write(data, path, name, expected, stage)
        r = self.publish(_writer=writer)
        self.assertEqual(r["code"], "E_READBACK_HASH")
        self.assertFalse(r["ready"])

    def test_no_replace_retains_existing_bytes(self):
        (self.path / NAME).write_bytes(b"existing")
        r = self.publish()
        self.assertEqual(r["code"], "E_DESTINATION_EXISTS")
        self.assertEqual(r["stage"], "create_exclusive")
        self.assertEqual((self.path / NAME).read_bytes(), b"existing")

    def test_directory_and_destination_symlinks_denied(self):
        (self.path / NAME).symlink_to(self.path / "elsewhere")
        self.assertEqual(self.publish()["code"], "E_DESTINATION_EXISTS")
        alias = self.path.parent / "alias"
        alias.symlink_to(self.path, target_is_directory=True)
        r = W._publish(DATA, alias, NAME, HASH, _seconds=1)
        self.assertEqual(r["code"], "E_DIRECTORY_UNSAFE")

    def test_source_hash_and_name_denied_before_fork(self):
        with patch.object(os, "fork", side_effect=AssertionError("must not fork")):
            for name, hash_ in [("../bad.command", HASH), (NAME, "0" * 64)]:
                with self.assertRaises(W.PublicationError):
                    W._publish(DATA, self.path, name, hash_)

    def test_complete_event_without_exit_zero_never_ready(self):
        def writer(*args):
            W._write(*args)
            os._exit(3)
        r = self.publish(_writer=writer)
        self.assertEqual(r["exit_code"], 3)
        self.assertFalse(r["ready"])

    def test_exit_zero_without_completion_never_ready(self):
        r = self.publish(_writer=lambda *args: None)
        self.assertEqual(r["exit_code"], 0)
        self.assertFalse(r["ready"])
        self.assertEqual(r["code"], "E_WRITER_INCOMPLETE")

    def test_identity_drift_on_timeout_never_signals(self):
        calls = []
        first = True
        def identity(pid):
            nonlocal first
            observed = W._identity(pid)
            if first:
                first = False
                return observed
            return (observed[0], observed[1] + 1, *observed[2:])
        r = W._publish(DATA, self.path, NAME, HASH, _seconds=.08,
                       _writer=lambda *args: time.sleep(.18),
                       _identity_fn=identity, _signal_fn=lambda *a: calls.append(a))
        self.assertFalse(r["ready"] or r["signal_sent"])
        self.assertTrue(r["child_reaped"])
        self.assertEqual(calls, [])
        self.assertEqual(r["code"], "E_DESKTOP_IO_PENDING_IDENTITY")

    def test_preexisting_process_never_a_signal_target(self):
        calls = []
        me = W._identity(os.getpid())
        with self.assertRaises(W.PublicationError):
            W._signal_owned(os.getpid(), me, W._identity, lambda *a: calls.append(a))
        self.assertEqual(calls, [])

    def test_unknown_initial_identity_never_releases_writer(self):
        def no_identity(pid):
            raise W.PublicationError("E_WRITER_IDENTITY_UNAVAILABLE")
        r = self.publish(_identity_fn=no_identity)
        self.assertFalse(r["ready"] or r["signal_sent"])
        self.assertTrue(r["child_reaped"])
        self.assertFalse((self.path / NAME).exists())

    def test_production_deadline_not_cli_configurable(self):
        self.assertEqual(W.DEADLINE_SECONDS, 60)
        source = Path(SPEC.origin).read_text()
        self.assertNotIn('add_argument("--deadline', source)
        self.assertNotIn('add_argument("--destination', source)
        self.assertEqual(ctypes.sizeof(W._Info), 136)


if __name__ == "__main__":
    unittest.main()
