#!/usr/bin/env python3
"""Measure recorder cost on one warm Kernel and deno_core worker."""

import argparse
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import re
import shutil
import socket
import sqlite3
import subprocess
import threading
import tempfile
import time

REPO = Path(__file__).resolve().parents[2]
PAYLOAD = b'{"sum":10}'


def write_json(path, value):
    path.write_text(json.dumps(value) + "\n")


def prepare(root):
    cli = ["bun", str(REPO / "scripts/konae.ts")]
    subprocess.run(cli + ["init", "--dir", str(root), "--name", "Tracing probe"], check=True)
    settings = root / "konae.json"
    config = json.loads(settings.read_text())
    config["visibility"] = "public"
    config["modules"] = {"functions": {"on": True}, "actions": {"on": True}}
    write_json(settings, config)
    capsule = root / "app"
    shutil.rmtree(capsule)
    (capsule / "functions").mkdir(parents=True)
    (capsule / "index.html").write_text('<main>Tracing probe</main><script type="module" src="./app.js"></script>\n')
    (capsule / "app.js").write_text("export {};\n")
    (capsule / "schema.sql").write_text("CREATE TABLE IF NOT EXISTS sample (n INTEGER PRIMARY KEY);\n")
    write_json(capsule / "actions.json", {"version": 1, "actions": {
        "probe.read": {"kind": "query", "access": "anonymous", "bindings": [],
                       "statement": "SELECT n FROM sample LIMIT 1"},
    }})
    write_json(capsule / "functions.json", {"version": 1, "functions": {
        "probe": {"entry": "functions/probe.js", "mount": "/api/probe",
                  "methods": ["GET"], "actions": ["probe.read"], "access": "anonymous"},
    }})
    (capsule / "functions/probe.js").write_text("""export async function handle(request, context) {
  let sum = 0;
  for (let call = 0; call < 10; call++) {
    const rows = await context.actions.call("probe.read", {});
    if (rows.length) sum++;
  }
  return Response.json({ sum });
}
""")
    subprocess.run(cli + ["build", "--dir", str(root)], check=True)
    return config["id"]


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


class Collector(ThreadingHTTPServer):
    def __init__(self, stalled):
        self.stalled = stalled
        self.received = 0
        super().__init__(("127.0.0.1", 0), CollectorHandler)


class CollectorHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        payload = json.loads(self.rfile.read(length))
        count = sum(len(scope["spans"]) for resource in payload["resourceSpans"]
                    for scope in resource["scopeSpans"])
        self.server.received += count
        if self.server.stalled:
            time.sleep(5)
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", "2")
        self.end_headers()
        try:
            self.wfile.write(b"{}")
        except BrokenPipeError:
            pass

    def log_message(self, *_args):
        pass


def wait_for_kernel(server, port):
    until = time.monotonic() + 30
    while server.poll() is None and time.monotonic() < until:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("Kernel failed to start")


def child_kernel_pid(server, kernel):
    children = subprocess.run(
        ["pgrep", "-P", str(server.pid)], check=False, text=True,
        stdout=subprocess.PIPE,
    ).stdout.splitlines()
    for child in children:
        command = subprocess.run(
            ["ps", "-p", child, "-o", "command="], check=True, text=True,
            stdout=subprocess.PIPE,
        ).stdout.strip()
        if str(kernel) in command:
            return child
    raise RuntimeError(f"Could not find Kernel child of konae dev process {server.pid}")


def worker_pid(kernel_pid, runner):
    found = subprocess.run(
        ["pgrep", "-f", "-P", str(kernel_pid), re.escape(str(runner))],
        check=True, text=True, stdout=subprocess.PIPE,
    ).stdout.splitlines()
    if len(found) != 1:
        raise RuntimeError(f"Expected one worker, got {found}")
    return found[0]


def usage_count(path):
    try:
        with sqlite3.connect(path, timeout=1) as connection:
            return connection.execute(
                "SELECT count(*) FROM function_action_latency WHERE action='probe.read'"
            ).fetchone()[0]
    except sqlite3.OperationalError as error:
        if "no such table" in str(error):
            return 0
        raise


