"""Measure the same checked MCP mutations with copies disabled and enabled."""
import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import shutil
import tempfile
import time
import uuid

from pilot_fixture import cut, invoke, session
from pilot_observation import observed_server


def p99(samples):
    return sorted(samples)[math.ceil(len(samples) * .99) - 1]


def trial(binary, profile, mode):
    apps = json.loads((profile / "apps.json").read_bytes())
    assert len(apps) == 6
    result = {"mode": mode, "observation": {}}
    with tempfile.TemporaryDirectory(prefix="pilot-load-") as folder:
        root = Path(folder).resolve()
        live = root / "live"
        shutil.copytree(profile / "live", live)
        env = dict(cut["setting"](live), PATH=os.defpath, TOKIO_WORKER_THREADS="2",
                   KONAE_COPY_INTERVAL_SECONDS="40", KONAE_COPY_STAGE_BYTES=str(1024**3),
                   KONAE_COPY_READERS="512", KONAE_COPY_SECRET="disposable-write-latency-fixture-key")
        if mode == "legacy":
            env["KONAE_COPY_DIR"] = str(root / "repo")
        if mode == "candidate":
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
                    assert not t["lastError"], t["lastError"]
                    if t["lastCopyId"]:
                        point = t["lastCopyId"]
                        stamp = datetime.strptime(point, "%Y-%m-%dT%H-%M-%S.%fZ").replace(tzinfo=timezone.utc)
                        attempts[point] = (t.get("lastCutAt") or round(stamp.timestamp() * 1000), t["lastSuccessAt"])
                return targets

            if mode != "control":
                def first_copy():
                    observe()
                    return bool(attempts)
                cut["wait"](first_copy, "Initial pilot copy did not publish", 180)
                ready_at = min(at for at, _ in attempts.values()) / 1000
            time.sleep(max(0, ready_at + 36 - time.time()))
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
                def published():
                    observe()
                    return any(begin <= at <= end for at, _ in attempts.values())
                cut["wait"](published, "No published cut started within the write workload", 180)
            result["cutsStartedUnderLoad"] = sum(begin <= at <= end for at, _ in attempts.values())
            result["writesOverlappingPublication"] = sum(any(a <= stop and b >= start for a, b in attempts.values())
                                                        for start, stop in intervals)
            result["loadMs"] = end - begin
    result.update(samplesMs=[round(t, 4) for t in samples], n=len(samples),
                  p99Ms=p99(samples), maxMs=max(samples))
    return result


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary", type=Path, required=True)
    p.add_argument("--six", type=Path, required=True)
    p.add_argument("--modes", choices=("control", "legacy", "candidate"), nargs="+",
                   default=["control", "legacy", "candidate"])
    p.add_argument("--report", type=Path, required=True)
    args = p.parse_args()
    assert args.modes[0] == "control" and len(set(args.modes)) == len(args.modes)
    if args.report.exists():
        p.error("choose a new report path")
    results = []
    for mode in args.modes:
        result = trial(args.binary.resolve(strict=True), args.six, mode)
        result["addedP99Ms"] = result["p99Ms"] - results[0]["p99Ms"] if results else 0
        if results:
            result["p99PairedDifferenceMs"] = p99([ms - control for ms, control in
                                                  zip(result["samplesMs"], results[0]["samplesMs"], strict=True)])
        results.append(result)
        args.report.write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps({k: v for k, v in result.items() if k not in {"samplesMs", "observation"}}), flush=True)
