"""Replay a seeded app history and compare each published v3 point with its inputs."""
import argparse
from collections import Counter
from contextlib import closing
from dataclasses import dataclass, field
import copy
import hashlib
import json
import os
from pathlib import Path
import random
import runpy
import shutil
import sqlite3
import subprocess
import tempfile
import time
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import Request, urlopen
import uuid

cut = runpy.run_path(str(Path(__file__).with_name("common-cut.py")))
server, request = cut["server"], cut["request"]
HOST = {"Host": "probe.localhost", "Sec-Fetch-Site": "same-origin"}
FILE = "00000000-0000-4000-8000-000000000001"
EXTRA = "00000000-0000-4000-8000-000000000002"
MEMBERS = ["ana@example.test", "bea@example.test", "cy@example.test"]
SCHEMA = "CREATE TABLE IF NOT EXISTS notes(title TEXT PRIMARY KEY, owner TEXT NOT NULL);"
FIRST_KEY = "first-synthetic-key-for-the-year-campaign"
NEXT_KEY = "second-synthetic-key-for-the-year-campaign"


@dataclass
class Model:
    rows: dict = field(default_factory=dict)
    users: dict = field(default_factory=dict)
    files: dict = field(default_factory=dict)
    projects: set = field(default_factory=set)
    published: set = field(default_factory=set)


class Divergence(AssertionError):
    pass


def equal(actual, expected, check):
    if actual != expected:
        raise Divergence(check)


def query(database, sql):
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as db:
        return db.execute(sql).fetchall()


def account(port, email, operation):
    data, headers = request(port, "/_konae/auth/" + operation,
                            {"email": email, "password": "synthetic-password-for-recovery"}, HOST)
    return data["result"]["user"]["userId"], headers["Set-Cookie"].split(";")[0]


def owner_session(port, env, signup=False):
    operation = "signup" if signup else "login"
    data = {"email": "owner@example.test", "password": "synthetic-owner-password"}
    if signup:
        data["setupKey"] = env["KONAE_SETUP_KEY"]
    _, headers = request(port, "/_konae/owner/" + operation, data)
    cookie = headers["Set-Cookie"].split(";")[0]
    return cookie, cut["rpc"](port, "session_inspect", {}, cookie)["can"]


def check_projects(port, cookie, model):
    projects = cut["rpc"](port, "projects_list", {}, cookie)["projects"]
    equal({p["project"]["id"] for p in projects}, model.projects, "owner project list")
    for project in sorted(model.projects):
        inspected = cut["rpc"](port, "project_inspect", {"projectId": project}, cookie)
        equal(inspected["project"]["id"], project, "owner opens project")
        equal(inspected["deployment"] is not None, project in model.published, "draft or published project")


def action(port, cookie, name, args):
    call = {"name": "actions_invoke", "arguments": {"name": name, "input": args}}
    result = request(port, "/_konae/mcp", {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                           "params": call}, dict(HOST, Cookie=cookie))[0]["result"]
    if result["isError"]:
        raise AssertionError(result["content"])
    return result["structuredContent"]["result"]


def check_rows(port, cookie, email, model):
    observed = [r["title"] for r in action(port, cookie, "notes.mine", {})]
    equal(observed, sorted(t for t, e in model.rows.items() if e == email), "row permissions")


def fixture(live):
    cut["fixture"](live)
    app = live / "apps/probe"
    (app / "app/sample.bin").unlink()
    config = json.loads((app / "konae.json").read_text())
    config["modules"].update(auth={"on": True}, files={"on": True})
    (app / "konae.json").write_text(json.dumps(config))
    (app / "app/schema.sql").write_text(SCHEMA)
    title = {"source": "input", "key": "title", "type": "string", "maxLength": 80}
    owner = {"source": "actor", "field": "id"}
    actions = {name: dict(kind=kind, access="authenticated", statement=sql, bindings=bindings)
               for name, kind, sql, bindings in [
                   ("notes.add", "mutation", "INSERT INTO notes VALUES(?1, ?2)", [title, owner]),
                   ("notes.drop", "mutation", "DELETE FROM notes WHERE title=?1 AND owner=?2", [title, owner]),
                   ("notes.mine", "query", "SELECT title FROM notes WHERE owner=?1 ORDER BY title LIMIT 500", [owner])]}
    (app / "app/actions.json").write_text(json.dumps({"version": 1, "actions": actions}))


def cli(binary, env, *args):
    result = subprocess.run([binary, *args], env=env, capture_output=True, timeout=90)
    if result.returncode:
        raise AssertionError(result.stderr.decode(errors="replace")[-1500:])
    return result.stdout


