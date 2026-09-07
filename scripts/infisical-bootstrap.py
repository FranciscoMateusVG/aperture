#!/usr/bin/env python3
"""infisical-bootstrap - one-record, non-model credential bootstrap adapter.

Bead: aperture-a4ph5.  Operator authorised ONE whole-record transient read
(2026-09-07).  Design gate: Cipher (aperture-5nxd8); revision 2 applies his
exact corrections.  NO real registry read or auth until Cipher PASSES this
source AND GLaDOS issues the bounded execution dispatch.

WHAT THIS IS
  Reads ONE pinned record from the local mempalace SQLite store by exact
  collection + immutable record id, verifies a non-secret metadata
  discriminator BEFORE any document text is selected, then extracts exactly
  two named fields under a pinned serialization grammar and publishes them to
  the already-reviewed credential file.

WHAT THIS IS NOT
  Not a search.  There is no LIKE, no FTS, no similarity query, no list, no
  peek, no collection enumeration, and no chromadb import - the store is
  opened read-only/immutable and queried with fixed SQL by primary key.  No
  SQLite extension is loaded and no pickle is read.  There is no code path
  that can address a second record.

HONEST LIMITATION ON ZEROIZATION (Cipher, required)
  Python cannot guarantee zeroization.  The record document and the extracted
  values may transiently persist in the interpreter's allocator and in OS
  memory after this program drops its references, and this process cannot
  scrub them.  What IS guaranteed: they are never printed, logged, written
  anywhere but the fixed destination, returned, fingerprinted, or attached to
  an exception - and the process exits immediately after publication.
"""

from __future__ import annotations

import json
import os
import re
import sqlite3
import stat
import sys

# ─── COMPILED LOCATOR - build-time constants only ─────────────────────────
# None of these is a CLI argument, an environment variable, or discovered at
# runtime.  Changing any is a source edit that returns to review.
STORE_DB = os.path.join(os.path.expanduser("~"), ".mempalace", "palace",
                        "chroma.sqlite3")
COLLECTION = "mempalace_drawers"

# Chroma stores a record's document as a metadata row under this internal key.
# Pinned as a constant rather than discovered; flagged for Cipher's review.
DOCUMENT_KEY = "chroma:document"

# ── OPERATOR-SUPPLIED INPUTS.  Empty = the adapter refuses to run. ────────
# Deliberately NOT discoverable by this program: obtaining them would require
# the corpus search this design exists to avoid.
RECORD_ID = ""                 # (2) exact immutable record id
DISCRIMINATOR_KEY = ""         # (3) non-secret metadata key identifying the record
DISCRIMINATOR_VALUE = ""       # (3) its exact expected value
FIELD_CLIENT_ID = ""           # (4) literal label carrying the client id
FIELD_CLIENT_SECRET = ""       # (4) literal label carrying the client secret
# (4) serialization grammar: a regex with ONE capture group; {LABEL} is
# substituted with the escaped field label.  Supplied by the operator, not
# guessed by me.  Example shape only - NOT a default:
GRAMMAR = ""                   # e.g. r"^[ \t]*{LABEL}[ \t]*:[ \t]*(\S+)[ \t]*$"

EXPECTED_COUNT = 1
MAX_DOC_BYTES = 262144
MAX_FIELD_LEN = 512

DEST_PATH = os.path.join(os.path.expanduser("~"), ".config", "aperture",
                         "infisical-peppy-admin.env")
DEST_DIR = os.path.dirname(DEST_PATH)

ONLY_ACTION = "bootstrap"


class Fail(Exception):
    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


E_NOT_ARMED = "E_NOT_ARMED"
E_UNSAFE_RUNTIME = "E_UNSAFE_RUNTIME"
E_BAD_ACTION = "E_BAD_ACTION"
E_STORE_MISSING = "E_STORE_MISSING"
E_COLLECTION_MISMATCH = "E_COLLECTION_MISMATCH"
E_RECORD_NOT_FOUND = "E_RECORD_NOT_FOUND"
E_COUNT_MISMATCH = "E_COUNT_MISMATCH"
E_DISCRIMINATOR_MISMATCH = "E_DISCRIMINATOR_MISMATCH"
E_DOC_TOO_LARGE = "E_DOC_TOO_LARGE"
E_FIELD_MISSING = "E_FIELD_MISSING"
E_FIELD_AMBIGUOUS = "E_FIELD_AMBIGUOUS"
E_FIELD_INVALID = "E_FIELD_INVALID"
E_DEST_DIR_PERMS = "E_DEST_DIR_PERMS"
E_DEST_EXISTS = "E_DEST_EXISTS"
E_WRITE_FAILED = "E_WRITE_FAILED"
E_VERIFY_FAILED = "E_VERIFY_FAILED"
E_INTERNAL = "E_INTERNAL"


