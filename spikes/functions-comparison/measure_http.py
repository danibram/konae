#!/usr/bin/env python3
"""Measure authenticated Kaminando read and write routes on a local Kernel."""
import argparse
from http.client import HTTPConnection
import json
import statistics
import subprocess
import threading
import time

_connections = threading.local()
import uuid


def percentile(values, p):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int((len(ordered) - 1) * p))]


def rss(pid):
    try:
        output = subprocess.check_output(["ps", "-A", "-o", "pid=,ppid=,rss=,command="], text=True)
    except subprocess.CalledProcessError:
        return {"kernel_kib": None, "workers_kib": [], "workers_total_kib": 0}
    kernel = None
    workers = []
    for line in output.splitlines():
        fields = line.strip().split(None, 3)
        if len(fields) < 4:
            continue
        child, parent, size = int(fields[0]), int(fields[1]), int(fields[2])
        command = fields[3]
        if child == pid:
            kernel = size
        elif parent == pid and "konae-functions-runner" in command:
            workers.append(size)
    return {"kernel_kib": kernel, "workers_kib": workers, "workers_total_kib": sum(workers)}


def request(port, host, cookie, method, path, body=None, expected_status=200):
    headers = {"Host": f"{host}:{port}", "Cookie": cookie, "Accept": "application/json"}
    payload = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        headers["Idempotency-Key"] = uuid.uuid4().hex
        payload = json.dumps(body, separators=(",", ":"))
    connection = getattr(_connections, "connection", None)
    if connection is None or connection.sock is None:
        connection = HTTPConnection("127.0.0.1", port, timeout=30)
        _connections.connection = connection
    started = time.perf_counter_ns()
    connection.request(method, path, payload, headers)
    response = connection.getresponse()
    raw = response.read()
    elapsed = (time.perf_counter_ns() - started) / 1_000_000
    status = response.status
    if status != expected_status:
        raise RuntimeError(f"{method} {path} returned {status}: {raw[:400]!r}")
    return elapsed, raw


def summary(times):
    return {"count": len(times), "p50_ms": percentile(times, .50), "p95_ms": percentile(times, .95), "mean_ms": statistics.fmean(times)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--host", default="kamynando2.localhost")
    parser.add_argument("--cookie", required=True)
    parser.add_argument("--kernel-pid", type=int, required=True)
    parser.add_argument("--runs", type=int, default=200)
    parser.add_argument("--runner", choices=["deno", "wasm"], required=True)
    parser.add_argument("--spacing-ms", type=float, default=110)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    cookie_fields = open(args.cookie, encoding="utf-8").read().splitlines()[-1].split("\t")
    cookie = cookie_fields[-2] + "=" + cookie_fields[-1]

    cold = {}
    for name, path in {
        "profiles": "/api/clinic/profile",
        "dashboard": "/api/clinic/dashboard/recent",
        "patients": "/api/clinic/patients?archived=0&limit=1",
        "files": "/api/clinic/files/trash/mine",
    }.items():
        elapsed, _ = request(args.port, args.host, cookie, "GET", path)
        cold[name] = elapsed

    _, directory = request(args.port, args.host, cookie, "GET", "/api/clinic/patients?archived=0&limit=1")
    patients = json.loads(directory)["patients"]
    if patients:
        patient_id = patients[0]["id"]
    else:
        patient = {"name": "Synthetic benchmark patient", "birthdate": "2018-02-03", "contactName1": "Synthetic", "contactEmail1": "", "contactPhone1": "", "contactName2": "", "contactEmail2": "", "contactPhone2": "", "defaultContact": "1", "educationCenter": "Synthetic", "address": "", "tutor": ""}
        _, created = request(args.port, args.host, cookie, "POST", "/api/clinic/patients", {"patient": patient, "siblings": []}, 201)
        patient_id = json.loads(created)["id"]
    read_path = f"/api/clinic/patients/{patient_id}"
    _, read_body = request(args.port, args.host, cookie, "GET", read_path)
    assert json.loads(read_body)["id"] == patient_id

    resting = rss(args.kernel_pid)
    stop = threading.Event()
    sampled = []

    def sample():
        while not stop.is_set():
            sampled.append(rss(args.kernel_pid))
            time.sleep(.025)

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    read_times = []
    write_times = []

    def read_once(_):
        return request(args.port, args.host, cookie, "GET", read_path)[0]

    def write_once(index):
        return request(args.port, args.host, cookie, "POST", f"/api/clinic/patients/{patient_id}/archive", {"archived": bool(index % 2)})[0]

    for index in range(args.runs):
        read_times.append(read_once(index))
        time.sleep(args.spacing_ms / 1000)
    for index in range(args.runs):
        write_times.append(write_once(index))
        time.sleep(args.spacing_ms / 1000)
    stop.set()
    sampler.join()
    peak = max(sampled, key=lambda row: row["workers_total_kib"], default=resting)
    result = {
        "runner": args.runner,
        "runs_per_route": args.runs,
        "cold_first_request_ms": cold,
        "read_route": {"path": read_path, **summary(read_times)},
        "write_route": {"path": f"/api/clinic/patients/{patient_id}/archive", **summary(write_times)},
        "rss_rest": resting,
        "rss_peak_sampled_during_load": peak,
        "rss_samples": len(sampled),
        "load_concurrency": 1,
        "spacing_ms": args.spacing_ms,
    }
    with open(args.output, "w", encoding="utf-8") as out:
        json.dump(result, out, indent=2)
        out.write("\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
