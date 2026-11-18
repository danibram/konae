"""Measure the same checked MCP mutations with copies disabled and enabled."""
import argparse
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


def trial(binary, profile, mode):
    apps = json.loads((profile / "apps.json").read_bytes())
    assert len(apps) == 6
    result = {"mode": mode, "observation": {}}
    with tempfile.TemporaryDirectory(prefix="pilot-load-") as folder:
        root = Path(folder).resolve()
        live = root / "live"
        shutil.copytree(profile / "live", live)
        env = dict(cut["setting"](live), PATH=os.defpath, TOKIO_WORKER_THREADS="2",
                   KONAE_COPY_INTERVAL_SECONDS="4", KONAE_COPY_STAGE_BYTES=str(1024**3),
                   KONAE_COPY_READERS="512", KONAE_COPY_SECRET="disposable-write-latency-fixture-key")
        if mode == "legacy":
            env["KONAE_COPY_DIR"] = str(root / "repo")
        if mode == "candidate":
            env["KONAE_COPY_V3_DIR"] = str(root / "copies-v3" / uuid.uuid4().hex)
        samples = []
        attempts = {}
        with observed_server(binary, env, cut["request"], result["observation"]) as port:
            users = [session(port, app, "writer") for app in apps]

            def observe():
                targets = cut["targets"](port)
                assert len(targets) == int(mode != "control")
                for t in targets:
                    assert not t["lastError"], t["lastError"]
                    if t["lastCopyId"]:
                        attempts[t["lastCopyId"]] = (t.get("lastCutAt") or t["lastAttemptAt"], t["lastSuccessAt"])
                return targets

            begin = time.time_ns() // 1_000_000
            for i in range(36):
                for user in users:
                    title = f"pilot-{i}"
                    start = time.perf_counter_ns()
                    invoke(port, user, "notes.create", {"title": title})
                    samples.append((time.perf_counter_ns() - start) / 1_000_000)
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
            result["loadMs"] = end - begin
    samples.sort()
    result.update(samplesMs=[round(t, 4) for t in samples], n=len(samples),
                  p99Ms=samples[math.ceil(len(samples) * .99) - 1], maxMs=max(samples))
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
        results.append(result)
        args.report.write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps({k: v for k, v in result.items() if k not in {"samplesMs", "observation"}}), flush=True)
