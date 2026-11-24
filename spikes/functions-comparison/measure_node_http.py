#!/usr/bin/env python3
"""Measure the Node compatibility fixture's cold GETs and warm read route."""
import argparse
from http.client import HTTPConnection
import json
import statistics
import subprocess
import threading
import time


def percentile(values, p):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int((len(ordered) - 1) * p))]


def rss(pid):
    output = subprocess.check_output(["ps", "-A", "-o", "pid=,ppid=,rss=,command="], text=True)
    kernel = None
    workers = []
    for line in output.splitlines():
        fields = line.strip().split(None, 3)
        if len(fields) < 4:
            continue
        child, parent, size = int(fields[0]), int(fields[1]), int(fields[2])
        if child == pid:
            kernel = size
        elif parent == pid and "konae-functions-runner" in fields[3]:
            workers.append(size)
    return {"kernel_kib": kernel, "workers_kib": workers, "workers_total_kib": sum(workers)}


def request(port, host, path):
    connection = HTTPConnection("127.0.0.1", port, timeout=30)
    started = time.perf_counter_ns()
    connection.request("GET", path, headers={"Host": f"{host}:{port}", "Accept": "application/json"})
    response = connection.getresponse()
    body = response.read()
    elapsed = (time.perf_counter_ns() - started) / 1_000_000
    connection.close()
    if response.status != 200:
        raise RuntimeError(f"GET {path} returned {response.status}: {body[:400]!r}")
    return elapsed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--host", default="nodecompat.localhost")
    parser.add_argument("--kernel-pid", type=int, required=True)
    parser.add_argument("--functions", required=True)
    parser.add_argument("--route", default="/api/zod")
    parser.add_argument("--runs", type=int, default=200)
    parser.add_argument("--spacing-ms", type=float, default=500)
    parser.add_argument("--cooldown-seconds", type=float, default=11)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    manifest = json.load(open(args.functions, encoding="utf-8"))
    cold = {}
    for name, declaration in manifest["functions"].items():
        path = declaration["mount"]
        cold[name] = request(args.port, args.host, path)
    time.sleep(args.cooldown_seconds)
    resting = rss(args.kernel_pid)
    stop = threading.Event()
    samples = []

    def sample():
        while not stop.is_set():
            samples.append(rss(args.kernel_pid))
            time.sleep(0.025)

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    times = []
    for _ in range(args.runs):
        times.append(request(args.port, args.host, args.route))
        time.sleep(args.spacing_ms / 1000)
    stop.set()
    sampler.join()
    peak = max(samples, key=lambda row: row["workers_total_kib"], default=resting)
    result = {
        "cold_first_request_ms": cold,
        "read_route": {
            "path": args.route,
            "count": len(times),
            "p50_ms": percentile(times, .50),
            "p95_ms": percentile(times, .95),
            "mean_ms": statistics.fmean(times),
        },
        "rss_rest": resting,
        "rss_peak_sampled_during_load": peak,
        "rss_samples": len(samples),
        "load_concurrency": 1,
        "spacing_ms": args.spacing_ms,
        "write_route": None,
    }
    with open(args.output, "w", encoding="utf-8") as out:
        json.dump(result, out, indent=2)
        out.write("\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
