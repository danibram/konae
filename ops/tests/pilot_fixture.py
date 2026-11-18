"""Prepare and check the synthetic installations used by the writer pilot."""
from collections import Counter
from contextlib import closing
import hashlib
import json
from pathlib import Path
import random
import runpy
import shutil
import sqlite3
import struct
from urllib.error import HTTPError
from urllib.request import Request, urlopen

smoke = Path(__file__).resolve().parents[2] / "tests/smoke"
year = runpy.run_path(str(smoke / "recovery-year.py"))
cut = year["cut"]


def small_profile(binary, live, env):
    year["fixture"](live)
    model = year["Model"]()
    if binary.name == "konae":
        app = live / "apps/probe"
        for name in ["public-a", "public-b"]:
            copy = live / "apps" / name
            shutil.copytree(app, copy)
            config = json.loads((copy / "konae.json").read_bytes())
            config.update(id=name, slug=name, modules={})
            (copy / "konae.json").write_text(json.dumps(config))
            (copy / "app/actions.json").unlink()
        with cut["server"](binary.with_name("kernel"), env):
            pass
    with cut["server"](binary, env) as (port, _):
        if binary.name == "konae":
            cookie, can = year["owner_session"](port, env, signup=True)
            assert can["write"]
            cut["rpc"](port, "project_create", {"projectId": "draft", "name": "Draft"}, cookie)
            assert cut["rpc"](port, "project_inspect", {"projectId": "draft"}, cookie)["deployment"] is None
            model.published.update(["probe", "public-a", "public-b"])
            model.projects.update(model.published | {"draft"})
            year["check_projects"](port, cookie, model)
        for email in year["MEMBERS"][:2]:
            model.users[email], cookie = year["account"](port, email, "signup")
            year["check_rows"](port, cookie, email, model)
    if binary.name == "konae":
        folder = live / "apps/draft/app"
        folder.mkdir(exist_ok=True)
        (folder / "index.html").write_text("<main>Unpublished edit</main>")
    year["import_version"](binary.with_name("kernel"), env, live, model, random.Random(45), 1)
    return model


def session(port, app, role):
    host = app["hostname"]
    _, headers = cut["request"](port, "/_konae/auth/login", {
        "email": role + "@copy.test", "password": "copy-restore-password-2026"},
        {"Host": host, "Sec-Fetch-Site": "same-origin"})
    return {"Host": host, "Cookie": headers["Set-Cookie"].split(";")[0]}


def invoke(port, headers, action, inputs):
    call = {"name": "actions_invoke", "arguments": {"name": action, "input": inputs}}
    envelope = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": call}
    result = cut["request"](port, "/_konae/mcp", envelope, headers)[0]["result"]
    assert not result["isError"], result["content"]
    return result["structuredContent"]["result"]


def six_access(binary, env, root, profile):
    apps = json.loads((profile / "apps.json").read_bytes())
    files = json.loads((profile / "expected.json").read_bytes())
    assert len(apps) == 6 and len(files) == 255
    with cut["server"](binary, dict(env, KONAE_DATA_DIR=str(root))) as (port, _):
        _, headers = cut["request"](port, "/_konae/owner/login", {
            "email": "owner@copy.test", "password": "copy-restore-password-2026"})
        owner = headers["Set-Cookie"].split(";")[0]
        assert cut["rpc"](port, "session_inspect", {}, owner)["can"]["read"]
        for i, app in enumerate(apps):
            project = cut["rpc"](port, "project_inspect", {"projectId": app["projectId"]}, owner)
            assert project["deployment"]["hostname"] == app["hostname"]
            file = next(f for f in reversed(files) if f["app"] == app["id"])
            url = f"http://127.0.0.1:{port}/_konae/files/{file['id']}"
            user = session(port, app, "writer")
            assert invoke(port, user, "notes.list", {}) == [{"id": 1, "title": f"Synthetic {i}"}]
            with urlopen(Request(url, headers=user), timeout=20) as response:
                assert hashlib.sha256(response.read()).hexdigest() == file["sha256"]
            other = session(port, app, "reader")
            assert invoke(port, other, "notes.list", {}) == []
            try:
                with urlopen(Request(url, headers=other), timeout=20):
                    raise AssertionError("Second user read a private file")
            except HTTPError as error:
                assert error.code == 404


def sqlite_hash(path):
    def field(value):
        if value is None:
            return b"n"
        if isinstance(value, int):
            return b"i" + struct.pack(">q", value)
        if isinstance(value, float):
            return b"f" + struct.pack(">d", value)
        if isinstance(value, str):
            return b"t" + value.encode()
        return b"b" + value

    def row_hash(row):
        h = hashlib.sha256()
        for value in map(field, row):
            h.update(struct.pack(">Q", len(value)))
            h.update(value)
        return h.digest()

    with closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)) as db:
        db.execute("BEGIN")
        assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
        schema = db.execute("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name").fetchall()
        tables = {}
        for kind, name, _, _ in schema:
            if kind == "table":
                quoted = '"' + name.replace('"', '""') + '"'
                rows = sorted(map(row_hash, db.execute("SELECT * FROM " + quoted)))
                tables[name] = len(rows), hashlib.sha256(b"".join(rows)).digest()
        return schema, db.execute("PRAGMA user_version").fetchone(), tables


def excluded(path):
    parts = path.parts
    return (path.name.endswith((".sqlite-wal", ".sqlite-shm", ".sqlite-journal"))
            or parts[0] in {"kernel.lock", "mail-key", "files-store.json", "transfers", ".copy-health", ".copy-state.json",
                            ".copy-staging", ".copy-operations", "google.sqlite"})


def inventory(root, source=False):
    files = {}
    for p in sorted(root.rglob("*")):
        rel = p.relative_to(root)
        if not p.is_file() or (source and excluded(rel)):
            continue
        if p.name.endswith((".sqlite-wal", ".sqlite-shm", ".sqlite-journal")):
            # These sidecars can appear when the offline observer opens a WAL database.
            continue
        if p.suffix == ".sqlite":
            files[rel] = sqlite_hash(p)
        else:
            with p.open("rb") as f:
                files[rel] = hashlib.file_digest(f, "sha256").digest()
    return files


def compare(expected, actual):
    missing = expected.keys() - actual.keys()
    extra = actual.keys() - expected.keys()
    different = [p for p in expected.keys() & actual.keys() if expected[p] != actual[p]]
    if missing or extra or different:
        raise AssertionError(f"Cut mismatch: missing {sorted(missing)}, extra {sorted(extra)}, changed {different}")
    tables = [v[2] for p, v in actual.items() if p.suffix == ".sqlite"]
    return dict(paths=len(actual), sqlite=len(tables), tables=sum(map(len, tables)),
                rows=sum(n for t in tables for n, _ in t.values()))


def check_access(binary, clean, dest, profile, model):
    if model is None:
        six_access(binary, clean, dest, profile)
        return {"apps": 6, "ownerCatalogChecks": 6, "rowPermissionChecks": 12,
                "fileReadsAllowed": 6, "fileReadsDenied": 6}
    counts = Counter()
    year["verify"](binary, clean, dest, model, set(), counts)
    if binary.name == "konae":
        draft = dest / "apps/draft"
        assert (draft / "app/index.html").read_text() == "<main>Unpublished edit</main>"
        assert not (draft / "dist").exists() and not list(draft.glob("data/*.sqlite"))
    else:
        assert not any((dest / p).exists() for p in ("owners.sqlite", "mcp.sqlite"))
    return dict(counts)
