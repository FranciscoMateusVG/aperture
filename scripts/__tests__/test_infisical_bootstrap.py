"""Synthetic tests for scripts/infisical-bootstrap.py (aperture-a4ph5).

ISOLATION CONTRACT: these tests NEVER touch the real mempalace store and never
publish to the real credential path.  Every constant in the module under test
is monkeypatched to a throwaway temp directory built by the test itself.  The
adapter is unarmed in source, so an accidental real run is refused anyway.

Canaries are INJECTED into record documents and asserted absent from every
output and every thrown error.

Run: python3 -m unittest discover -s scripts/__tests__ -p "test_*.py" -v
(stdlib unittest only - no third-party test dependency is introduced.)
"""
import contextlib
import importlib.util
import io
import os
import sqlite3
import stat
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
MODPATH = os.path.join(HERE, "..", "infisical-bootstrap.py")

SECRET_CANARY = "CANARY_SECRET_VALUE_ee6f1b2c"
ID_CANARY = "CANARY_CLIENT_ID_9a3d7f04"
GRAMMAR = r"^[ \t]*{LABEL}[ \t]*:[ \t]*(\S+)[ \t]*$"


def load():
    spec = importlib.util.spec_from_file_location("bootstrap_mod", MODPATH)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def make_store(tmp, *, collection="mempalace_drawers", record_id="rec-1",
               disc_key="title", disc_value="PocketSoftware Infisical",
               document=None, extra_rows=0):
    """Build a throwaway SQLite store with the same shape the adapter queries."""
    db = os.path.join(tmp, "chroma.sqlite3")
    conn = sqlite3.connect(db)
    conn.executescript(
        "CREATE TABLE collections(id TEXT PRIMARY KEY, name TEXT);"
        "CREATE TABLE segments(id TEXT PRIMARY KEY, collection TEXT);"
        "CREATE TABLE embeddings(id INTEGER PRIMARY KEY, segment_id TEXT,"
        " embedding_id TEXT);"
        "CREATE TABLE embedding_metadata(id INTEGER, key TEXT,"
        " string_value TEXT);"
    )
    conn.execute("INSERT INTO collections VALUES('c1',?)", (collection,))
    conn.execute("INSERT INTO segments VALUES('s1','c1')")
    conn.execute("INSERT INTO embeddings VALUES(1,'s1',?)", (record_id,))
    conn.execute("INSERT INTO embedding_metadata VALUES(1,?,?)",
                 (disc_key, disc_value))
    if document is not None:
        conn.execute("INSERT INTO embedding_metadata VALUES(1,'chroma:document',?)",
                     (document,))
    for i in range(extra_rows):
        conn.execute("INSERT INTO embedding_metadata VALUES(1,'chroma:document',?)",
                     ("duplicate %d" % i,))
    conn.commit()
    conn.close()
    return db


def arm(mod, tmp, db, *, record_id="rec-1", disc_key="title",
        disc_value="PocketSoftware Infisical"):
    dest_dir = os.path.join(tmp, "cfg")
    os.makedirs(dest_dir, mode=0o700, exist_ok=True)
    os.chmod(dest_dir, 0o700)
    mod.STORE_DB = db
    mod.RECORD_ID = record_id
    mod.DISCRIMINATOR_KEY = disc_key
    mod.DISCRIMINATOR_VALUE = disc_value
    mod.FIELD_CLIENT_ID = "INFISICAL_CLIENT_ID"
    mod.FIELD_CLIENT_SECRET = "INFISICAL_CLIENT_SECRET"
    mod.GRAMMAR = GRAMMAR
    mod.DEST_DIR = dest_dir
    mod.DEST_PATH = os.path.join(dest_dir, "infisical-peppy-admin.env")
    return mod.DEST_PATH


GOOD_DOC = (
    "Drawer: PocketSoftware Infisical\n"
    "INFISICAL_CLIENT_ID: " + ID_CANARY + "\n"
    "INFISICAL_CLIENT_SECRET: " + SECRET_CANARY + "\n"
    "note: escrow only\n"
)