def assert_armed() -> None:
    for value in (RECORD_ID, DISCRIMINATOR_KEY, DISCRIMINATOR_VALUE,
                  FIELD_CLIENT_ID, FIELD_CLIENT_SECRET, GRAMMAR):
        if not value:
            raise Fail(E_NOT_ARMED)


def assert_safe_runtime() -> None:
    """Detects an instrumented interpreter.  Does NOT protect against code
    already imported before main() runs - stated, not implied."""
    for var in ("PYTHONSTARTUP", "PYTHONPATH", "PYTHONINSPECT",
                "PYTHONBREAKPOINT", "PYTHONDEVMODE", "PYTHONPROFILEIMPORTTIME",
                "PYTHONVERBOSE"):
        if os.environ.get(var):
            raise Fail(E_UNSAFE_RUNTIME)
    if sys.flags.inspect or sys.flags.debug or sys.flags.verbose:
        raise Fail(E_UNSAFE_RUNTIME)
    if sys.gettrace() is not None or sys.getprofile() is not None:
        raise Fail(E_UNSAFE_RUNTIME)


def _open_readonly(path: str) -> sqlite3.Connection:
    if not os.path.isfile(path):
        raise Fail(E_STORE_MISSING)
    try:
        # Read-only AND immutable: no writes, no journal, no recovery pass.
        conn = sqlite3.connect("file:" + path + "?mode=ro&immutable=1",
                               uri=True, timeout=5)
        conn.execute("PRAGMA query_only = ON")
        return conn
    except Exception:
        raise Fail(E_STORE_MISSING)


# Fixed SQL.  Exact collection name and exact record id are the only inputs;
# both are bound parameters, never interpolated.  No LIKE, no FTS, no scan.
_SQL_META = (
    "SELECT em.string_value "
    "FROM collections c "
    "JOIN segments s ON s.collection = c.id "
    "JOIN embeddings e ON e.segment_id = s.id "
    "JOIN embedding_metadata em ON em.id = e.id "
    "WHERE c.name = ? AND e.embedding_id = ? AND em.key = ?"
)


def _fetch_single(conn: sqlite3.Connection, key: str, missing_code: str) -> str:
    try:
        rows = conn.execute(_SQL_META, (COLLECTION, RECORD_ID, key)).fetchall()
    except Exception:
        raise Fail(E_STORE_MISSING)
    if not rows:
        raise Fail(missing_code)
    if len(rows) != EXPECTED_COUNT:
        raise Fail(E_COUNT_MISMATCH)
    value = rows[0][0]
    if not isinstance(value, str):
        raise Fail(missing_code)
    return value


def fetch_record_document(conn: sqlite3.Connection) -> str:
    """Two-phase: the discriminator is verified BEFORE any document text is
    selected, so a wrong record id stops WITHOUT document retrieval."""
    # Phase 1 - non-secret discriminator only.
    discriminator = _fetch_single(conn, DISCRIMINATOR_KEY,
                                  E_DISCRIMINATOR_MISMATCH)
    if discriminator != DISCRIMINATOR_VALUE:
        raise Fail(E_DISCRIMINATOR_MISMATCH)
    # Phase 2 - only now is document text selected.
    document = _fetch_single(conn, DOCUMENT_KEY, E_RECORD_NOT_FOUND)
    if len(document.encode("utf-8")) > MAX_DOC_BYTES:
        raise Fail(E_DOC_TOO_LARGE)
    return document