def import_version(binary, env, live, model, rng, generation):
    """Give the importer new rows and bytes at the same file ID."""
    owner = MEMBERS[generation % 2]
    title = f"import-{generation}"
    rows = {title: owner}
    files = {FILE: (owner, "private", rng.randbytes(512))}
    if generation % 2 == 0:
        files[EXTRA] = (owner, "public", rng.randbytes(128))
    with tempfile.TemporaryDirectory(dir=live.parent) as tmp:
        data = Path(tmp) / "data"
        assets = data / "files"
        assets.mkdir(parents=True)
        with closing(sqlite3.connect(data / "database.sqlite")) as db:
            db.execute(SCHEMA)
            db.execute("INSERT INTO notes VALUES(?, ?)", (title, model.users[owner]))
            db.commit()
        with closing(sqlite3.connect((live / "apps/probe/data/identity.sqlite").as_uri() + "?mode=ro",
                                     uri=True)) as src, closing(sqlite3.connect(data / "identity.sqlite")) as dst:
            src.backup(dst)
        index = []
        for fid, (email, visibility, body) in files.items():
            (assets / (fid + ".blob")).write_bytes(body)
            index.append(dict(id=fid, ownerId=model.users[email], visibility=visibility,
                              name="sample.bin", type="application/octet-stream", size=len(body)))
        (assets / "index.json").write_text(json.dumps(index))
        equal(json.loads(cli(binary, env, "import", tmp, "--app", "probe"))["imported"], True, "import")
    model.rows, model.files = rows, files


def check_file(port, fid, cookie, body):
    headers = dict(HOST, Cookie=cookie)
    try:
        with urlopen(Request(f"http://127.0.0.1:{port}/_konae/files/{fid}", headers=headers), timeout=10) as r:
            equal(r.status, 200, "file response")
            equal(r.read(), body, "file permission or bytes")
    except HTTPError as error:
        if error.code != 404 or body is not None:
            raise


def verify(binary, env, dest, model, suspended, counts):
    data = dest / "apps/probe/data"
    identity = data / "identity.sqlite"
    users = query(identity, "SELECT email, id, state FROM app_users ORDER BY email")
    equal(users, sorted((e, uid, "suspended" if e in suspended else "active")
                        for e, uid in model.users.items()), "restored accounts")
    equal(query(identity, "SELECT COUNT(*) FROM app_sessions"), [(0,)], "old sessions")
    equal(query(data / "database.sqlite", "SELECT title, owner FROM notes ORDER BY title"),
          sorted((t, model.users[e]) for t, e in model.rows.items()), "restored rows")
    assets = data / "files"
    entries = json.loads((assets / "index.json").read_text())
    equal(sorted((f["id"], f["ownerId"], f["visibility"], f["size"]) for f in entries),
          sorted((k, model.users[e], v, len(b)) for k, (e, v, b) in model.files.items()), "file index")
    equal({p.stem for p in assets.glob("*.blob")}, set(model.files), "blob inventory")
    if binary.name == "konae":
        owners = query(dest / "owners.sqlite", "SELECT email, id FROM app_users")
        equal([e for e, _ in owners], ["owner@example.test"], "restored owner")
        equal(query(dest / "mcp.sqlite", "SELECT id, owner FROM projects ORDER BY id"),
              sorted((project, owners[0][1]) for project in model.projects), "restored owner catalogue")
    local = {k: v for k, v in env.items() if not k.startswith("KONAE_S3_")}
    with server(binary, dict(local, KONAE_DATA_DIR=str(dest))) as (port, _):
        if binary.name == "konae":
            cookie, can = owner_session(port, env)
            equal(can["read"], True, "restored owner read access")
            check_projects(port, cookie, model)
            counts["ownerCatalogChecks"] += 1
        for email in MEMBERS[:2]:
            _, cookie = account(port, email, "login")
            check_rows(port, cookie, email, model)
            counts["rowPermissionChecks"] += 1
            for fid in [FILE, EXTRA]:
                asset = model.files.get(fid)
                allowed = asset and (asset[0] == email or asset[1] == "public")
                check_file(port, fid, cookie, asset[2] if allowed else None)
                counts["fileReadsAllowed" if allowed else "fileReadsDenied"] += 1
        if MEMBERS[2] in suspended:
            try:
                account(port, MEMBERS[2], "login")
            except HTTPError as error:
                equal(error.code, 403, "login of suspended account")
            else:
                raise AssertionError("suspended account logged in")


def capture(binary, env, previous, kinds):
    def finished():
        reports = cut["targets"](port)
        equal({r["kind"] for r in reports}, kinds, "configured destinations")
        for r in reports:
            if r["lastError"] and not r["inProgress"]:
                raise AssertionError(r["lastError"])
        ids = {r["lastCopyId"] for r in reports}
        if not any(r["inProgress"] for r in reports) and len(ids) == 1 and not ids & {None, previous}:
            return ids.pop()
    with server(binary, env) as (port, _):
        return cut["wait"](finished, "no new complete point", 120)


