"""Exercise v3 through the installation writer and the offline restore command."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from urllib.parse import urlsplit

from readonly_replica import mirror

cut = runpy.run_path(str(Path(__file__).with_name("common-cut.py")))
server, wait = cut["server"], cut["wait"]
FIRST = "synthetic-first-block-data-secret-for-the-lab"
SECOND = "synthetic-second-block-data-secret-for-the-lab"
LEDGER = "synthetic-stable-ledger-secret-for-the-lab"


def restore(binary, env, root, source, point, witness=None, ok=True):
    output = root / "restored"
    shutil.rmtree(output, ignore_errors=True)
    command = [binary, "restore", "--from", source, "--copy", point]
    if witness:
        command.extend(["--require-revocation", witness])
    result = subprocess.run(command, env=dict(env, KONAE_DATA_DIR=str(output)), capture_output=True, timeout=60)
    assert (result.returncode == 0) == ok, result.stderr.decode()
    if not ok:
        assert not output.exists() or not {p.name for p in output.iterdir()} - {"kernel.lock"}
    return output


def rejected_config(binary, env, root):
    for name, destination in [("live-alias", root / "live"), ("legacy-alias", root / "legacy")]:
        destination.mkdir(exist_ok=True)
        link = root / name
        link.symlink_to(destination, target_is_directory=True)
        candidate = link / "missing/copies-v3" / uuid.uuid4().hex
        settings = dict(env, KONAE_COPY_V3_DIR=str(candidate), KONAE_COPY_DIR=str(root / "legacy"))
        with socket.socket() as sock:
            sock.bind(("127.0.0.1",0))
            settings["KONAE_KERNEL_PORT"] = str(sock.getsockname()[1])
        out = subprocess.run([binary], env=settings, capture_output=True, timeout=15)
        assert out.returncode and (b"outside the data directory" in out.stderr or b"separate directory trees" in out.stderr), out.stderr
        assert not (destination / "missing").exists()
    for name in (".partial-fixture", "2026-01-01T00-00-00.000Z"):
        candidate = root / name / "copies-v3" / uuid.uuid4().hex
        settings = dict(env, KONAE_COPY_V3_DIR=str(candidate), KONAE_KERNEL_PORT=settings["KONAE_KERNEL_PORT"])
        out = subprocess.run([binary],env=settings,capture_output=True,timeout=15)
        assert out.returncode and b"legacy cleaner" in out.stderr, out.stderr
        assert not (root/name).exists()
    print("PASS two alias overlaps and two legacy cleaner paths refused before repository creation", flush=True)


class Remote:
    def __init__(self, mc, repo):
        self.mc = mc
        self.path = "lab/konae-test/copies-v3/"+repo+"/"
        self.source = "s3://konae-test/copies-v3/"+repo

    def command(self, action, path, data=None):
        return subprocess.run([self.mc, action, self.path+path], input=data, capture_output=True,
                              timeout=30, check=True).stdout

    def list(self, group):
        result = subprocess.run([self.mc, "ls", "--json", self.path+group+"/"], capture_output=True, check=True)
        return {item["key"]: item["lastModified"] for line in result.stdout.splitlines()
                if (item := json.loads(line))["type"] == "file"}


def exercise(args, root, remote):
    binary = args.binary.resolve()
    live = root / "live"
    cut["fixture"](live)
    db_secret = Path("apps/probe/data/secrets.sqlite")
    subprocess.run([sys.executable, args.deleted_secret, "seed", live/db_secret], check=True)
    env = dict(cut["setting"](live), PATH=os.defpath, AWS_EC2_METADATA_DISABLED="true",
               TOKIO_WORKER_THREADS="2", KONAE_COPY_SECRET=LEDGER, KONAE_COPY_V3_SECRET=FIRST)
    rejected_config(binary, env, root)
    if remote:
        for key in ("ENDPOINT", "BUCKET", "ACCESS_KEY", "SECRET_KEY"):
            env["KONAE_S3_"+key] = os.environ["KONAE_TEST_S3_"+key]
        assert urlsplit(env["KONAE_S3_ENDPOINT"]).hostname == "127.0.0.1"
        assert env["KONAE_S3_BUCKET"] == "konae-test"
    with server(binary.with_name("kernel"), env) as (port, _):
        cut["rpc"](port, "actions_invoke", {"name":"notes.add","input":{}})
        assert cut["app_count"](port) == 1
    email = "owner@block-format.test"
    with server(binary, env) as (port, _):
        cut["request"](port, "/_konae/owner/signup",
            dict(email=email, password="synthetic-password-for-this-fixture", setupKey=env["KONAE_SETUP_KEY"]))

    if remote:
        env["KONAE_COPY_V3_S3"] = remote.source.rsplit("/",1)[1]
        if args.marker:
            args.marker.unlink(missing_ok=True)
            with server(binary, env) as (port, proc):
                wait(args.marker.exists, "slow transfer did not reach its marker", 60)
                prior_objects = remote.list("objects")
                assert prior_objects and not remote.list("points")
                proc.kill()
                proc.wait(timeout=10)
            assert not remote.list("points")

            # The dead writer was uploading a block and never sent a point PUT.
            reservation, = remote.list("users")
            remote.command("rm", "users/"+reservation)
            assert not remote.list("users")
            print("PASS interrupted writer left one S3 reservation", flush=True)

    directory = root / "copies-v3" / uuid.uuid4().hex
    legacy = root / "legacy"
    env.update(KONAE_COPY_V3_DIR=str(directory), KONAE_COPY_DIR=str(legacy))
    if remote:
        env["KONAE_COPY_S3"] = "1"
    expected_targets = {"directory", "directory-v3"} | ({"s3", "s3-v3"} if remote else set())

    def capture(writes=False):
        started_at = time.time_ns() // 1_000_000
        with server(binary, env) as (port, _):
            if writes:
                def local_point():
                    report = next(r for r in cut["targets"](port) if r["kind"] == "directory-v3")
                    point = report["lastCopyId"]
                    if point and (directory / "points" / point).is_file():
                        return point

                # The interrupted connection can still touch the proxy marker.
                pinned = wait(local_point, "retry did not publish its directory-v3 manifest", 60)
                writer, = wait(lambda: remote.list("users"), "retry did not reserve s3-v3", 60)
                args.marker.unlink(missing_ok=True)
                wait(args.marker.exists, "no slow transfer during retry", 60)
                assert not remote.list("points"), "s3-v3 published before the five writes"
                started = time.monotonic()
                for i in range(5):
                    cut["rpc"](port, "actions_invoke", {"name":"notes.add","input":{}})
                    assert cut["app_count"](port) == i+2
                latency = time.monotonic()-started
                assert set(remote.list("users")) == {writer}, "s3-v3 released its writer during the five writes"
                assert not remote.list("points"), "s3-v3 published during the five writes"
                assert any(r["kind"] == "s3-v3" and r["inProgress"] for r in cut["targets"](port))
                print(f"PASS directory-v3 published {pinned} before five HTTP writes and read-backs "
                      f"in {latency:.3f} s while s3-v3 held its writer reservation", flush=True)
            def finished():
                reports = cut["targets"](port)
                if any(r["inProgress"] or (r["lastAttemptAt"] or 0) < started_at for r in reports):
                    return None
                for report in reports:
                    assert not report["lastError"], report["lastError"]
                    assert report["lastSuccessAt"] >= report["lastAttemptAt"]
                return reports
            reports = wait(finished, "copy remained incomplete", 120)
            assert {r["kind"] for r in reports} == expected_targets
            ids = {r["lastCopyId"] for r in reports}
            assert len(ids) == 1
            if writes:
                assert ids == {pinned}, "destinations reported a different cut from the directory-v3 manifest"
            return ids.pop()

    first = capture(writes=bool(args.marker))
    if remote and args.marker:
        retried = remote.list("objects")
        assert all(retried.get(k) == modified for k, modified in prior_objects.items())
        print(f"PASS killed S3 upload left zero points and retry kept {len(prior_objects)} existing objects", flush=True)
    sample = (live/"apps/probe/app/sample.bin").read_bytes()
    # The fixture's largest file occupies one object with a unique length.
    size = len(sample)+16
    local_object, = [p for p in (directory/"objects").iterdir() if p.stat().st_size == size]
    local_bytes = local_object.read_bytes()
    if remote:
        records = subprocess.check_output([remote.mc,"ls","--json",remote.path+"objects/"])
        remote_name, = [o["key"] for line in records.splitlines() if (o:=json.loads(line))["size"] == size]
        remote_bytes = remote.command("cat","objects/"+remote_name)
        assert remote_name != local_object.name and remote_bytes != local_bytes
    env.update(KONAE_COPY_V3_SECRET=SECOND, KONAE_COPY_V3_PREVIOUS_SECRETS=json.dumps([FIRST]))
    subprocess.run([binary,"owner-account-suspend",email],env=env,check=True,stdout=subprocess.DEVNULL)
    second = capture()
    assert first < second
    journal = live / ".copy-operations"
    journal.mkdir(exist_ok=True)
    (journal/"fixture.json").write_text(json.dumps({"subject":"probe","phase":"unfinished fixture write"}))
    with server(binary, env) as (port, _):
        def refused():
            reports = cut["targets"](port)
            return reports if all("Unfinished operation" in (r["lastError"] or "") for r in reports) else None
        reports = wait(refused, "a destination accepted an incomplete operation", 30)
        assert {r["kind"] for r in reports} == expected_targets
        assert all(r["lastCopyId"] == second for r in reports)
    assert {p.name for p in (directory/"points").iterdir()} == {first,second}
    if remote:
        assert set(remote.list("points")) == {first,second}
    print(f"PASS unfinished operation refused on {len(reports)} targets with previous cut retained",flush=True)
    shutil.rmtree(live)
    sources = [("directory-v3",directory),("directory",legacy)]
    if remote:
        sources.extend([("s3-v3",remote.source),("s3","s3://konae-test/copies/")])
    for kind, source in sources:
        if kind.startswith("s3"):
            prefix = "lab/konae-test/copies/revocations/" if kind == "s3" else remote.path+"revocations/"
            files = subprocess.check_output([remote.mc,"ls","--json",prefix])
            record, = [o["key"] for line in files.splitlines() if (o:=json.loads(line))["key"] != "started.json"]
            signed = subprocess.check_output([remote.mc,"cat",prefix+record])
        else:
            record, = [p for p in (source/"revocations").iterdir() if p.name != "started.json"]
            signed = record.read_bytes()
        witness = hashlib.sha256(signed).hexdigest()
        elapsed = []
        for point in [first,second]:
            started = time.monotonic()
            output = restore(binary,env,root,source,point,witness if point == first else None)
            elapsed.append(time.monotonic() - started)
            assert cut["number"](output/"owners.sqlite","SELECT count(*) FROM app_users WHERE state='suspended'") == 1
            count = 6 if args.marker and point == second else 1
            actual = cut["number"](output/"apps/probe/data/database.sqlite","SELECT count(*) FROM notes")
            cut_name = "first" if point == first else "second"
            rows = f"{kind} {cut_name} cut {point} notes expected={count} restored={actual}"
            print(rows, flush=True)
            assert actual == count, rows
            assert (output/"apps/probe/app/sample.bin").read_bytes() == sample
            subprocess.run([sys.executable,args.deleted_secret,"verify",output/db_secret],check=True,stdout=subprocess.DEVNULL)
        if kind == "s3-v3":
            prefix = remote.source.removeprefix("s3://konae-test/")+"/"
            replica = root/"emergency"/prefix
            mirror(remote.mc, prefix, replica)
            offline = {k:v for k,v in env.items() if not k.startswith(("KONAE_S3_", "KONAE_COPY_S3", "KONAE_COPY_V3_S3"))}
            output = restore(binary, offline, root, replica, first, witness)
            assert cut["number"](output/"owners.sqlite", "SELECT count(*) FROM app_users WHERE state='suspended'") == 1
            assert (output/"apps/probe/app/sample.bin").read_bytes() == sample
            print("PASS offline installation restore from read-only replica with post-cut revocation", flush=True)
        if kind.endswith("v3"):
            def replace_record(data):
                if kind == "directory-v3":
                    record.write_bytes(data)
                else:
                    remote.command("pipe", "revocations/"+record, data)
            unsigned = json.loads(signed)
            del unsigned["mac"]
            replace_record(json.dumps(unsigned).encode())
            restore(binary,env,root,source,first,witness,ok=False)
            held = restore(binary,env,root,source,first)
            assert (held/"recovery-hold.json").is_file()
            operator = dict(env,KONAE_DATA_DIR=str(held))
            bypass = subprocess.run([binary,"reconcile","--from",legacy],env=operator,capture_output=True)
            assert bypass.returncode and (held/"recovery-hold.json").is_file()
            replace_record(signed)
            subprocess.run([binary,"reconcile","--from",source],env=operator,check=True,stdout=subprocess.DEVNULL)
            assert not (held/"recovery-hold.json").exists()
            assert cut["number"](held/"owners.sqlite","SELECT count(*) FROM app_users WHERE state='suspended'") == 1
            missing = {k:v for k,v in env.items() if k != "KONAE_COPY_V3_PREVIOUS_SECRETS"}
            restore(binary,missing,root,source,first,ok=False)
        print(f"PASS {kind} restored both cuts in {elapsed[0]:.3f}/{elapsed[1]:.3f} s with the suspended owner and zero deleted markers", flush=True)
    for backend in (["directory", "s3"] if remote else ["directory"]):
        local = backend == "directory"
        source = directory if local else remote.source
        original = local_bytes if local else remote_bytes
        foreign = remote_bytes if local and remote else local_bytes
        variants = [original[:-1], bytes([original[0]^1])+original[1:], None]
        if remote:
            variants.append(foreign)
        for data in variants:
            if local:
                if data is None:
                    local_object.unlink()
                else:
                    local_object.write_bytes(data)
            else:
                remote.command("rm" if data is None else "pipe", "objects/"+remote_name, data)
            restore(binary,env,root,source,first,ok=False)
        print(f"PASS {backend} refused {len(variants)} damaged or missing objects before activation", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--deleted-secret", type=Path, default=Path(__file__).resolve().parents[2] /
                        "kernel-rs/test-fixtures/copies/deleted-secret.py")
    parser.add_argument("--mc", type=Path)
    parser.add_argument("--marker", type=Path)
    parser.add_argument("--require-s3", action="store_true")
    args = parser.parse_args()
    remote = None
    if args.mc and os.environ.get("KONAE_TEST_S3_ENDPOINT"):
        alias = urlsplit(os.environ["MC_HOST_lab"])
        assert alias.scheme == "http" and alias.hostname == "127.0.0.1"
        remote = Remote(args.mc,uuid.uuid4().hex)
    else:
        print("SKIP S3 destination (pass --mc inside with-minio)", flush=True)
        if args.require_s3 or args.marker:
            parser.error("S3 destination required")
    if not args.marker:
        print("SKIP slow-upload interruption and concurrent HTTP writes (pass --marker inside slow-s3)", flush=True)
    with tempfile.TemporaryDirectory(prefix="konae-block-test-") as tmp:
        exercise(args,Path(tmp),remote)


if __name__ == "__main__":
    main()
