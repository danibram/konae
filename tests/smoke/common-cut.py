"""Run from a checkout with both debug binaries built. All state lives in a temporary directory."""
import argparse
from contextlib import contextmanager, closing
from concurrent.futures import ThreadPoolExecutor
import errno
import fcntl
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.request
import sys


def wait(check, reason, seconds=30):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = check()
        if result:
            return result
        time.sleep(.025)
    raise AssertionError(reason)


def request(port, path, body=None, headers=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=data,
                                 headers={"Content-Type": "application/json", **(headers or {})})
    with urllib.request.urlopen(req, timeout=30) as response:
        return json.load(response), response.headers


@contextmanager
def server(binary, env):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen([binary], env=dict(env, KONAE_KERNEL_PORT=str(port)),
                                stdout=log, stderr=log)
        def ready():
            if proc.poll() is not None:
                log.seek(0)
                raise AssertionError(log.read().decode())
            try:
                return request(port, "/_konae/health")[0]["ok"]
            except OSError:
                return False
        try:
            wait(ready, "server did not start")
            yield port, proc
        finally:
            if proc.poll() is None:
                proc.terminate()
            proc.wait(timeout=10)


def rpc(port, name, args, owner=None):
    path, headers = ("/mcp", {"Cookie": owner}) if owner else ("/_konae/mcp", {"Host": "probe.localhost"})
    result = request(port, path, {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                  "params": {"name": name, "arguments": args}}, headers)[0]["result"]
    assert not result["isError"], result["content"]
    return result["structuredContent"]["result"]


def fixture(live):
    app = live / "apps/probe"
    for directory in ("app", "dist"):
        (app / directory).mkdir(parents=True)
    (app / "konae.json").write_text(json.dumps(dict(id="probe", name="Probe", slug="probe", visibility="public",
        signupPolicy={"mode": "open", "allowlist": []}, signInMethods=["password"], kernel="1", manifest={"version": 1},
        modules={"actions": {"on": True}})))
    (app / "app/schema.sql").write_text("CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);")
    actions = {"notes.add": {"kind": "mutation", "access": "anonymous", "bindings": [],
                             "statement": "INSERT INTO notes(id) VALUES(NULL)", "signal": {"name": "note.added"}},
               "notes.count": {"kind": "query", "access": "anonymous", "bindings": [],
                               "statement": "SELECT COUNT(*) AS n FROM notes LIMIT 1"}}
    (app / "app/actions.json").write_text(json.dumps({"version": 1, "actions": actions}))
    (app / "dist/index.html").write_text("<main>Probe</main>")
    (app / "app/sample.bin").write_bytes(bytes(range(256)) * 2048)


def setting(root):
    return dict(KONAE_DATA_DIR=str(root), KONAE_BIND="127.0.0.1", KONAE_HOST_SUFFIX="localhost",
                KONAE_PUBLIC_ORIGIN="http://localhost", KONAE_SETUP_KEY="synthetic-setup-for-common-cut",
                KONAE_MAIL_TRANSPORT="development", KONAE_COPY_INTERVAL_SECONDS="3600")


def targets(port):
    return request(port, "/_konae/health")[0]["copies"]["targets"]


def complete(port):
    reports = targets(port)
    for report in reports:
        assert not report["lastError"], report["lastError"]
    return reports if all(r["lastCopyId"] and not r["inProgress"] for r in reports) else None


def number(database, sql):
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as conn:
        return conn.execute(sql).fetchone()[0]


def app_count(port):
    return rpc(port, "actions_invoke", {"name": "notes.count", "input": {}})[0]["n"]


