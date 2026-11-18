"""Exercise repository exclusion, retention and cold restore on synthetic v3 points."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import parse_qs, urlsplit

from readonly_replica import mirror

REPO = "0123456789abcdef0123456789abcdef"
PREFIX = f"lab/konae-test/copies-v3/{REPO}/"
WORKER = "blocks::tests::retention::worker"


@contextmanager
def list_fault(endpoint, mode):
    """Change one response to the real MinIO client's list request."""
    origin = urlsplit(endpoint)
    seen = []
    release = threading.Event()

    class Proxy(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def forward(self):
            query = parse_qs(urlsplit(self.path).query)
            prefix = query.get("prefix", [""])[0]
            if prefix.endswith("points/") and (
                mode == "no-token" or "continuation-token" in query
            ):
                seen.append(mode)
                if mode == "no-token":
                    self.send_response(200)
                    self.end_headers()
                    self.wfile.write(b"<ListBucketResult><KeyCount>0</KeyCount>"
                                     b"<IsTruncated>true</IsTruncated></ListBucketResult>")
                else:
                    self.send_error(403)
                return
            if mode == "timeout" and self.command == "PUT" and "/points/" in self.path:
                seen.append(mode)
                release.wait(30)
                return
            if mode == "full" and self.command == "PUT" and "/points/" in self.path:
                seen.append(mode)
                self.send_error(507)
                return
            conn = http.client.HTTPConnection(origin.hostname, origin.port, timeout=40)
            try:
                data = self.rfile.read(int(self.headers.get("Content-Length", 0)))
                conn.request(self.command, self.path, body=data, headers=dict(self.headers))
                reply = conn.getresponse()
                self.send_response(reply.status)
                for name, value in reply.getheaders():
                    if name.lower() not in ("connection", "transfer-encoding"):
                        self.send_header(name, value)
                self.end_headers()
                self.wfile.write(reply.read())
            finally:
                conn.close()

        do_GET = do_HEAD = do_PUT = do_DELETE = forward

    with ThreadingHTTPServer(("127.0.0.1", 0), Proxy) as server:
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}"
            assert seen, f"Fault {mode} did not fire"
        finally:
            release.set()
            server.shutdown()
            thread.join()


