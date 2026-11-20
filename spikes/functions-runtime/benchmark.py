#!/usr/bin/env python3
"""Measure equivalent loopback endpoints and verify every returned body."""

import argparse
import concurrent.futures
import datetime
import http.client
import json
import math
import os
from pathlib import Path
import platform
import resource
import sqlite3
import statistics
import subprocess
import time
from urllib.parse import urlsplit


def cpu_seconds(value):
    days = 0
    if "-" in value:
        day, value = value.split("-", 1)
        days = int(day)
    parts = [float(part) for part in value.split(":")]
    total = 0.0
    for part in parts:
        total = total * 60 + part
    return days * 86400 + total


def process_sample(pid):
    output = subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,time=,rss="], text=True
    )
    processes = {}
    for line in output.splitlines():
        process_id, parent_id, cpu, rss = line.split()
        processes[int(process_id)] = {
            "parent": int(parent_id), "cpu_s": cpu_seconds(cpu), "rss_kib": int(rss)
        }
    if pid not in processes:
        raise RuntimeError(f"Server process {pid} is absent")
    included = {pid}
    while True:
        children = {p for p, data in processes.items() if data["parent"] in included}
        expanded = included | children
        if expanded == included:
            break
        included = expanded
    return {
        "pids": sorted(included),
        "cpu_s": sum(processes[p]["cpu_s"] for p in included),
        "rss_kib": sum(processes[p]["rss_kib"] for p in included),
    }


def database_state(directory):
    path = (Path(directory) / "a.sqlite").resolve()
    with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True) as db:
        row = db.execute("SELECT id, owner_id, quantity FROM orders WHERE id = 1").fetchone()
        audit = db.execute("SELECT COUNT(*) FROM audit").fetchone()[0]
    return {"id": row[0], "ownerId": row[1], "quantity": row[2]}, audit


def connection(url):
    parsed = urlsplit(url)
    if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise ValueError("Benchmarks may only target loopback HTTP servers")
    return http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=10)


def request(conn, workload, expected):
    body = '{"quantity":3}' if workload == "post" else None
    headers = {"Cookie": "session=a-alice", "Accept": "application/json"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    started = time.perf_counter_ns()
    conn.request("POST" if workload == "post" else "GET", "/a/orders/1", body, headers)
    response = conn.getresponse()
    payload = response.read()
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    parsed = json.loads(payload)
    if response.status != 200 or parsed != expected:
        raise AssertionError({"status": response.status, "actual": parsed, "expected": expected})
    return elapsed_ms


def perform(target, workload, count, concurrency, expected):
    def worker(size):
        latencies = []
        errors = []
        conn = connection(target["url"])
        try:
            for _ in range(size):
                try:
                    latencies.append(request(conn, workload, expected))
                except Exception as exc:
                    errors.append(str(exc))
                    conn.close()
                    conn = connection(target["url"])
        finally:
            conn.close()
        return latencies, errors

    sizes = [count // concurrency + (i < count % concurrency) for i in range(concurrency)]
    if concurrency == 1:
        return worker(sizes[0])
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
        results = list(pool.map(worker, sizes))
    return (
        [latency for latencies, _ in results for latency in latencies],
        [error for _, errors in results for error in errors],
    )


def percentile(values, percent):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * percent) - 1)]


def client_cpu():
    usage = resource.getrusage(resource.RUSAGE_SELF)
    return usage.ru_utime + usage.ru_stime