def remote_settings(mc):
    if not os.environ.get("KONAE_TEST_S3_ENDPOINT"):
        raise Divergence("Missing required s3-v3 destination")
    endpoint = urlsplit(os.environ["KONAE_TEST_S3_ENDPOINT"])
    if (endpoint.scheme, endpoint.hostname, os.environ["KONAE_TEST_S3_BUCKET"]) != (
            "http", "127.0.0.1", "konae-test") or not endpoint.port:
        raise ValueError("Expected the disposable loopback MinIO bucket konae-test")
    alias = urlsplit(os.environ["MC_HOST_lab"])
    if (alias.scheme, alias.hostname, alias.port) != (endpoint.scheme, endpoint.hostname, endpoint.port):
        raise ValueError("mc must use the disposable MinIO endpoint")
    entries = subprocess.check_output([mc, "ls", "--recursive", "--json", "lab/konae-test"], timeout=30)
    equal(entries.strip(), b"", "MinIO bucket must start empty")
    return {"KONAE_S3_" + k: os.environ["KONAE_TEST_S3_" + k]
            for k in ("ENDPOINT", "BUCKET", "ACCESS_KEY", "SECRET_KEY")}


def save(report, path):
    if path:
        pending = path.with_name(path.name + ".tmp")
        pending.write_text(json.dumps(report, sort_keys=True) + "\n")
        pending.replace(path)