class Lab:
    def __init__(self, args, root, target):
        self.args, self.root, self.target = args, root, target
        self.env = dict(os.environ, KONAE_LAB_ROOT=str(root), KONAE_LAB_TARGET=target,
                        TOKIO_WORKER_THREADS="2", AWS_EC2_METADATA_DISABLED="true")
        self.remote = target == "s3"
        self.summary = {"phase": "setup", "checks": {}, "expectedRefusals": 0}
        self.start = datetime.now(timezone.utc).replace(hour=0, minute=0, second=0, microsecond=0)
        self.env["KONAE_LAB_DAYS"] = "365" if args.year else "45"
        self.env["KONAE_LAB_START"] = str(int(self.start.timestamp()) * 1000)
        self.command = [args.worker, "--exact", WORKER, "--ignored", "--nocapture"]
        self.data = root / "copies-v3" / REPO

    def run(self, phase, error=None, measure=False, **settings):
        self.summary["phase"] = phase
        env = dict(self.env, KONAE_LAB_PHASE=phase, **settings)
        if env["KONAE_LAB_TARGET"] == "directory":
            env = {k:v for k,v in env.items() if not k.startswith(("AWS_", "KONAE_TEST_S3_", "MC_", "MINIO_"))}
        command = self.command
        if self.remote and measure and self.args.measure:
            command = [self.args.measure, *command]
        result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=600)
        output = result.stdout + result.stderr
        if error:
            assert result.returncode and error in output, (phase, output[-3000:])
        else:
            assert result.returncode == 0 and "1 passed" in output, (phase, output[-3000:])
        checks = self.summary["checks"]
        checks[phase] = checks.get(phase, 0) + 1
        self.summary["expectedRefusals"] += int(error is not None)
        for line in output.splitlines():
            if line.startswith(('{"processExit"', 'Reachable objects', 'Expired roots')):
                print(self.target, phase, line, flush=True)

    def cold(self, phase, measure=False, **settings):
        with tempfile.TemporaryDirectory(prefix="konae-cold-restore-") as clean:
            self.run(phase, measure=measure, KONAE_LAB_ROOT=clean,
                     KONAE_LAB_DESTINATION=str(self.root), **settings)

    def inventory(self):
        self.run("inventory")
        return tuple(json.loads((self.root / f"{name}.json").read_text()) for name in ("listed", "objects"))

    def mc(self, *args, data=None, **settings):
        return subprocess.run([self.args.mc, *args], input=data, capture_output=True,
                              env=dict(self.env, **settings), check=True, timeout=120).stdout

    def block_versions(self):
        if self.remote:
            return {o["key"]: (o["size"], o["lastModified"], o["etag"])
                    for line in self.mc("ls", "--json", PREFIX+"objects/").splitlines()
                    if (o := json.loads(line))["type"] == "file"}
        versions = {}
        for path in (self.data/"objects").iterdir():
            info = path.stat()
            versions[path.name] = (info.st_size, info.st_mtime_ns, info.st_ino)
        return versions

    def publish_unchanged(self):
        before = self.block_versions()
        self.run("next", measure=True)
        assert before and self.block_versions() == before
        print(f"PASS {self.target} stable publication preserved all {len(before)} stored block versions", flush=True)

    def bytes(self, name):
        return self.mc("cat", PREFIX+name) if self.remote else (self.data / name).read_bytes()

    def replace(self, name, content):
        if self.remote:
            if content is None:
                self.mc("rm", PREFIX+name)
            else:
                self.mc("pipe", PREFIX+name, data=content)
        elif content is None:
            (self.data / name).unlink()
        else:
            (self.data / name).write_bytes(content)

    def recover_abandoned(self, group):
        records = [json.loads(line)["key"] for line in self.mc("ls", "--json", PREFIX+group+"/").splitlines()]
        assert len(records) == 1
        self.mc("rm", PREFIX+group+"/"+records[0])

    @contextmanager
    def paused(self, phase, step, name):
        ready = self.root / name
        env = dict(self.env, KONAE_LAB_PHASE=phase, KONAE_LAB_PAUSE=step, KONAE_LAB_READY=str(ready))
        child = subprocess.Popen(self.command, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        try:
            until = time.monotonic() + 120
            while not ready.exists():
                assert child.poll() is None, child.stdout.read()
                assert time.monotonic() < until, name
                time.sleep(.025)
            yield child, ready.with_suffix(".resume")
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=10)
            child.stdout.close()

    def check_concurrency(self, before):
        busy = "reservations" if self.remote else "repository is busy"
        with self.paused("original", "restore", "reading") as (reader, proceed):
            with self.paused("retry", "publish", "writer1") as (writer, finish):
                with self.paused("abandon", "publish", "writer2") as (abandoned, _):
                    self.run("prune", busy)
                    abandoned.kill()
                    abandoned.wait(timeout=10)
                finish.touch()
                assert writer.wait(timeout=30) == 0
            self.run("prune", busy)
            proceed.touch()
            assert reader.wait(timeout=30) == 0
        if self.remote:
            self.run("prune", busy)
            self.recover_abandoned("users")
        with self.paused("prune", "mark", "marked") as (collector, _):
            self.run("prune", "collector reservation" if self.remote else busy)
            self.run("retry", "collector reservation" if self.remote else busy)
            self.run("original", "collector reservation" if self.remote else busy)
            collector.kill()
            collector.wait(timeout=10)
        if self.remote:
            self.run("prune", "collector reservation")
            self.recover_abandoned("collectors")
        roots, blocks = self.inventory()
        assert roots == before[0] and set(before[1]) < set(blocks) and len(blocks) == len(before[1]) + 1
        assert self.bytes("objects/"+next(iter(set(blocks)-set(before[1]))))

        print(f"PASS {self.target} two independent writers, restore, dead writer and collector killed after mark", flush=True)

    def readonly_restore(self):
        # All writers and collectors have exited. Seal this fixture for the duration of the mirror.
        mirror(self.args.mc, f"copies-v3/{REPO}/", self.data)
        self.cold("oldest", KONAE_LAB_TARGET="directory")
        self.cold("latest", KONAE_LAB_TARGET="directory")
        print("PASS read-only MinIO user mirrored the stationary prefix and restored both ends offline", flush=True)

    def exercise(self):
        if self.remote:
            assert not self.mc("ls", "--recursive", "--json", "lab/konae-test"), "Expected an empty bucket"
        self.run("seed", measure=True)
        before = self.inventory()
        ids, objects = before
        assert len(ids) == 1080 and len(objects) == 1082
        self.run("no-key", "No supplied v3 key")
        self.run("legacy")
        assert self.inventory() == before
        self.check_concurrency(before)
        before = self.inventory()

        # Authenticate the latest manifest and compare the listed size of its shared block.
        root_name = "points/"+ids[-1]
        original = self.bytes(root_name)
        self.replace(root_name, original[:-1])
        self.run("prune", "unreadable point")
        assert self.inventory() == before
        self.replace(root_name, original)
        if self.remote:
            for fault, error in [("no-token", "pagination"), ("page-two", "Listing"),
                                 ("full", "did not confirm"), ("timeout", "timeout")]:
                publishing = fault in ("full", "timeout")
                options = {"KONAE_LAB_STALL_PUT": "1"} if fault == "timeout" else {}
                with list_fault(self.env["KONAE_TEST_S3_ENDPOINT"], fault) as endpoint:
                    self.run("next" if publishing else "prune", error, KONAE_TEST_S3_ENDPOINT=endpoint, **options)
                if publishing:
                    self.recover_abandoned("users")
                assert self.inventory() == before
        self.run("prune", measure=True)
        retained, survivors = self.inventory()
        days = int(self.env["KONAE_LAB_DAYS"])
        cutoff = self.start + timedelta(days=days, hours=-1)
        assert ids[0] == self.start.strftime("%Y-%m-%dT%H-%M-%S.000Z")
        assert ids[-1] == cutoff.strftime("%Y-%m-%dT%H-%M-%S.000Z")
        expected = set(ids[-24:]) | {ids[1079-24*i] for i in range(31)}
        assert len(retained) == 54 and set(retained) == expected and len(survivors) == 56
        self.summary.update(seededPoints=len(ids), retainedPoints=len(retained),
                            retainedObjects=len(survivors), simulatedDays=days)
        if self.args.oracle:
            all_file = self.root/"all.json"
            all_file.write_text(json.dumps(ids))
            subprocess.run([sys.executable, self.args.oracle, all_file, self.root/"listed.json",
                            cutoff.strftime("%Y-%m-%dT%H-%M-%S.000Z")], check=True)
        self.cold("oldest", measure=True)
        self.cold("latest", measure=True)
        self.run("reachable")
        if self.remote:
            self.readonly_restore()
        self.publish_unchanged()
        self.run("next-prune", measure=True)
        self.cold("newest", measure=True)
        retained, survivors = self.inventory()
        missing = "objects/"+survivors[0]
        original = self.bytes(missing)
        garbage = "objects/"+("b"*64)
        assert garbage.split("/")[1] not in survivors
        self.replace(garbage, b"expired orphan")
        self.replace(missing, None)
        damaged = self.inventory()
        self.run("next-prune", "omitted 1 retained blocks")
        assert self.inventory() == damaged
        self.replace(missing, original[:-1])
        self.run("next-prune", "wrong length")
        self.replace(missing, original)
        self.run("next-prune")
        self.run("reachable")
        self.run("grace")
        self.summary["phase"] = "complete"
        print(f"PASS {self.target} retained 54/1080 roots and 56/1083 objects; stable publication reused both blocks", flush=True)