def extract_field(document: str, label: str) -> str:
    """Exactly-once extraction under the pinned grammar.  Never echoes the
    document; any ambiguity fails rather than guesses, because guessing here
    means publishing the wrong secret."""
    pattern = re.compile(GRAMMAR.replace("{LABEL}", re.escape(label)),
                         re.MULTILINE)
    matches = pattern.findall(document)
    if not matches:
        raise Fail(E_FIELD_MISSING)
    if len(matches) > 1:
        raise Fail(E_FIELD_AMBIGUOUS)
    value = matches[0]
    if not isinstance(value, str):
        raise Fail(E_FIELD_INVALID)
    value = value.strip()
    if not value or len(value) > MAX_FIELD_LEN:
        raise Fail(E_FIELD_INVALID)
    if any(ord(ch) < 0x20 or ord(ch) == 0x7F for ch in value):
        raise Fail(E_FIELD_INVALID)
    if "\n" in value or "\r" in value:
        raise Fail(E_FIELD_INVALID)
    return value


def publish(client_id: str, client_secret: str) -> None:
    """Atomic AND no-overwrite.  Publishes by hard-linking a private temp onto
    an ABSENT destination; never renames over an existing file."""
    if not os.path.isdir(DEST_DIR):
        raise Fail(E_DEST_DIR_PERMS)
    dir_st = os.lstat(DEST_DIR)
    if dir_st.st_uid != os.getuid() or (dir_st.st_mode & 0o077):
        raise Fail(E_DEST_DIR_PERMS)
    if os.path.lexists(DEST_PATH):
        raise Fail(E_DEST_EXISTS)          # refuse rather than clobber

    body = (
        "INFISICAL_CLIENT_ID=" + client_id + "\n"
        "INFISICAL_CLIENT_SECRET=" + client_secret + "\n"
    ).encode("utf-8")

    tmp = os.path.join(DEST_DIR, ".infisical-bootstrap.%d.tmp" % os.getpid())
    fd = None
    try:
        fd = os.open(tmp, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW,
                     0o600)
        st = os.fstat(fd)
        if (not stat.S_ISREG(st.st_mode) or st.st_uid != os.getuid()
                or st.st_nlink != 1 or (st.st_mode & 0o077)):
            raise Fail(E_WRITE_FAILED)
        written = os.write(fd, body)
        if written != len(body):
            raise Fail(E_WRITE_FAILED)
        os.fsync(fd)
        os.close(fd)
        fd = None

        os.link(tmp, DEST_PATH)            # fails if destination exists
        dir_fd = os.open(DEST_DIR, os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    except Fail:
        raise
    except FileExistsError:
        raise Fail(E_DEST_EXISTS)
    except Exception:
        raise Fail(E_WRITE_FAILED)
    finally:
        if fd is not None:
            try:
                os.close(fd)
            except OSError:
                pass
        if os.path.lexists(tmp):
            try:
                os.unlink(tmp)
            except OSError:
                pass

    # Re-open and verify the published file is what we expect.
    try:
        vfd = os.open(DEST_PATH, os.O_RDONLY | os.O_NOFOLLOW)
    except Exception:
        raise Fail(E_VERIFY_FAILED)
    try:
        vst = os.fstat(vfd)
        if (not stat.S_ISREG(vst.st_mode) or vst.st_uid != os.getuid()
                or (vst.st_mode & 0o077) or vst.st_size != len(body)
                or vst.st_nlink != 1):
            raise Fail(E_VERIFY_FAILED)
    finally:
        os.close(vfd)


def bootstrap() -> dict:
    assert_armed()
    assert_safe_runtime()
    conn = _open_readonly(STORE_DB)
    try:
        document = fetch_record_document(conn)
    finally:
        try:
            conn.close()
        except Exception:
            pass
    client_id = extract_field(document, FIELD_CLIENT_ID)
    client_secret = extract_field(document, FIELD_CLIENT_SECRET)
    del document
    publish(client_id, client_secret)
    del client_id, client_secret
    # Bounded CONSTANT receipt.  No hash, no length, no oracle of any kind.
    return {"ok": True, "code": "BOOTSTRAP_COMPLETE", "fields": 2}


def main(argv: list[str]) -> int:
    if len(argv) != 1 or argv[0] != ONLY_ACTION:
        print(json.dumps({"ok": False, "error": E_BAD_ACTION}))
        return 2
    try:
        print(json.dumps(bootstrap()))
        return 0
    except Fail as failure:
        print(json.dumps({"ok": False, "error": failure.code}))
        return 1
    except Exception:
        print(json.dumps({"ok": False, "error": E_INTERNAL}))
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