def probe(binary, root, marker):
    live = root / "live"
    fixture(live)
    env = {k: v for k, v in os.environ.items() if not k.startswith(("KONAE_", "AWS_"))}
    env.update(setting(live))
    if marker:
        for key in ("ENDPOINT", "BUCKET", "ACCESS_KEY", "SECRET_KEY"):
            env["KONAE_S3_" + key] = os.environ["KONAE_TEST_S3_" + key]
        assert env["KONAE_S3_ENDPOINT"].startswith("http://127.0.0.1:")
    kernel = binary.with_name("kernel")
    with server(kernel, env) as (port, _proc):
        assert app_count(port) == 0
        rpc(port, "actions_invoke", {"name": "notes.add", "input": {}})
        assert app_count(port) == 1
    copies = root / "directory"
    env["KONAE_COPY_DIR"] = str(copies)
    if not marker:
        env["KONAE_COPY_INTERVAL_SECONDS"] = "2"
    if marker:
        marker.unlink(missing_ok=True)
        env["KONAE_COPY_S3"] = "1"
    with server(binary, env) as (port, _proc):
        if marker:
            wait(marker.exists, "slow proxy did not observe 64 KiB")
            assert any(r["kind"] == "s3" and r["inProgress"] for r in targets(port))
            start = time.monotonic()
            for i in range(5):
                rpc(port, "actions_invoke", {"name": "notes.add", "input": {}})
                assert app_count(port) == i + 2
            latency = time.monotonic() - start
            assert any(r["kind"] == "s3" and r["inProgress"] for r in targets(port)), "upload finished before writes"
        reports = wait(lambda: complete(port), "copy did not finish", 90)
        assert len(reports) == (2 if marker else 1)
        copy = reports[0]["lastCopyId"]
        assert all(r["lastCopyId"] == copy for r in reports)
        if not marker:
            data = live / "apps/probe/data"
            status = next((live / ".copy-health").glob("*.json"))
            with ThreadPoolExecutor(1) as executor, closing(sqlite3.connect(data / "usage.sqlite")) as blocker:
                blocker.execute("BEGIN IMMEDIATE")
                write = executor.submit(rpc, port, "actions_invoke", {"name": "notes.add", "input": {}})
                wait(lambda: number(data / "database.sqlite", "SELECT count(*) FROM notes") == 2,
                     "action did not commit its row")
                wait(lambda: json.loads(status.read_text())["running"], "copy did not start during the mutation")
                assert not write.done()
                assert number(data / "usage.sqlite", "SELECT count(*) FROM signals WHERE name='note.added'") == 1
                blocker.rollback()
                write.result(timeout=5)
                assert app_count(port) == 2
            next_cut = wait(lambda: (r if (r := targets(port)[0])["lastCopyId"] != copy else None),
                            "copy did not drain the action")
            staged = copies / next_cut["lastCopyId"] / "apps/probe/data"
            assert number(staged / "database.sqlite", "SELECT count(*) FROM notes") == 2
            assert number(staged / "usage.sqlite", "SELECT count(*) FROM signals WHERE name='note.added'") == 2
            print(f"{binary.name} drained an HTTP mutation held between its row and Signal commits", flush=True)
        if marker:
            print(f"{binary.name} completed five HTTP writes and five read-backs in {latency:.3f}s during the S3 upload", flush=True)
    sources = [str(copies)]
    if marker:
        sources.append("s3://" + env["KONAE_S3_BUCKET"] + "/copies")
    for i, source in enumerate(sources):
        restored = root / f"restore-{i}"
        subprocess.run([binary, "restore", "--from", source, "--copy", copy],
                       env=dict(env, KONAE_DATA_DIR=str(restored)), check=True, stdout=subprocess.DEVNULL)
        assert number(restored / "apps/probe/data/database.sqlite", "SELECT count(*) FROM notes") == 1
        assert number(restored / "apps/probe/data/usage.sqlite", "SELECT count(*) FROM signals WHERE name='note.added'") == 1
        assert (restored / "apps/probe/app/sample.bin").read_bytes() == (live / "apps/probe/app/sample.bin").read_bytes()
    print(f"{binary.name} restored {len(sources)} destination(s) at the selected cut", flush=True)