def campaign(args, root, report):
    binary = args.binary.resolve(strict=True)
    with binary.open("rb") as image:
        report["binarySha256"] = hashlib.file_digest(image, "sha256").hexdigest()
    live = root / "live"
    fixture(live)
    rng = random.Random(args.seed)
    days, interval = (365, 7) if args.year else (12, 3)
    local = root / "copies-v3" / uuid.uuid4().hex
    env = dict(cut["setting"](live), PATH=os.defpath, AWS_EC2_METADATA_DISABLED="true",
               TOKIO_WORKER_THREADS="2", KONAE_COPY_SECRET="synthetic-recovery-campaign-ledger-key",
               KONAE_COPY_V3_SECRET=FIRST_KEY)
    settings = {"KONAE_COPY_V3_DIR": str(local)}
    sources = {"directory-v3": local}
    if args.s3:
        env.update(remote_settings(args.mc))
        remote = uuid.uuid4().hex
        settings["KONAE_COPY_V3_S3"] = remote
        sources["s3-v3"] = "s3://konae-test/copies-v3/" + remote
    operator = binary.with_name("kernel")
    model = Model()
    if binary.name == "konae":
        report["operation"] = "open app folder"
        with server(operator, env) as (port, _):
            equal(request(port, "/_konae/health")[0]["ok"], True, "fixture Kernel startup")
    counts = report["operations"]
    with server(binary, env) as (port, _):
        if binary.name == "konae":
            report["operation"] = "owner signup"
            owner, can = owner_session(port, env, signup=True)
            equal(can["write"], True, "owner project write access")
            model.projects.add("probe")
            model.published.add("probe")
            check_projects(port, owner, model)
            report["operation"] = "create draft"
            cut["rpc"](port, "project_create", {"projectId": "draft", "name": "Draft"}, owner)
            model.projects.add("draft")
            check_projects(port, owner, model)
            counts["projectsCreated"] += 1
        for email in MEMBERS:
            report["operation"] = "app signup"
            model.users[email], _ = account(port, email, "signup")
            counts["signups"] += 1
    snapshots = {}
    previous = None
    suspended = set()

    def restore(point):
        for kind, source in sources.items():
            report.update(phase="restore", operation="restore", destination=kind, cutDay=snapshots[point][0])
            save(report, args.report)
            with tempfile.TemporaryDirectory(dir=root) as clean:
                destination = Path(clean) / "restored"
                cli(binary, dict(env, KONAE_DATA_DIR=str(destination)), "restore", "--from", source, "--copy", point)
                verify(binary, env, destination, snapshots[point][1], suspended, counts)
            report["restores"][kind] += 1

    dates = sorted(set(range(1, days + 1, interval)) | {days})
    for index, end in enumerate(dates):
        begin = dates[index - 1] + 1 if index else 1
        report.update(day=begin, phase="mutate", destination=None, cutDay=None)
        save(report, args.report)
        if index in {0, len(dates) // 3, 2 * len(dates) // 3}:
            report["operation"] = "import"
            import_version(operator, env, live, model, rng, counts["imports"])
            counts["imports"] += 1
            if previous:
                restore(previous)
        if index == len(dates) // 2:
            env.update(KONAE_COPY_V3_SECRET=NEXT_KEY, KONAE_COPY_V3_PREVIOUS_SECRETS=json.dumps([FIRST_KEY]))
            counts["rotations"] += 1
            report["operation"] = "suspend"
            cli(operator, env | settings, "suspend", model.users[MEMBERS[2]], "--app", "probe")
            suspended.add(MEMBERS[2])
            counts["revocations"] += 1
            restore(previous)
        with server(binary, env) as (port, _):
            if index == len(dates) - 2:
                report.update(phase="mutate", operation="signup", destination=None, cutDay=None)
                late = "late@example.test"
                model.users[late], _ = account(port, late, "signup")
                counts["signups"] += 1
                if binary.name == "konae":
                    owner, can = owner_session(port, env)
                    equal(can["write"], True, "owner delete access")
                    cut["rpc"](port, "project_delete", {"projectId": "draft", "name": "Draft"}, owner)
                    model.projects.remove("draft")
                    check_projects(port, owner, model)
                    counts["projectsDeleted"] += 1
            cookies = {e: account(port, e, "login")[1] for e in MEMBERS[:2]}
            for day in range(begin, end + 1):
                report.update(day=day, phase="mutate", destination=None)
                email = rng.choice(MEMBERS[:2])
                title = f"{day:03}-{rng.getrandbits(32):08x}"
                report["operation"] = "insert"
                action(port, cookies[email], "notes.add", {"title": title})
                model.rows[title] = email
                check_rows(port, cookies[email], email, model)
                counts["inserts"] += 1
                if day % 3 == 0:
                    title = rng.choice(sorted(model.rows))
                    report["operation"] = "delete"
                    email = model.rows[title]
                    action(port, cookies[email], "notes.drop", {"title": title})
                    del model.rows[title]
                    check_rows(port, cookies[email], email, model)
                    counts["deletes"] += 1
        report.update(phase="capture", operation="publish", cutDay=end)
        previous = capture(binary, env | settings, previous, set(sources))
        snapshots[previous] = (end, copy.deepcopy(model))
        for kind in sources:
            report["points"][kind] += 1
        restore(previous)
    shutil.rmtree(live)
    report.update(phase="retention")
    # Each CLI cut has a real UTC timestamp. This campaign finishes within 30 days.
    daily = {p[:10]: p for p in sorted(snapshots)}
    kept = set(sorted(snapshots)[-24:]) | set(daily.values())
    equal({p.name for p in (local / "points").iterdir()}, kept, "directory retention")
    if args.s3:
        subprocess.run([args.mc, "rm", "--recursive", "--force", "lab/konae-test/files/probe/"],
                       check=True, capture_output=True, timeout=30)
        equal(subprocess.check_output([args.mc, "ls", "--recursive", "--json", "lab/konae-test/files/"],
                                      timeout=30).strip(), b"", "live MinIO files after volume loss")
        records = subprocess.check_output([args.mc, "ls", "--json", f"lab/konae-test/copies-v3/{remote}/points/"],
                                          timeout=30)
        equal({json.loads(line)["key"] for line in records.splitlines()}, kept, "MinIO retention")
    report["prunedPerDestination"] = len(snapshots) - len(kept)
    for point in sorted(kept):
        restore(point)
    report.update(phase="complete", destination=None, cutDay=None, operation=None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--seed", type=int, default=44)
    parser.add_argument("--year", action="store_true")
    parser.add_argument("--s3", action="store_true", help="require a loopback MinIO in addition to the directory")
    parser.add_argument("--mc", default="mc")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    if not args.s3:
        print("SKIP s3-v3 destination (--s3 requires a disposable MinIO bucket)", flush=True)
    report = dict(binary=args.binary.name, importAndSuspendBinary="kernel", seed=args.seed, days=365 if args.year else 12,
                  skipped=[] if args.s3 else ["s3-v3"], phase="setup", operations=Counter(),
                  restores=Counter(), points=Counter())
    started = time.monotonic()
    try:
        with tempfile.TemporaryDirectory(prefix="konae-year-") as tmp:
            campaign(args, Path(tmp), report)
    except Exception as error:
        if isinstance(error, HTTPError):
            report["httpStatus"] = error.code
            report["requestPath"] = urlsplit(error.url).path
        report["firstFailure"] = str(error) if isinstance(error, Divergence) else type(error).__name__
        raise
    finally:
        report["seconds"] = round(time.monotonic() - started, 3)
        summary = json.dumps(report, sort_keys=True)
        print(summary, flush=True)
        save(report, args.report)


if __name__ == "__main__":
    main()
