"""Compare installation writers on one cut. Run one case per empty MinIO bucket."""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time
import uuid

from pilot_delay import delayed_reads
from pilot_fixture import check_access, compare, cut, edit_restored_draft, inventory, invoke, session, small_profile, year
from pilot_observation import observed_server, Traffic, tree_bytes


def save(report, path):
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


def catalogue(binary, env, root):
    result = subprocess.run([binary, "inventory"], env=dict(env, KONAE_DATA_DIR=str(root)),
                            timeout=30, capture_output=True)
    if result.returncode:
        return {"certified": False, "exit": result.returncode, "reason": result.stderr.decode().strip().splitlines()[-1]}
    components = json.loads(result.stdout)["components"]
    return {"certified": True, "classes": len(components), "entries": sum(components.values())}


def run(args, root, report):
    binary = args.binary.resolve(strict=True)
    assert binary.name == ("kernel" if args.profile == "standalone" else "konae")
    check = subprocess.run([args.reader, "--list"], env=dict(cut["setting"](root / "reader-probe"),
                           KONAE_KERNEL_PORT="0"), capture_output=True, timeout=5, check=True)
    if b"copies::pilot::read_pilot_cut: test" not in check.stdout:
        raise ValueError("--reader must be the release kernel --lib test executable")
    remote = args.store != "local"
    live_s3 = remote and args.store != "replay"
    if remote:
        settings = year["remote_settings"](args.mc)
    else:
        settings = {}
        print("SKIP s3 and s3-v3: local file store selected; this runtime shares its S3 setting with copies.",
              flush=True)
    omitted = [] if remote else ["s3", "s3-v3"]
    if args.legacy_s3_only:
        if not live_s3:
            raise ValueError("--legacy-s3-only requires a live S3 file store")
        omitted = ["directory", "directory-v3", "s3-v3"]
        print("SKIP directory, directory-v3 and s3-v3: only the legacy S3 writer was requested", flush=True)
    report["omitted"] = omitted
    live = root / "live"
    base = dict(cut["setting"](live), PATH=os.defpath, AWS_EC2_METADATA_DISABLED="true", TOKIO_WORKER_THREADS="2")
    env = dict(base, **settings, KONAE_COPY_SECRET=secrets.token_hex(32))
    model = None
    if args.profile == "six":
        shutil.copytree(args.six / "live", live)
        # Open the historical fixture once to run this binary's startup migrations.
        with cut["server"](binary, base):
            pass
    else:
        model = small_profile(binary, live, base)
    report["liveBytes"] = tree_bytes(live)
    if not args.historical:
        report["sourceCatalogue"] = catalogue(binary, base, live)
    oracle = inventory(live, source=True)
    report["inventory"] = compare(oracle, oracle)
    if args.profile == "six":
        assets = json.loads((args.six / "expected.json").read_bytes())
        expected = {Path("apps") / a["app"] / "data/files" / (a["id"] + ".blob"):
                    bytes.fromhex(a["sha256"]) for a in assets}
        assert {p: v for p, v in oracle.items() if p.suffix == ".blob"} == expected
        report.update(privateFiles=len(expected), privateBytes=sum(a["size"] for a in assets))
    blocks = root / "copies-v3" / uuid.uuid4().hex
    targets = {"directory": root / "directory", "directory-v3": blocks}
    config = dict(KONAE_COPY_DIR=str(targets["directory"]), KONAE_COPY_V3_DIR=str(blocks),
                  KONAE_COPY_STAGE_BYTES=str(1024**3), KONAE_COPY_READERS="512")
    if remote:
        rid = uuid.uuid4().hex
        if live_s3:
            config.update(KONAE_COPY_S3="1", KONAE_COPY_V3_S3=rid)
        targets.update({"s3": "s3://konae-test/copies/", "s3-v3": "s3://konae-test/copies-v3/" + rid})

    if args.legacy_s3_only:
        targets = {"s3": targets["s3"]}
        config = {"KONAE_COPY_S3": "1", "KONAE_COPY_STAGE_BYTES": str(1024**3), "KONAE_COPY_READERS": "512"}

    def mc(*args_, **kwargs):
        return subprocess.run([args.mc, *args_], check=True, capture_output=True, timeout=120, **kwargs).stdout

    with ExitStack() as stack:
        traffic = stack.enter_context(Traffic(args.mc)) if remote else None
        if args.delay_file_get:
            assert args.profile == "standalone" and live_s3
            endpoint, slow, hits = stack.enter_context(delayed_reads(env["KONAE_S3_ENDPOINT"]))
            env["KONAE_S3_ENDPOINT"] = endpoint
        def phase(name):
            report["phase"] = name
            save(report, args.report)
            if traffic:
                traffic.boundary(name)
        if live_s3:
            if args.store == "versioned":
                mc("version", "enable", "lab/konae-test")
            phase("migration")
            subprocess.run([binary, "files-migrate", "--to", "s3"], env=env, check=True, capture_output=True)
        phase("publish")
        if args.delay_file_get:
            slow.set()
        observation = report["resources"] = {}
        online = set(targets) if live_s3 else {"directory", "directory-v3"}
        writer = env if live_s3 else {k: v for k, v in env.items() if not k.startswith("KONAE_S3_")}
        if args.second_cut:
            config["KONAE_COPY_INTERVAL_SECONDS"] = str(args.second_cut)
        with observed_server(binary, dict(writer, **config), cut["request"], observation) as port:
            def finished():
                status = cut["targets"](port)
                kinds = {s["kind"] for s in status}
                assert kinds == online, f"Missing targets {online - kinds}; unexpected {kinds - online}"
                if all(s["lastAttemptAt"] and not s.get("inProgress") and (s["lastCopyId"] or s["lastError"]) for s in status):
                    return status
            statuses = cut["wait"](finished, "pilot capture did not finish", 900)
            if args.second_cut:
                assert all(s["lastCopyId"] for s in statuses)
                first, = {s["lastCopyId"] for s in statuses}
                phase("next-publish")
                def second():
                    records = finished()
                    if records and all(s["lastCopyId"] != first or s["lastError"] for s in records):
                        return records
                following = cut["wait"](second, "No second complete cut", 900)
                first_cut, = {s["lastCutAt"] for s in statuses}
                report["ageBeforeNextPointMs"] = {s["kind"]: s["lastSuccessAt"] - first_cut for s in following
                                                  if not s["lastError"]}
                statuses = following
            phase("post-capture")
            if any(s["lastError"] for s in statuses):
                if model is None:
                    app = json.loads((args.six / "apps.json").read_bytes())[0]
                    user = session(port, app, "writer")
                    invoke(port, user, "notes.create", {"title": "after-refusal"})
                    assert any(r["title"] == "after-refusal" for r in invoke(port, user, "notes.list", {}))
                else:
                    email = year["MEMBERS"][0]
                    _, cookie = year["account"](port, email, "login")
                    year["action"](port, cookie, "notes.add", {"title": "after-refusal"})
                    model.rows["after-refusal"] = email
                    year["check_rows"](port, cookie, email, model)
                report["writeAfterRefusal"] = True
        if args.delay_file_get:
            report["delayedFileGet"] = hits.is_set()
            assert hits.is_set(), "The injected delay missed the live GET"
        for s in statuses:
            if "lastCutAt" not in s:
                # The historical writer names the copy for the start of the attempt.
                s["lastCutAt"] = s["lastAttemptAt"] if s["lastCopyId"] else None
                report["historicalAttemptTime"] = True
        phase("published")
        report["publication"] = {s["kind"]: dict(error=s["lastError"], point=s["lastCopyId"],
            cutMs=s["lastCutAt"], lagMs=s["lastSuccessAt"] - s["lastCutAt"] if s["lastCutAt"] else None)
            for s in statuses}
        if traffic:
            report["traffic"] = traffic.stats
            report["serverCounters"] = traffic.counters
        failures = [(s["kind"], "publish", s["lastError"]) for s in statuses if s["lastError"]]
        for s in statuses:
            if s["lastCopyId"] is None:
                target = s["kind"]
                source = targets.pop(target)
                if isinstance(source, Path):
                    points = list((source / "points").glob("*")) if target.endswith("v3") else list(source.glob("*/manifest.json"))
                else:
                    prefix = source.removeprefix("s3://konae-test/").rstrip("/")
                    entries = mc("ls", "--recursive", "--json", "lab/konae-test/" + prefix + "/")
                    points = [key for line in entries.splitlines() if (key := json.loads(line)["key"]).startswith("points/")
                              or key.endswith("/manifest.json")]
                assert not points, "A refused capture left a published point"
        complete = [s for s in statuses if s["lastCopyId"]]
        if not complete:
            raise AssertionError("No destination published a restorable point")
        point, = {s["lastCopyId"] for s in complete}
        at, = {s["lastCutAt"] for s in complete}
        assert point and at
        if args.store == "replay":
            for target in ["s3", "s3-v3"]:
                phase("replay-" + target)
                replay_env = dict(env, PILOT_COPY=point, PILOT_BINARY=binary.name,
                                  PILOT_DIRECTORY=str(targets["directory"]), PILOT_TARGET="s3" if target == "s3" else rid)
                start = time.monotonic()
                replay = subprocess.run([args.reader, "--exact", "copies::pilot::replay_pilot_cut", "--ignored"],
                                        env=replay_env, capture_output=True, timeout=900)
                assert replay.returncode == 0 and b"1 passed" in replay.stdout, replay.stdout[-800:]
                report["publication"][target] = dict(point=point, cutMs=at, replay=True,
                    replaySeconds=round(time.monotonic() - start, 3), lagMs=time.time_ns() // 1_000_000 - at)
        phase("storage")
        report["stored"] = {}
        for target, source in targets.items():
            if isinstance(source, Path):
                report["stored"][target] = tree_bytes(source)
            else:
                prefix = source.removeprefix("s3://konae-test/").rstrip("/")
                entries = mc("ls", "--recursive", "--versions", "--json", "lab/konae-test/" + prefix + "/")
                report["stored"][target] = sum(json.loads(line)["size"] for line in entries.splitlines())
        phase("remove-source")
        shutil.rmtree(live)
        if live_s3:
            args_ = ["--versions"] if args.store == "versioned" else []
            mc("rm", "--force", "--recursive", *args_, "lab/konae-test/files/")
            assert not mc("ls", "--recursive", "--versions", "--json", "lab/konae-test/files/").strip()
        report["restores"] = {}
        for target, source in targets.items():
            result = report["restores"][target] = {}
            with tempfile.TemporaryDirectory(dir=root) as tmp:
                temp = Path(tmp)
                dest = temp / "restored"
                phase("restore-" + target)
                start = time.monotonic()
                try:
                    command = [binary, "restore", "--from", source, "--copy", point]
                    output = subprocess.run(command, env=dict(env, KONAE_DATA_DIR=str(dest)), check=True,
                                            capture_output=True, timeout=1800)
                    assert b"Applied the revocation record" in output.stdout
                    restored = time.monotonic()
                    result["restoreSeconds"] = round(restored - start, 3)
                    result["access"] = check_access(binary, base, dest, args.six, model)
                    result["rtoSeconds"] = round(time.monotonic() - start, 3)
                    if args.profile == "draft":
                        result["editableDraftRevision"] = edit_restored_draft(binary, base, dest)
                    if not args.historical:
                        result["catalogue"] = catalogue(binary, base, dest)
                        if not result["catalogue"]["certified"]:
                            failures.append((target, "restored catalogue", result["catalogue"]["reason"]))
                except Exception as error:
                    failures.append((target, "access", str(error)))
                phase("inspect-" + target)
                image = temp / "image"
                try:
                    command = [args.reader, "--exact", "copies::pilot::read_pilot_cut", "--ignored", "--nocapture"]
                    started = time.monotonic()
                    read = subprocess.run(command, env=dict(env, PILOT_COPY=point, PILOT_IMAGE=str(image),
                        PILOT_BINARY=binary.name, PILOT_SOURCE=str(source)), capture_output=True, timeout=600)
                    assert read.returncode == 0 and b"1 passed" in read.stdout, read.stderr.decode()[-500:]
                    result["imageSeconds"] = round(time.monotonic() - started, 3)
                    result["inventory"] = compare(oracle, inventory(image))
                except Exception as error:
                    failures.append((target, "inventory", str(error)))
            save(report, args.report)
        phase("end")
        if not args.historical and not report["sourceCatalogue"]["certified"]:
            failures.append(("source", "catalogue", report["sourceCatalogue"]["reason"]))
        if failures:
            raise AssertionError(str(failures))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--reader", type=Path, required=True, help="release kernel --lib test executable")
    parser.add_argument("--profile", choices=["draft", "standalone", "six"], required=True)
    parser.add_argument("--six", type=Path, help="synthetic six-apps fixture and external oracle")
    parser.add_argument("--store", choices=["local", "unversioned", "versioned", "replay"], required=True)
    parser.add_argument("--delay-file-get", action="store_true", help="hold the standalone fixture's live GET for 2.2 s")
    parser.add_argument("--second-cut", type=int, metavar="SECONDS", help="observe age up to the next publication")
    parser.add_argument("--historical", action="store_true", help="af1a897 predates the inventory command")
    parser.add_argument("--legacy-s3-only", action="store_true", help="isolate the v2 writer on a live S3 file store")
    parser.add_argument("--mc", default="mc")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if args.profile == "six" and not args.six:
        parser.error("six needs --six")
    if args.second_cut and (args.second_cut < 2 or args.historical or args.store == "replay"):
        parser.error("choose an integrated live writer and an interval of at least two seconds")
    if args.historical and not args.legacy_s3_only:
        parser.error("the historical measurement requires --legacy-s3-only")
    if args.report.exists():
        parser.error("choose a new report path")
    os.umask(0o077)
    report = dict(profile=args.profile, store=args.store, result="failed")
    try:
        with tempfile.TemporaryDirectory(prefix="writer-pilot-") as tmp:
            run(args, Path(tmp).resolve(), report)
        report["result"] = "passed"
    except Exception as error:
        report["failure"] = str(error)
        raise
    finally:
        save(report, args.report)
        print(f"{args.profile} {args.store}: {report['result']} ({args.report})")


if __name__ == "__main__":
    main()
