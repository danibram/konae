"""Time the same checked MCP mutations with copies disabled and enabled."""
import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid

from pilot_fixture import cut, invoke, session, year
from pilot_observation import observed_server


def p99(samples):
    return sorted(samples)[math.ceil(len(samples) * .99) - 1]


def trial(args, mode, s3):
    binary = args.binary.resolve(strict=True)
    apps = json.loads((args.six / "apps.json").read_bytes())
    assert len(apps) == 6
    result = {"mode": mode, "store": args.store, "observation": {}, "startUtc": datetime.now(timezone.utc).isoformat(),
              "loadAverageBefore": os.getloadavg()}
    with tempfile.TemporaryDirectory(prefix="pilot-load-") as folder:
        root = Path(folder).resolve()
        live = root / "live"
        shutil.copytree(args.six / "live", live)
        env = dict(cut["setting"](live), PATH=os.defpath, TOKIO_WORKER_THREADS="2", **s3,
                   KONAE_COPY_INTERVAL_SECONDS=str(args.interval), KONAE_COPY_STAGE_BYTES=str(1024**3),
                   KONAE_COPY_READERS="512", KONAE_COPY_SECRET="disposable-write-latency-fixture-key")
        if s3:
            subprocess.run([binary, "files-migrate", "--to", "s3"], env=env, check=True, capture_output=True)
        if mode == "legacy":
            if s3:
                env["KONAE_COPY_S3"] = "1"
            else:
                env["KONAE_COPY_DIR"] = str(root / "repo")
        if mode == "candidate":
            if s3:
                env["KONAE_COPY_V3_S3"] = uuid.uuid4().hex
            else:
                env["KONAE_COPY_V3_DIR"] = str(root / "copies-v3" / uuid.uuid4().hex)
        samples = []
        attempts = {}
        intervals = []
        ready_at = time.time()
        with observed_server(binary, env, cut["request"], result["observation"]) as port:
            users = [session(port, app, "writer") for app in apps]

            def observe():
                targets = cut["targets"](port)
                assert len(targets) == int(mode != "control")
                for t in targets:
                    at = t["lastAttemptAt"]
                    if at is None or t.get("inProgress"):
                        continue
                    if t["lastError"]:
                        attempts.setdefault(at, (time.time_ns() / 1_000_000, False))
                    elif t["lastSuccessAt"] and t["lastSuccessAt"] >= at:
                        point = datetime.strptime(t["lastCopyId"], "%Y-%m-%dT%H-%M-%S.%fZ")
                        stamp = round(point.replace(tzinfo=timezone.utc).timestamp() * 1000)
                        if stamp >= at:
                            attempts[at] = t["lastSuccessAt"], True
                return attempts

            if mode != "control":
                cut["wait"](observe, "Initial pilot copy did not finish", 300)
                ready_at = min(attempts) / 1000
            time.sleep(max(0, ready_at + args.interval - 4 - time.time()))
            begin = time.time_ns() // 1_000_000
            for i in range(36):
                for user in users:
                    title = f"pilot-{i}"
                    wall = time.time_ns() / 1_000_000
                    start = time.perf_counter_ns()
                    invoke(port, user, "notes.create", {"title": title})
                    samples.append((time.perf_counter_ns() - start) / 1_000_000)
                    intervals.append((wall, wall + samples[-1]))
                    assert any(r["title"] == title for r in invoke(port, user, "notes.list", {}))
                    time.sleep(.055)
                observe()
            end = time.time_ns() // 1_000_000
            if mode != "control":
                def under_load():
                    return any(begin <= at <= end for at in observe())
                try:
                    cut["wait"](under_load, "No attempt began within the write workload", 180)
                except AssertionError as e:
                    result["error"] = str(e)
            result["attemptsUnderLoad"] = sum(begin <= at <= end for at in attempts)
            result["publishedUnderLoad"] = sum(begin <= at <= end and ok for at, (_, ok) in attempts.items())
            result["writesOverlappingAttempt"] = sum(any(at <= stop and last >= start
                                                         for at, (last, _) in attempts.items())
                                                     for start, stop in intervals)
            result["loadMs"] = end - begin
    result["loadAverageAfter"] = os.getloadavg()
    result["endUtc"] = datetime.now(timezone.utc).isoformat()
    result.update(samplesMs=[round(t, 4) for t in samples], n=len(samples), p99Ms=p99(samples), maxMs=max(samples))
    return result


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary", type=Path, required=True)
    p.add_argument("--six", type=Path, required=True)
    p.add_argument("--modes", choices=("control", "legacy", "candidate"), nargs="+",
                   default=["control", "legacy", "candidate"])
    p.add_argument("--store", choices=["local", "unversioned", "versioned"], default="local")
    p.add_argument("--interval", type=int, default=90)
    p.add_argument("--mc", default="mc")
    p.add_argument("--report", type=Path, required=True)
    args = p.parse_args()
    assert args.modes[0] == "control" and len(set(args.modes)) == len(args.modes)
    if args.report.exists() or args.interval < 8:
        p.error("choose a new report and an interval of at least eight seconds")
    remote = year["remote_settings"](args.mc) if args.store != "local" else {}
    if args.store == "versioned":
        subprocess.run([args.mc, "version", "enable", "lab/konae-test"], check=True, capture_output=True)
    results = []
    for mode in args.modes:
        result = trial(args, mode, remote)
        result["addedP99Ms"] = result["p99Ms"] - results[0]["p99Ms"] if results else 0
        if results:
            result["p99PairedDifferenceMs"] = p99([ms - control for ms, control in
                                                  zip(result["samplesMs"], results[0]["samplesMs"], strict=True)])
        results.append(result)
        args.report.write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps({k: v for k, v in result.items() if k not in {"samplesMs", "observation"}}), flush=True)
        if remote:
            versions = ["--versions"] if args.store == "versioned" else []
            subprocess.run([args.mc, "rm", "--force", "--recursive", *versions, "lab/konae-test/"],
                           check=True, capture_output=True)
    if any("error" in r or r["publishedUnderLoad"] < 1 for r in results if r["mode"] != "control"):
        raise SystemExit("A writer did not publish under load; its timing samples remain in the report")
