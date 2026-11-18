#!/usr/bin/env python3
"""Run the recovery exercise against two disposable MinIO servers on macOS."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import runpy
import secrets
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
from urllib import request

OPS = Path(__file__).resolve().parents[1]
DRILL = OPS / "recovery-drill.py"
BUCKET = "drill-store"
BASE_ENV = {"PATH": os.environ["PATH"], "AWS_EC2_METADATA_DISABLED": "true", "AWS_PAGER": "", "TOKIO_WORKER_THREADS": "2"}


def check(command, env, data=None):
    result = subprocess.run(command, input=data, capture_output=True, env=env, timeout=120)
    # Some arguments and child errors contain the disposable credentials.
    if result.returncode:
        raise RuntimeError(f"{Path(command[0]).name} failed ({result.returncode})")
    return result.stdout


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@contextlib.contextmanager
def process(command, env, ready):
    child = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
    try:
        for _ in range(300):
            if child.poll() is not None:
                raise RuntimeError(f"{Path(command[0]).name} exited before readiness")
            try:
                with request.urlopen(ready, timeout=.5) as response:
                    if response.status == 200:
                        break
            except OSError:
                pass
            time.sleep(.1)
        else:
            raise RuntimeError("readiness timed out")
        yield child
    finally:
        child.terminate()
        child.wait(timeout=15)


class Store:
    def __init__(self, tools, root):
        self.tools = tools
        self.root = root
        self.access, self.secret = secrets.token_hex(12), secrets.token_hex(24)
        self.address = f"127.0.0.1:{free_port()}"
        self.endpoint = "http://" + self.address
        self.env = dict(BASE_ENV, MINIO_ROOT_USER=self.access, MINIO_ROOT_PASSWORD=self.secret,
                        MINIO_BROWSER="off", MINIO_UPDATE="off", MC_CONFIG_DIR=str(root / "mc"),
                        MC_HOST_lab=f"http://{self.access}:{self.secret}@{self.address}")
        self.reader = ("reader", secrets.token_hex(24))
        self.writer = ("writer", secrets.token_hex(24))

    def mc(self, *args, data=None):
        return check([self.tools / "mc", *args], self.env, data)

    @contextlib.contextmanager
    def running(self):
        self.root.mkdir()
        with process([self.tools / "minio", "server", "--address", self.address, self.root / "objects"],
                     self.env, self.endpoint + "/minio/health/live"):
            self.mc("mb", "lab/" + BUCKET)
            self.mc("version", "enable", "lab/" + BUCKET)
            self.account(self.reader, ["s3:GetObject"])
            self.account(self.writer, ["s3:PutObject"])
            yield self

    def account(self, credential, actions):
        resource = f"arn:aws:s3:::{BUCKET}"
        policy = {"Version": "2012-10-17", "Statement": [
            {"Effect": "Allow", "Action": actions, "Resource": [resource + "/copies/*"]},
            {"Effect": "Allow", "Action": ["s3:ListBucket"], "Resource": [resource],
             "Condition": {"StringLike": {"s3:prefix": ["copies/*"]}}},
        ]}
        filename = self.root / (credential[0] + ".json")
        filename.write_text(json.dumps(policy))
        self.mc("admin", "user", "add", "lab", *credential)
        self.mc("admin", "policy", "create", "lab", credential[0], filename)
        self.mc("admin", "policy", "attach", "lab", credential[0], "--user", credential[0])

    def env_for(self, credential):
        return dict(KONAE_S3_ACCESS_KEY=credential[0], KONAE_S3_SECRET_KEY=credential[1],
                    KONAE_S3_ENDPOINT=self.endpoint, KONAE_S3_BUCKET=BUCKET)


def permissions(store):
    credentials = store.root / "profiles"
    credentials.write_text("".join(f"[{user}]\naws_access_key_id={user}\naws_secret_access_key={secret}\n"
                                   for user, secret in [store.reader, store.writer, (store.access, store.secret)]))
    env = dict(BASE_ENV, AWS_SHARED_CREDENTIALS_FILE=str(credentials), AWS_CONFIG_FILE=os.devnull)
    cmd = [sys.executable, OPS / "recovery-permissions.py", "--endpoint", store.endpoint,
           "--bucket", BUCKET, "--reader", "reader", "--writer"]
    success = json.loads(check([*cmd, "writer"], env))
    assert success["deniedDeleteRequests"] == 6
    print(json.dumps(success), flush=True)
    unsafe = subprocess.run([*cmd, store.access], env=env, capture_output=True, timeout=120)
    assert unsafe.returncode and json.loads(unsafe.stdout)["reason"] == "writer-deleted-canary"

    # Keep a positive object under files/ so a 404 cannot pass for a denied read.
    store.mc("pipe", "lab/" + BUCKET + "/files/live-canary", data=b"lab-only")
    for op, key in [("get-object", "files/live-canary"), ("put-object", "copies/drill-canaries/reader.canary")]:
        args = [os.devnull] if op == "get-object" else ["--body", str(credentials.parent / "reader.json")]
        result = subprocess.run(["aws", "--region", "us-east-1", "--endpoint-url", store.endpoint,
                                 "--profile", "reader", "s3api", op, "--bucket", BUCKET, "--key", key, *args],
                                capture_output=True, env=env, timeout=60)
        assert result.returncode and b"(AccessDenied)" in result.stderr, op


def check_due_jobs(live, app, binary):
    run = runpy.run_path(str(DRILL))["restricted"]
    env = {"KONAE_DATA_DIR": str(live), "KONAE_EXECUTION_MODE": "test", "TMPDIR": str(live)}
    assert run(binary, live, ["drill-check"], env).returncode == 0

    def sql(db, statement):
        with contextlib.closing(sqlite3.connect(db)) as connection:
            rows = connection.execute(statement).fetchall()
            connection.commit()
            return rows

    sql(live / "mail-test.sqlite", """INSERT INTO jobs(id,purpose,recipient,scope,app_name,token_hash,
        link_expires_at,state,next_attempt_at,created_at,updated_at) VALUES
        ('due','notify','job@drill.test','app','Drill','fixture',1,'pending',0,0,0)""")
    sql(live / "calls.sqlite", """INSERT INTO calls(id,deployment,action,input,state,next_attempt_at,
        created_at,updated_at) VALUES ('due','app_legacy_schedule_route','job','{}','pending',0,0,0)""")
    data = app / "data"
    data.mkdir(exist_ok=True)
    sql(data / "database.sqlite", (app / "app/schema.sql").read_text())
    sql(data / "usage.sqlite", "CREATE TABLE IF NOT EXISTS schedule_state(action TEXT PRIMARY KEY, schedule TEXT, slot INTEGER)")
    sql(data / "usage.sqlite", "INSERT OR REPLACE INTO schedule_state VALUES('notes.tick','daily 10:00',0)")
    assert run(binary, live, ["drill-check"], env).returncode == 0
    for db, table in [("calls.sqlite", "calls"), ("mail-test.sqlite", "jobs")]:
        assert sql(live / db, f"SELECT state,attempts FROM {table} WHERE id='due'") == [("pending", 0)]
    assert sql(data / "database.sqlite", "SELECT count(*) FROM notes") == [(0,)]
    assert sql(data / "usage.sqlite", "SELECT slot FROM schedule_state WHERE action='notes.tick'") == [(0,)]
    print(json.dumps({"dueMailAttempts": 0, "dueCallAttempts": 0, "dueScheduleWrites": 0}), flush=True)


def capture(tmp, store, binary, secret):
    live = tmp / "live"
    directory = tmp / "primary-copies"
    fixture = OPS.parent / "kernel-rs/test-fixtures/module-actions-schedule-route-0.9.0"
    app = live / "apps/app_legacy_schedule_route"
    shutil.copytree(fixture, app)
    (app / ".konae").mkdir()
    (app / ".konae/deployment.json").write_text(json.dumps({
        "hostname": "schedule.localhost", "createdAt": 1, "artifactVersion": "fixture",
        "actions": (app / "app/actions.json").read_text()}))
    (app / "dist").mkdir()
    (app / "dist/index.html").write_text("<main>Recovery test</main>")
    port = str(free_port())
    origin = "http://127.0.0.1:" + port
    env = dict(BASE_ENV, **store.env_for((store.access, store.secret)), KONAE_COPY_SECRET=secret,
               KONAE_COPY_S3="1", KONAE_COPY_INTERVAL_SECONDS="3600", KONAE_COPY_DIR=str(directory),
               KONAE_DATA_DIR=str(live), KONAE_SETUP_KEY=secrets.token_hex(32),
               KONAE_BIND="127.0.0.1", KONAE_KERNEL_PORT=port)
    first = {k: v for k, v in env.items() if not k.startswith("KONAE_COPY_")}
    with process([binary], first, origin + "/_konae/health"):
        account = {"email": "owner@drill.test", "password": secrets.token_hex(16), "setupKey": env["KONAE_SETUP_KEY"]}
        signup = request.Request(origin + "/_konae/owner/signup", json.dumps(account).encode(),
                                 headers={"Content-Type": "application/json"})
        with request.urlopen(signup, timeout=90) as response:
            assert response.status == 200
    signed_up = time.time() * 1000
    with process([binary], env, origin + "/_konae/health"):
        for _ in range(200):
            for manifest in sorted(directory.glob("*/manifest.json"), reverse=True):
                if json.loads(manifest.read_bytes())["createdAt"] <= signed_up:
                    continue
                point = manifest.parent.name
                stat = subprocess.run([store.tools / "mc", "stat", f"lab/{BUCKET}/copies/{point}/manifest.json"],
                                      env=store.env, capture_output=True)
                if stat.returncode == 0:
                    break
            else:
                time.sleep(.1)
                continue
            break
        else:
            raise RuntimeError("no complete point after signup")
    check([binary, "owner-account-suspend", account["email"]], env)
    record, = [p for p in (directory / "revocations").glob("*.json") if p.name != "started.json"]
    signed = store.mc("cat", f"lab/{BUCKET}/copies/revocations/{record.name}")
    check_due_jobs(live, app, binary)
    shutil.rmtree(live)
    return point, directory, record, signed


def check_sandbox(tmp):
    run = runpy.run_path(str(DRILL))["restricted"]
    scratch = tmp / "sandbox"
    scratch.mkdir()
    live = tmp / "live-sentinel"
    live.write_bytes(b"outside the drill")
    for path in (live, Path("/System/Volumes/Data") / str(live).lstrip("/")):
        result = run(Path("/bin/cat"), scratch, [str(path)], {})
        assert result.returncode and b"Operation not permitted" in result.stderr
    with socket.socket() as repo, socket.socket() as unrelated:
        for listener in (repo, unrelated):
            listener.bind(("127.0.0.1", 0))
            listener.listen()
        port = repo.getsockname()[1]
        for listener in (repo, unrelated):
            args = ["-z", "-v", "-w", "1", "127.0.0.1", str(listener.getsockname()[1])]
            result = run(Path("/usr/bin/nc"), scratch, args, {}, port=port)
            if listener is repo:
                assert result.returncode == 0
            else:
                assert result.returncode and b"Operation not permitted" in result.stderr
    print(json.dumps({"filesystemReadDenied": True, "repositoryPortOnly": True}), flush=True)


def lab(tmp, tools, binary):
    check_sandbox(tmp.resolve())
    store = Store(tools, tmp / "primary")
    replica = Store(tools, tmp / "independent")
    secret = secrets.token_hex(32)
    sha = runpy.run_path(str(DRILL))["digest"](binary)

    def restore(source, scenario, witness, expected, env=None, endpoint=None):
        cmd = [sys.executable, DRILL, "--scenario", scenario, "--material", "lab", "--binary", binary,
               "--binary-sha256", sha, "--source", source, "--copy", point, "--witness", witness]
        if endpoint:
            cmd += ["--endpoint", endpoint]
        result = subprocess.run(cmd, env=BASE_ENV | (env or {}), capture_output=True, timeout=180)
        report = json.loads(result.stdout)
        print(json.dumps(report), flush=True)
        assert report.get("volumeRemoved") and ((result.returncode == 0) == (expected == "passed"))
        assert report.get("reason", report["result"]) == expected
        if not result.returncode:
            assert report["deployments"] == 1 and report["postCutRevocationApplied"] and report["workerlessHealth"]
        return report

    with replica.running():
        permissions(replica)
        with store.running():
            point, directory, record, signed = capture(tmp, store, binary, secret)
            signed_hash = hashlib.sha256(signed).hexdigest()
            restore(directory, "primary", hashlib.sha256(record.read_bytes()).hexdigest(), "passed")
            locator = f"s3://{BUCKET}/copies/"
            creds = dict(store.env_for(store.reader), KONAE_COPY_SECRET=secret)
            restore(locator, "primary", signed_hash, "passed", creds, store.endpoint)
            dest = tmp / "sealed"
            store.mc("mirror", f"lab/{BUCKET}/copies/", dest)
            # Only the append credential crosses into the independent repository.
            writer, password = replica.writer
            writer_env = replica.env | {"MC_HOST_lab": f"http://{writer}:{password}@{replica.address}"}
            for obj in sorted(dest.rglob("*")):
                if obj.is_file():
                    key = f"lab/{BUCKET}/copies/{obj.relative_to(dest)}"
                    check([tools / "mc", "pipe", key], writer_env, obj.read_bytes())
            shutil.rmtree(dest)
        shutil.rmtree(directory)
        shutil.rmtree(store.root)
        creds = dict(replica.env_for(replica.reader), KONAE_COPY_SECRET=secret)
        passed = restore(locator, "independent", signed_hash, "passed", creds, replica.endpoint)
        assert passed["missingKeyRefused"]
        restore(locator, "independent", signed_hash, "custody-key-unavailable",
                replica.env_for(replica.reader), replica.endpoint)
        restore(locator, "independent", signed_hash, "wrong-custody-key",
                creds | {"KONAE_COPY_SECRET": secrets.token_hex(32)}, replica.endpoint)
        restore(locator, "independent", "0" * 64, "missing-witness", creds, replica.endpoint)
        key = f"lab/{BUCKET}/copies/revocations/{record.name}"
        entry = json.loads(signed)
        entry.pop("mac")
        replica.mc("pipe", key, data=json.dumps(entry).encode())
        restore(locator, "independent", signed_hash, "unsigned-input", creds, replica.endpoint)
        replica.mc("rm", f"lab/{BUCKET}/copies/revocations/started.json")
        restore(locator, "independent", signed_hash, "record-unavailable", creds, replica.endpoint)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--minio-bin-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=OPS.parent / "kernel-rs/target/debug/konae")
    args = parser.parse_args()
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="konae-drill-lab-") as directory:
        lab(Path(directory).resolve(), args.minio_bin_dir.resolve(), args.binary.resolve(strict=True))