def external_import(binary, root):
    live = root / "live"
    fixture(live)
    env = {k: v for k, v in os.environ.items() if not k.startswith(("KONAE_", "AWS_"))}
    env.update(setting(live), KONAE_COPY_DIR=str(root / "copies"), KONAE_COPY_INTERVAL_SECONDS="2")
    imported = root / "export/data"
    imported.mkdir(parents=True)
    with closing(sqlite3.connect(imported / "database.sqlite")) as db:
        db.executescript("CREATE TABLE notes(id INTEGER PRIMARY KEY); INSERT INTO notes VALUES(7);")
    with server(binary, env) as (port, _proc):
        wait(lambda: complete(port), "no pre-import point")
        status = next((live / ".copy-health").glob("directory-*.json"))
        volume = os.open(live, os.O_RDONLY)
        apps = os.open(live / "apps", os.O_RDONLY)
        def command_admitted():
            try:
                fcntl.flock(volume, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
            fcntl.flock(volume, fcntl.LOCK_UN)
            return False
        try:
            fcntl.flock(apps, fcntl.LOCK_EX)
            with subprocess.Popen([binary, "import", imported.parent, "--app", "probe"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE) as proc:
                try:
                    wait(command_admitted, "the importer did not take the data-directory permit")
                    wait(lambda: json.loads(status.read_text())["running"], "no capture during the blocked import")
                    assert proc.poll() is None
                    assert number(live / "apps/probe/data/database.sqlite", "SELECT count(*) FROM notes") == 0
                    fcntl.flock(apps, fcntl.LOCK_UN)
                    out, err = proc.communicate(timeout=30)
                    assert proc.returncode == 0, err.decode()
                    assert json.loads(out)["imported"]
                finally:
                    if proc.poll() is None:
                        proc.kill()
        finally:
            os.close(apps)
            os.close(volume)
        assert app_count(port) == 1
        report = wait(lambda: complete(port), "no complete post-import point")[0]
        staged = root / "copies" / report["lastCopyId"] / "apps/probe/data/database.sqlite"
        assert number(staged, "SELECT id FROM notes") == 7
    print("kernel import held the volume across SQLite replacement and the next copy contained the new generation", flush=True)


def low_disk(binary, root):
    image = root / "space.dmg"
    mount = root / "volume"
    mount.mkdir()
    subprocess.run(["hdiutil", "create", "-size", "96m", "-fs", "HFS+", "-volname", "CommonCut", image],
                   check=True, stdout=subprocess.DEVNULL)
    subprocess.run(["hdiutil", "attach", "-nobrowse", "-mountpoint", mount, image],
                   check=True, stdout=subprocess.DEVNULL)
    try:
        live = mount / "live"
        fixture(live)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("KONAE_", "AWS_"))}
        env.update(setting(live))
        with server(binary, env) as (port, _proc):
            assert app_count(port) == 0
        # Keep enough space for health and new action rows once the copy fails.
        with (live / "apps/probe/app/large").open("wb") as source:
            source.write(b"x" * (8 * 1024 * 1024))
        filler = mount / "filler"
        with filler.open("wb", buffering=0) as space:
            try:
                while True:
                    space.write(b"x" * (512 * 1024))
            except OSError as error:
                assert error.errno == errno.ENOSPC
            space.truncate(space.tell() - 2 * 1024 * 1024)
        env.update(KONAE_COPY_DIR=str(root / "copies"), KONAE_COPY_INTERVAL_SECONDS="2")
        with server(binary, env) as (port, _proc):
            wait(lambda: "staging space" in (targets(port)[0]["lastError"] or ""), "disk limit went unreported")
            assert targets(port)[0]["lastCopyId"] is None
            rpc(port, "actions_invoke", {"name": "notes.add", "input": {}})
            assert app_count(port) == 1
            filler.unlink()
            report = wait(lambda: (r if (r := targets(port)[0])["lastCopyId"] else None), "next point did not recover")
            assert not report["lastError"]
        print("kernel resumed writes on the full-volume trial and made a new point after space was freed", flush=True)
    finally:
        subprocess.run(["hdiutil", "detach", mount], check=True, stdout=subprocess.DEVNULL)


def read_restart(binary, root):
    live = root / "live"
    fixture(live)
    env = {k: v for k, v in os.environ.items() if not k.startswith(("KONAE_", "AWS_"))}
    env.update(setting(live))
    with server(binary.with_name("kernel"), env) as (port, _proc):
        assert app_count(port) == 0
    env["KONAE_COPY_DIR"] = str(root / "copies")
    with server(binary, env) as (port, proc):
        first = wait(lambda: complete(port), "no complete copy")[0]["lastCopyId"]
        pipe = live / "apps/probe/dist/read.txt"
        os.mkfifo(pipe)
        def page():
            req = urllib.request.Request(f"http://127.0.0.1:{port}/read.txt", headers={"Host": "probe.localhost"})
            try:
                with urllib.request.urlopen(req, timeout=20) as resp:
                    resp.read()
            except OSError:
                pass
        def reader_waiting():
            try:
                return os.open(pipe, os.O_WRONLY | os.O_NONBLOCK)
            except OSError as err:
                if err.errno == errno.ENXIO:
                    return None
                raise
        with ThreadPoolExecutor(1) as executor:
            read = executor.submit(page)
            fd = wait(reader_waiting, "the GET did not open the test page")
            try:
                assert not read.done()
                proc.kill()
                proc.wait()
            finally:
                os.close(fd)
            read.result()
        pipe.unlink()
    with server(binary, env) as (port, _proc):
        second = wait(lambda: (r if (r := targets(port)[0])["lastCopyId"] not in (None, first) else None),
                      "an interrupted GET prevented the next copy")
        assert not second["lastError"]
    print(f"{binary.name} completed a new copy after SIGKILL during a GET reading the page", flush=True)