def timed_round(target, workload, count, concurrency, run):
    expected, _ = database_state(target["data_dir"])
    if workload == "post":
        expected["quantity"] = 3
    _, warmup_errors = perform(target, workload, 200, 1, expected)
    if warmup_errors:
        raise RuntimeError({"target": target["name"], "warmup_errors": warmup_errors[:5]})
    before_row, before_audit = database_state(target["data_dir"])
    before = process_sample(target["pid"])
    cpu_start = client_cpu()
    start = time.perf_counter()
    latencies, errors = perform(target, workload, count, concurrency, expected)
    wall_s = time.perf_counter() - start
    client_s = client_cpu() - cpu_start
    after = process_sample(target["pid"])
    after_row, after_audit = database_state(target["data_dir"])
    expected_audit_delta = count if workload == "post" else 0
    if after_audit - before_audit != expected_audit_delta:
        errors.append(f"audit delta {after_audit - before_audit}, expected {expected_audit_delta}")
    if after_row != expected or (workload == "get" and before_row != after_row):
        errors.append(f"database row mismatch: {after_row}")
    runtime_stats = None
    if target["name"] == "deno":
        conn = connection(target["url"])
        try:
            conn.request("GET", "/__stats")
            response = conn.getresponse()
            runtime_stats = json.loads(response.read())
            if response.status != 200:
                errors.append("Deno stats unavailable")
        finally:
            conn.close()
    return {
        "target": target["name"], "workload": workload, "run": run,
        "concurrency": concurrency, "attempted": count, "verified_responses": len(latencies),
        "errors": len(errors), "error_examples": errors[:5],
        "wall_s": wall_s, "rps": count / wall_s,
        "p50_ms": percentile(latencies, 0.50) if latencies else None,
        "p95_ms": percentile(latencies, 0.95) if latencies else None,
        "p99_ms": percentile(latencies, 0.99) if latencies else None,
        "server_cpu_ms_per_request": (after["cpu_s"] - before["cpu_s"]) * 1000 / count,
        "client_cpu_ms_per_request": client_s * 1000 / count,
        "client_cpu_fraction": client_s / wall_s,
        "rss_before_kib": before["rss_kib"], "rss_after_kib": after["rss_kib"],
        "pids_before": before["pids"], "pids_after": after["pids"],
        "audit_delta": after_audit - before_audit,
        "runtime_stats": runtime_stats,
    }


def summarize(rows):
    groups = {}
    for row in rows:
        key = row["target"], row["workload"], row["concurrency"]
        groups.setdefault(key, []).append(row)
    output = []
    for (target, workload, concurrency), group in groups.items():
        result = {"target": target, "workload": workload, "concurrency": concurrency,
                  "runs": len(group), "errors": sum(row["errors"] for row in group)}
        for field in ["p50_ms", "p95_ms", "p99_ms", "rps", "server_cpu_ms_per_request",
                      "client_cpu_fraction", "rss_after_kib"]:
            values = [row[field] for row in group if row[field] is not None]
            result[field] = {"median": statistics.median(values), "min": min(values), "max": max(values)} if values else None
        output.append(result)
    return output


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--targets", type=Path, required=True,
                        help="JSON list with name, url, pid and data_dir for each server")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--requests", type=int, default=5000)
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--workloads", nargs="+", choices=["get", "post"], default=["get", "post"])
    args = parser.parse_args()
    if min(args.rounds, args.requests, args.concurrency) < 1:
        parser.error("rounds, requests and concurrency must be positive")
    targets = json.loads(args.targets.read_text())
    if not targets:
        parser.error("at least one target is required")
    args.output.mkdir(parents=True, exist_ok=True)
    if (args.output / "rounds.jsonl").exists():
        parser.error("output already contains rounds.jsonl; use a new directory")
    metadata = {
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(), "python": platform.python_version(),
        "cpus": os.cpu_count(), "load_average": os.getloadavg(),
        "targets": targets, "rounds": args.rounds, "requests_per_round": args.requests,
        "concurrency": args.concurrency, "workloads": args.workloads,
        "warmup_requests_per_round": 200,
        "latency_scope": "HTTP request through complete response body; JSON verification follows timer",
        "throughput_scope": "Client loop includes body verification; investigate client CPU before ranking throughput",
        "cpu_scope": "ps cumulative process-tree CPU before and after each round; platform timer precision applies",
        "rss_scope": "Sum of process-tree RSS at samples; shared pages may count more than once",
    }
    (args.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    rows = []
    with (args.output / "rounds.jsonl").open("w") as output:
        for run in range(1, args.rounds + 1):
            offset = (run - 1) % len(targets)
            ordered = targets[offset:] + targets[:offset]
            for workload in args.workloads:
                for target in ordered:
                    row = timed_round(target, workload, args.requests, args.concurrency, run)
                    rows.append(row)
                    output.write(json.dumps(row) + "\n")
                    output.flush()
                    print(json.dumps({key: row[key] for key in ["target", "workload", "run", "p50_ms", "errors"]}), flush=True)
                    (args.output / "summary.json").write_text(json.dumps(summarize(rows), indent=2) + "\n")
                    if row["errors"]:
                        raise SystemExit("A measured round failed verification; inspect rounds.jsonl")
    metadata["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    metadata["load_average_after"] = os.getloadavg()
    (args.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()