def surface(exc):
    return "|".join([str(getattr(exc, "code", "")), str(exc), repr(exc)])


# ── Armed/action gates ───────────────────────────────────────────────────
class BootstrapAdapterTests(unittest.TestCase):
    def test_refuses_while_unarmed(self):
        mod = load()
        with self.assertRaises(mod.Fail) as ctx:
            mod.bootstrap()
        self.assertEqual(ctx.exception.code, mod.E_NOT_ARMED)


    def test_rejects_unknown_actions(self):
        mod = load()
        for argv in (["dump"], ["read"], ["bootstrap", "--all"], [], ["BOOTSTRAP"]):
            self.assertEqual(mod.main(argv), 2)


    # ── Happy path ───────────────────────────────────────────────────────────
    def test_happy_path_publishes_and_receipt_is_a_bounded_constant(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=GOOD_DOC)
            dest = arm(mod, tmp, db)
            receipt = mod.bootstrap()
            self.assertEqual(receipt, {"ok": True, "code": "BOOTSTRAP_COMPLETE", "fields": 2})
            # No hash, no length, no oracle of any kind.
            blob = repr(receipt)
            self.assertTrue("sha" not in blob.lower() and "bytes" not in blob.lower())
            self.assertTrue(SECRET_CANARY not in blob and ID_CANARY not in blob)
            # File landed with the right content and 0600.
            st = os.lstat(dest)
            self.assertTrue(stat.S_ISREG(st.st_mode))
            self.assertEqual(st.st_mode & 0o777, 0o600)
            self.assertEqual(st.st_nlink, 1)
            body = open(dest, encoding="utf-8").read()
            expected = ("INFISICAL_CLIENT_ID=" + ID_CANARY + "\n"
                        "INFISICAL_CLIENT_SECRET=" + SECRET_CANARY + "\n")
            self.assertEqual(body, expected)


    # ── No-overwrite publication ─────────────────────────────────────────────
    def test_refuses_to_overwrite_an_existing_destination(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=GOOD_DOC)
            dest = arm(mod, tmp, db)
            with open(dest, "w", encoding="utf-8") as fh:
                fh.write("PRE-EXISTING\n")
            os.chmod(dest, 0o600)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_DEST_EXISTS)
            self.assertEqual(open(dest, encoding="utf-8").read(), "PRE-EXISTING\n")


    def test_refuses_a_world_readable_parent(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=GOOD_DOC)
            arm(mod, tmp, db)
            os.chmod(mod.DEST_DIR, 0o755)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_DEST_DIR_PERMS)


    def test_leaves_no_temp_file_behind_on_failure(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=GOOD_DOC)
            dest = arm(mod, tmp, db)
            open(dest, "w").close()
            os.chmod(dest, 0o600)
            with self.assertRaises(mod.Fail):
                mod.bootstrap()
            leftovers = [f for f in os.listdir(mod.DEST_DIR) if ".tmp" in f]
            self.assertEqual(leftovers, [])


    # ── Discriminator gate: wrong record stops BEFORE document retrieval ─────
    def test_wrong_discriminator_stops_without_touching_the_document(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, disc_value="Some Other Drawer", document=GOOD_DOC)
            arm(mod, tmp, db)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_DISCRIMINATOR_MISMATCH)
            self.assertTrue(SECRET_CANARY not in surface(ctx.exception))


    def test_missing_discriminator_fails_closed(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, disc_key="other_key", document=GOOD_DOC)
            arm(mod, tmp, db)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_DISCRIMINATOR_MISMATCH)


    def test_wrong_record_id_never_reaches_a_document(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=GOOD_DOC)
            arm(mod, tmp, db, record_id="rec-does-not-exist")
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_DISCRIMINATOR_MISMATCH)
            self.assertTrue(SECRET_CANARY not in surface(ctx.exception))


    def test_collection_mismatch_fails_closed(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, collection="some_other_collection", document=GOOD_DOC)
            arm(mod, tmp, db)
            with self.assertRaises(mod.Fail):
                mod.bootstrap()


    # ── Extraction strictness ────────────────────────────────────────────────
    def test_duplicate_field_is_ambiguous_and_fails(self):
        mod = load()
        doc = GOOD_DOC + "INFISICAL_CLIENT_ID: second-value\n"
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=doc)
            arm(mod, tmp, db)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_FIELD_AMBIGUOUS)


    def test_missing_field_fails(self):
        mod = load()
        doc = "Drawer: PocketSoftware Infisical\nINFISICAL_CLIENT_ID: x\n"
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=doc)
            arm(mod, tmp, db)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_FIELD_MISSING)


    def test_control_characters_in_a_value_are_rejected(self):
        mod = load()
        doc = GOOD_DOC.replace(ID_CANARY, "abc\x00def")
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=doc)
            arm(mod, tmp, db)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertTrue(ctx.exception.code in (mod.E_FIELD_INVALID, mod.E_FIELD_MISSING))


    def test_oversize_document_is_refused(self):
        mod = load()
        doc = GOOD_DOC + ("x" * 300000)
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=doc)
            arm(mod, tmp, db)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_DOC_TOO_LARGE)


    def test_duplicate_document_rows_fail_count_check(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=GOOD_DOC, extra_rows=1)
            arm(mod, tmp, db)
            with self.assertRaises(mod.Fail) as ctx:
                mod.bootstrap()
            self.assertEqual(ctx.exception.code, mod.E_COUNT_MISMATCH)


    # ── Canary containment across every failure surface ──────────────────────
    def test_no_canary_in_any_error_surface(self):
        mod = load()
        cases = [
            dict(document=GOOD_DOC + "INFISICAL_CLIENT_ID: dup\n"),
            dict(document=GOOD_DOC, extra_rows=1),
            dict(document=GOOD_DOC, disc_value="wrong"),
        ]
        for kw in cases:
            with tempfile.TemporaryDirectory() as tmp:
                db = make_store(tmp, **kw)
                arm(mod, tmp, db)
                with self.assertRaises(mod.Fail) as ctx:
                    mod.bootstrap()
                s = surface(ctx.exception)
                self.assertTrue(SECRET_CANARY not in s)
                self.assertTrue(ID_CANARY not in s)


    def test_cli_output_on_failure_is_a_stable_code_only(self):
        mod = load()
        with tempfile.TemporaryDirectory() as tmp:
            db = make_store(tmp, document=GOOD_DOC, disc_value="wrong")
            arm(mod, tmp, db)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = mod.main(["bootstrap"])
            out = buf.getvalue()
            self.assertEqual(rc, 1)
            self.assertTrue(SECRET_CANARY not in out and ID_CANARY not in out)
            self.assertTrue("E_DISCRIMINATOR_MISMATCH" in out)


    # ── Runtime guard ────────────────────────────────────────────────────────
    def test_instrumented_runtime_is_refused(self):
        mod = load()
        for var in ("PYTHONSTARTUP", "PYTHONPATH", "PYTHONINSPECT",
                    "PYTHONBREAKPOINT", "PYTHONDEVMODE", "PYTHONVERBOSE"):
            os.environ[var] = "1"
            with self.assertRaises(mod.Fail) as ctx:
                mod.assert_safe_runtime()
            self.assertEqual(ctx.exception.code, mod.E_UNSAFE_RUNTIME)
            os.environ.pop(var, None)


    def test_tracer_attached_is_refused(self):
        mod = load()
        old = sys.gettrace()
        sys.settrace(lambda *a: None)
        try:
            with self.assertRaises(mod.Fail) as ctx:
                mod.assert_safe_runtime()
            self.assertEqual(ctx.exception.code, mod.E_UNSAFE_RUNTIME)
        finally:
            sys.settrace(old)


    def test_clean_runtime_passes(self):
        mod = load()
        self.assertTrue(mod.assert_safe_runtime() is None)


if __name__ == "__main__":
    unittest.main(verbosity=2)