def mutation_restart(binary, root):
    live = root / "live"
    fixture(live)
    env = {k: v for k, v in os.environ.items() if not k.startswith(("KONAE_", "AWS_"))}
    env.update(setting(live))
    with server(binary.with_name("kernel"), env) as (port, _proc):
        rpc(port, "actions_invoke", {"name": "notes.add", "input": {}})
        assert app_count(port) == 1
    env["KONAE_COPY_DIR"] = str(root / "copies")
    with server(binary, env) as (port, proc):
        copy = wait(lambda: complete(port), "no complete point before the mutation")[0]["lastCopyId"]
        data = live / "apps/probe/data"
        with ThreadPoolExecutor(1) as executor, closing(sqlite3.connect(data / "usage.sqlite")) as blocker:
            blocker.execute("BEGIN IMMEDIATE")
            write = executor.submit(rpc, port, "actions_invoke", {"name": "notes.add", "input": {}})
            wait(lambda: number(data / "database.sqlite", "SELECT count(*) FROM notes") == 2,
                 "the action did not commit the row")
            assert number(data / "usage.sqlite", "SELECT count(*) FROM signals") == 1
            assert not write.done()
            proc.kill()
            proc.wait(timeout=5)
            blocker.rollback()
            try:
                write.result(timeout=5)
            except OSError:
                pass
    with server(binary, env) as (port, _proc):
        report = wait(lambda: (r if "probe/notes.add" in ((r := targets(port)[0])["lastError"] or "") else None),
                      "restart did not identify the interrupted action")
        assert "row and its declared effects" in report["lastError"]
        assert report["lastCopyId"] == copy
    restored = root / "recovered"
    subprocess.run([binary, "restore", "--from", env["KONAE_COPY_DIR"], "--copy", copy],
                   env=dict(env, KONAE_DATA_DIR=str(restored)), check=True, stdout=subprocess.DEVNULL)
    data = restored / "apps/probe/data"
    assert number(data / "database.sqlite", "SELECT count(*) FROM notes") == 1
    assert number(data / "usage.sqlite", "SELECT count(*) FROM signals") == 1
    print(f"{binary.name} identified the interrupted row/Signal operation and restored its previous point", flush=True)