def historical(args, root):
    import runpy
    cut = runpy.run_path(str(Path(__file__).with_name("common-cut.py")))
    directory = Lab(args, root/"directory", "directory")
    remote = Lab(args, root/"remote", "s3")
    assert not remote.mc("ls", "--recursive", "--json", "lab/konae-test")
    for lab in (directory, remote):
        lab.root.mkdir()
        lab.run("retry")
    inventories = [lab.inventory() for lab in (directory, remote)]
    old = [f"2000-01-01T00-00-00.{i:03}Z" for i in range(25)]
    for i, point in enumerate(old):
        folder = directory.root/point
        folder.mkdir()
        m = dict(copyId=point, createdAt=946684800000+i, version=1, files=[])
        (folder/"manifest.json").write_text(json.dumps(m))
        m["version"] = 1 + i % 2
        remote.mc("pipe", f"lab/konae-test/copies/{point}/manifest.json", data=json.dumps(m).encode())
    live = root/"live"
    cut["fixture"](live)
    env = dict(cut["setting"](live), KONAE_COPY_DIR=str(directory.root), KONAE_COPY_S3="1",
               AWS_EC2_METADATA_DISABLED="true", TOKIO_WORKER_THREADS="2")
    for name in ("ENDPOINT", "BUCKET", "ACCESS_KEY", "SECRET_KEY"):
        env["KONAE_S3_"+name] = os.environ["KONAE_TEST_S3_"+name]

    with cut["server"](args.historical_kernel.resolve(), env) as (port, _):
        def complete():
            reports = cut["targets"](port)
            assert all(not r["lastError"] for r in reports), "Historical copy worker failed"
            return len(reports) == 2 and all(r["lastCopyId"] for r in reports)
        cut["wait"](complete, "Historical kernel did not finish both copies", 90)
    remote_dirs = {json.loads(line)["key"].rstrip("/") for line in remote.mc("ls", "--json", "lab/konae-test/copies/").splitlines()}
    assert [p for p in old if (directory.root/p).exists()] == old[2:]
    assert [p for p in old if p in remote_dirs] == old[2:]
    for lab, before in zip((directory, remote), inventories):
        assert lab.inventory() == before
        lab.cold("latest")
    print("PASS historical kernel removed two old manifests per destination and both v3 points restored")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--worker", required=True, type=Path)
    p.add_argument("--year", action="store_true", help="spread the 1,080 cuts across 365 simulated days")
    p.add_argument("--mc", type=Path)
    p.add_argument("--report", type=Path, help="write aggregate results for the retention campaign")
    p.add_argument("--measure", type=Path)
    p.add_argument("--oracle", type=Path)
    p.add_argument("--require-s3", action="store_true")
    p.add_argument("--historical-kernel", type=Path)
    p.add_argument("--capacity-only", action="store_true")
    args = p.parse_args()
    if args.capacity_only:
        import shutil
        volume = Path(os.environ["LAB_SMALL_VOLUME"])
        assert 0 < shutil.disk_usage(volume).total <= 144 * 1024**2
        with tempfile.TemporaryDirectory(dir=volume) as dest, tempfile.TemporaryDirectory() as stage:
            lab = Lab(args, Path(stage), "directory")
            lab.env["KONAE_LAB_DESTINATION"] = dest
            lab.run("retry")
            filler = Path(dest)/"fill"
            with filler.open("wb") as stream:
                for _ in range(shutil.disk_usage(volume).free // 1024**2 - 2):
                    stream.write(bytes(1024**2))
            lab.run("capacity", "No space left on device")
            filler.unlink()
            roots, _ = lab.inventory()
            assert len(roots) == 1
            lab.run("latest")
        print("PASS directory ENOSPC preserved the complete root and its restored bytes")
        print("SKIP S3 destination (directory capacity case)")
        return
    report = {"targets": {}, "skipped": []}
    targets = ["directory"]
    if args.mc and os.environ.get("KONAE_TEST_S3_ENDPOINT"):
        endpoint = urlsplit(os.environ["KONAE_TEST_S3_ENDPOINT"])
        alias = urlsplit(os.environ["MC_HOST_lab"])
        assert endpoint.hostname == alias.hostname == "127.0.0.1" and endpoint.port == alias.port
        assert os.environ["KONAE_TEST_S3_BUCKET"] == "konae-test"
        targets.append("s3")
    else:
        report["skipped"].append("s3")
        print("SKIP S3 destination (requires --mc inside with-minio)", flush=True)
        if args.require_s3 or args.historical_kernel:
            p.error("S3 destination required")
    if args.historical_kernel:
        with tempfile.TemporaryDirectory(prefix="konae-historical-prune-") as tmp:
            historical(args, Path(tmp))
        return
    for target in targets:
        with tempfile.TemporaryDirectory(prefix="konae-v3-prune-") as tmp:
            lab = Lab(args, Path(tmp), target)
            report["targets"][target] = lab.summary
            try:
                lab.exercise()
            finally:
                if args.report:
                    args.report.write_text(json.dumps(report, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
