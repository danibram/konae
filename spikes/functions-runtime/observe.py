#!/usr/bin/env python3
"""Record idle process costs and cold Deno requests after explicit eviction."""

import argparse
import datetime
import json
from pathlib import Path
import time

from benchmark import connection, database_state, process_sample, request


def control(url, path, method="GET"):
    conn = connection(url)
    try:
        conn.request(method, path)
        response = conn.getresponse()
        value = json.loads(response.read())
        if response.status != 200:
            raise RuntimeError({"path": path, "status": response.status, "body": value})
        return value
    finally:
        conn.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--targets", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--idle-seconds", type=float, default=5)
    parser.add_argument("--cold-target", default="deno")
    parser.add_argument("--cold-rounds", type=int, default=5)
    args = parser.parse_args()
    targets = json.loads(args.targets.read_text())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    result = {
        "at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "idle": [], "cold": [],
        "notes": [
            "Idle CPU comes from ps cumulative process-tree time and may round to zero.",
            "RSS is sampled process-tree RSS; shared pages may be counted more than once.",
            "Cold means no retained function instance; filesystem caches and V8 process initialization may remain warm.",
            "Cold requests include HTTP, auth, module loading, function execution and SQLite; no snapshot is assumed.",
        ],
    }
    before = {target["name"]: process_sample(target["pid"]) for target in targets}
    started = time.perf_counter()
    time.sleep(args.idle_seconds)
    elapsed = time.perf_counter() - started
    for target in targets:
        after = process_sample(target["pid"])
        initial = before[target["name"]]
        result["idle"].append({
            "target": target["name"], "seconds": elapsed,
            "cpu_ms": (after["cpu_s"] - initial["cpu_s"]) * 1000,
            "rss_before_kib": initial["rss_kib"], "rss_after_kib": after["rss_kib"],
        })
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    target = next((target for target in targets if target["name"] == args.cold_target), None)
    if target:
        for run in range(1, args.cold_rounds + 1):
            evicted = control(target["url"], "/__control/evict", "POST")
            # Allow the idle worker to dispose its runtime before the next request.
            time.sleep(0.1)
            stats_before = control(target["url"], "/__stats")
            if stats_before["runtimesLive"] != 0 or stats_before["instances"]:
                raise RuntimeError(f"Cold sample still has live instances: {stats_before}")
            expected, _ = database_state(target["data_dir"])
            conn = connection(target["url"])
            try:
                latency = request(conn, "get", expected)
            finally:
                conn.close()
            stats_after = control(target["url"], "/__stats")
            if (stats_after["runtimesCreated"] != stats_before["runtimesCreated"] + 1
                    or stats_after["runtimesLive"] != 1):
                raise RuntimeError(f"Cold sample did not create exactly one runtime: {stats_after}")
            result["cold"].append({
                "run": run, "latency_ms": latency, "eviction": evicted,
                "stats_before": stats_before, "stats_after": stats_after,
            })
            args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