def suspension(binary, root):
    live = root / "live"
    env = {k: v for k, v in os.environ.items() if not k.startswith(("KONAE_", "AWS_"))}
    env.update(setting(live), KONAE_COPY_DIR=str(root / "copies"), KONAE_COPY_INTERVAL_SECONDS="1")
    email = "owner@common-cut.test"
    with server(binary, env) as (port, _proc):
        _, headers = request(port, "/_konae/owner/signup", {"email": email, "password": "synthetic-password-42",
                                                             "setupKey": env["KONAE_SETUP_KEY"]})
        cookie = headers["set-cookie"].split(";")[0]
        rpc(port, "project_create", {"projectId": "probe", "name": "Probe"}, cookie)
        assert rpc(port, "project_inspect", {"projectId": "probe"}, cookie)
        rpc(port, "project_create", {"projectId": "discard", "name": "Discard"}, cookie)
        assert rpc(port, "project_inspect", {"projectId": "discard"}, cookie)
        try:
            rpc(port, "project_delete", {"projectId": "discard", "name": "Wrong"}, cookie)
        except AssertionError as error:
            assert "name does not match" in str(error)
        else:
            raise AssertionError("deletion with the wrong name succeeded")
        assert not list((live / ".copy-operations").glob("*.json"))
        rpc(port, "project_delete", {"projectId": "discard", "name": "Discard"}, cookie)
        assert "discard" not in json.dumps(rpc(port, "projects_list", {}, cookie))
        rpc(port, "token_create", {"scopes": ["read"], "projectIds": ["probe"]}, cookie)
        assert len(rpc(port, "tokens_list", {}, cookie)["credentials"]) == 1
        wait(lambda: complete(port), "no complete copy")
        with closing(sqlite3.connect(live / "mcp.sqlite")) as db:
            db.execute("BEGIN IMMEDIATE")
            proc = subprocess.Popen([binary, "owner-account-suspend", email], env=env,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                wait(lambda: number(live / "owners.sqlite", "SELECT state FROM app_users LIMIT 1") == "suspended",
                     "suspension did not commit to the owner store")
                assert number(live / "mcp.sqlite", "SELECT count(*) FROM credentials") == 1
                assert proc.poll() is None
                proc.kill()
                proc.wait(timeout=5)
                assert number(live / "owners.sqlite", "SELECT count(*) FROM pending_owner_suspensions") == 1
                wait(lambda: "Unfinished owner suspension" in (targets(port)[0]["lastError"] or ""),
                     "the server did not reject the half-committed suspension")
            finally:
                if proc.poll() is None:
                    proc.kill()
                proc.wait()
                db.rollback()
    with server(binary, env) as (port, _proc):
        report = wait(lambda: (r if (r := targets(port)[0])["lastError"] is None and r["lastCopyId"] else None),
                      "restart did not finish the suspension")
        snapshot = root / "copies" / report["lastCopyId"]
        assert number(snapshot / "owners.sqlite", "SELECT count(*) FROM pending_owner_suspensions") == 0
        assert number(snapshot / "owners.sqlite", "SELECT state FROM app_users LIMIT 1") == "suspended"
        assert number(snapshot / "mcp.sqlite", "SELECT count(*) FROM credentials") == 0
        assert number(snapshot / "mcp.sqlite", "SELECT count(*) FROM projects WHERE id='probe'") == 1
        assert (snapshot / "apps/probe/konae.json").is_file()
        assert not (snapshot / "apps/discard").exists()
        assert number(snapshot / "mcp.sqlite", "SELECT count(*) FROM projects WHERE id='discard'") == 0
    print("konae refused a killed suspension between commits and completed it on restart", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=Path("kernel-rs/target/debug"))
    parser.add_argument("--slow-marker", type=Path)
    parser.add_argument("--binary", choices=("kernel", "konae"))
    parser.add_argument("--minio-lab", type=Path)
    parser.add_argument("--disk-full", action="store_true", help="test a disposable 96 MiB macOS disk image")
    args = parser.parse_args()
    if args.disk_full:
        print("Directory on a disposable disk image. SKIP S3 (run --minio-lab separately).", flush=True)
        with tempfile.TemporaryDirectory(prefix="cut-space-") as root:
            low_disk((args.bin_dir / "kernel").resolve(), Path(root))
        sys.exit(0)
    if args.minio_lab:
        for name in ("kernel", "konae"):
            with tempfile.TemporaryDirectory() as tmp:
                marker = Path(tmp) / "upload"
                subprocess.run([args.minio_lab / "with-minio", args.minio_lab / "slow-s3", "--kib-per-second", "128",
                                "--marker", marker, sys.executable, __file__, "--bin-dir", args.bin_dir,
                                "--binary", name, "--slow-marker", marker], check=True)
        sys.exit(0)
    if not args.slow_marker:
        selected = args.binary or "both binaries"
        print(f"SKIP slow MinIO upload and S3 restore (no --slow-marker). Directory runs on {selected}.", flush=True)
    for name in ([args.binary] if args.binary else ["kernel", "konae"]):
        with tempfile.TemporaryDirectory(prefix="common-cut-") as root:
            probe((args.bin_dir / name).resolve(), Path(root), args.slow_marker)
        if not args.slow_marker:
            with tempfile.TemporaryDirectory(prefix="cut-read-") as root:
                read_restart((args.bin_dir / name).resolve(), Path(root))
            with tempfile.TemporaryDirectory(prefix="cut-action-") as root:
                mutation_restart((args.bin_dir / name).resolve(), Path(root))
    if args.binary != "konae" and not args.slow_marker:
        with tempfile.TemporaryDirectory(prefix="cut-import-") as root:
            external_import((args.bin_dir / "kernel").resolve(), Path(root))
    if args.binary != "kernel" and not args.slow_marker:
        with tempfile.TemporaryDirectory(prefix="cut-suspension-") as root:
            suspension((args.bin_dir / "konae").resolve(), Path(root))