def request(connection, host, sampled, sequence, allow_timeout=False):
    headers = {
        "Host": host,
        "X-Forwarded-For": f"198.18.{sequence // 250}.{1 + sequence % 250}",
    }
    started = time.perf_counter_ns()
    connection.request("GET", "/api/probe", headers=headers)
    response = connection.getresponse()
    body = response.read()
    elapsed = time.perf_counter_ns() - started
    traceparent = response.getheader("traceparent")
    if allow_timeout and response.status == 504:
        return elapsed, None
    if allow_timeout and (response.status, body) == (200, b'{"sum":0}'):
        return elapsed, traceparent
    if (response.status, body) != (200, PAYLOAD):
        raise RuntimeError(f"HTTP {response.status} {body!r}")
    flag = "01" if sampled else "00"
    if not re.fullmatch(rf"00-[0-9a-f]{{32}}-[0-9a-f]{{16}}-{flag}", traceparent or ""):
        raise RuntimeError(f"Unexpected traceparent: {traceparent!r}")
    return elapsed, traceparent


def percentile_us(values, p):
    return sorted(values)[math.ceil(len(values) * p / 100) - 1] / 1_000


def run(server, kernel_pid, runner, port, host, usage_path, rounds, sampled, collector, baseline_us,
        profile_seconds, profile_prefix):
    connections = [HTTPConnection("127.0.0.1", port, timeout=5)]
    samples = []
    pid = None
    usage_start = usage_count(usage_path)
    usage_before = usage_start
    profiles = []
    try:
        for sequence in range(40):
            traceparent = None
            for _attempt in range(10):
                _, traceparent = request(connections[0], host, sampled, sequence, allow_timeout=True)
                if traceparent is not None:
                    break
            if traceparent is None:
                raise RuntimeError("Function did not return HTTP 200 during warmup")
            if sequence == 0:
                pid = worker_pid(kernel_pid, runner)
        warm_usage_target = usage_start + 40 * 10
        warm_usage_deadline = time.monotonic() + 10
        stable_count = None
        stable_reads = 0
        while time.monotonic() < warm_usage_deadline:
            current_count = usage_count(usage_path)
            if current_count >= warm_usage_target and current_count == stable_count:
                stable_reads += 1
                if stable_reads >= 3:
                    usage_before = current_count
                    break
            else:
                stable_count = current_count
                stable_reads = 0
            time.sleep(0.05)
        else:
            raise RuntimeError("Usage did not settle after warmup requests")
        with sqlite3.connect(usage_path.parent / "database.sqlite") as database:
            database.execute("INSERT INTO sample VALUES (1)")
        if profile_seconds:
            for label, process_id in (("kernel", kernel_pid), ("runner", pid)):
                output = f"{profile_prefix}.{label}.sample.txt"
                profiles.append((output, subprocess.Popen(
                    ["/usr/bin/sample", str(process_id), str(profile_seconds), "-file", output],
                    stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
                )))
        for sequence in range(rounds):
            elapsed, _ = request(connections[0], host, sampled, 40 + sequence)
            samples.append(elapsed)
        assert worker_pid(kernel_pid, runner) == pid
    finally:
        for connection in connections:
            connection.close()
        for output, profile in profiles:
            _, stderr = profile.communicate(timeout=profile_seconds + 15)
            if profile.returncode:
                raise RuntimeError(f"/usr/bin/sample failed for {output}: {stderr.strip()}")

    p50 = percentile_us(samples, 50)
    expected_usage = usage_before + rounds * 10
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and usage_count(usage_path) < expected_usage:
        time.sleep(0.05)
    usage_after = usage_count(usage_path)
    if usage_after != expected_usage:
        raise RuntimeError(f"Usage rows changed by {usage_after - usage_before}; expected {rounds * 10}")
    result = {"warm_worker_pid": pid, "collector": collector, "requests": len(samples),
              "p50_us": p50, "p95_us": percentile_us(samples, 95),
              "usage_rows": usage_after - usage_before, "warmup_requests": 40}
    if baseline_us is not None:
        budget = max(baseline_us * 0.02, 50)
        result.update(baseline_p50_us=baseline_us, delta_p50_us=p50 - baseline_us,
                      budget_us=budget)
    print(json.dumps(result, indent=2), flush=True)
    if baseline_us is not None and result["delta_p50_us"] > result["budget_us"]:
        raise SystemExit("Loopback p50 exceeded the exporter budget")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kernel", type=lambda path: Path(path).resolve())
    parser.add_argument("runner", type=lambda path: Path(path).resolve())
    parser.add_argument("--rounds", type=int, default=1200, help="warm GET requests to measure")
    parser.add_argument("--collector", choices=("off", "reachable", "stalled"), default="off")
    parser.add_argument("--baseline-us", type=float, help="disabled-export p50 from the baseline run")
    parser.add_argument("--runner-kind", choices=("deno", "wasm"), default="deno")
    parser.add_argument("--preparer", type=lambda path: Path(path).resolve())
    parser.add_argument("--profile-seconds", type=int, default=0)
    parser.add_argument("--profile-prefix", default="/tmp/konae-tracing-otlp-profile")
    args = parser.parse_args()
    if not 1 <= args.rounds <= 5_000:
        parser.error("--rounds must be between 1 and 5000")
    if args.runner_kind == "wasm" and args.preparer is None:
        parser.error("--preparer is required for --runner-kind wasm")
    if args.profile_seconds < 0 or args.profile_seconds > 30:
        parser.error("--profile-seconds must be between 0 and 30")

    with tempfile.TemporaryDirectory(prefix="konae-functions-tracing-otlp-", dir="/tmp") as scratch:
        folder = Path(scratch) / "app"
        deployment = prepare(folder)
        cache = Path(scratch) / "wasm-cache"
        cache.mkdir()
        port = free_port()
        collector = None
        if args.collector != "off":
            collector = Collector(args.collector == "stalled")
            threading.Thread(target=collector.serve_forever, daemon=True).start()
        env = {key: value for key, value in os.environ.items() if not key.startswith("KONAE_")}
        env.update(
            KONAE_KERNEL_BIN=str(args.kernel), KONAE_FUNCTIONS_RUNNER=str(args.runner),
            KONAE_TRUSTED_PROXIES="127.0.0.1",
            KONAE_FUNCTIONS_DEVELOPMENT="1", KONAE_EXECUTION_MODE="live",
            KONAE_MAIL_TRANSPORT="development",
        )
        if args.runner_kind == "wasm":
            env.update(KONAE_FUNCTIONS_RUNNER_KIND="wasm", KONAE_FUNCTIONS_PREPARER=str(args.preparer),
                       KONAE_FUNCTIONS_WASM_CACHE=str(cache))
        if collector is not None:
            env["KONAE_OTLP_TRACES_ENDPOINT"] = f"http://127.0.0.1:{collector.server_port}/v1/traces"
            env["KONAE_OTLP_TRACES_SAMPLE_RATE"] = "1"
        with tempfile.TemporaryFile(mode="w+b") as log:
            dev_command = ["bun", str(REPO / "scripts/konae.ts"), "dev", "--dir", str(folder),
                           "--port", str(port), "--events-port", str(free_port())]
            with subprocess.Popen(dev_command, env=env, stdout=log, stderr=log) as server:
                try:
                    wait_for_kernel(server, port)
                    kernel_pid = child_kernel_pid(server, args.kernel)
                    slug = json.loads((folder / "konae.json").read_text())["slug"]
                    run(server, kernel_pid, args.runner, port, f"{slug}.localhost:{port}",
                        folder / "data" / "usage.sqlite", args.rounds,
                        True,
                        args.collector, args.baseline_us, args.profile_seconds, args.profile_prefix)
                    if collector is not None:
                        print(f"Collector {args.collector} received {collector.received} spans.", flush=True)
                except BaseException:
                    log.seek(0)
                    print(log.read().decode(errors="replace"), flush=True)
                    raise
                finally:
                    server.terminate()
                    try:
                        server.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait()
        if collector is not None:
            collector.shutdown()
            collector.server_close()


if __name__ == "__main__":
    main()
