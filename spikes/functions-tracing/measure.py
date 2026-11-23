#!/usr/bin/env python3
"""Measure recorder cost on one warm Kernel and deno_core worker."""

import argparse
from collections import defaultdict
from http.client import HTTPConnection
import itertools
import json
import math
import os
from pathlib import Path
import random
import re
import shutil
import socket
import sqlite3
import statistics
import subprocess
import tempfile
import time

REPO = Path(__file__).resolve().parents[2]
MODES = ("off", "metrics", "trace")
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
    (capsule / "schema.sql").write_text("CREATE TABLE sample (n INTEGER PRIMARY KEY);\n")
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
    sum += rows[0].n;
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


def wait_for_kernel(server, port):
    until = time.monotonic() + 30
    while server.poll() is None and time.monotonic() < until:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("Kernel failed to start")


def worker_pid(server, runner):
    found = subprocess.run(
        ["pgrep", "-f", "-P", str(server.pid), re.escape(str(runner))],
        check=True, text=True, stdout=subprocess.PIPE,
    ).stdout.splitlines()
    if len(found) != 1:
        raise RuntimeError(f"Expected one worker, got {found}")
    return found[0]


def request(connection, host, mode, sequence):
    headers = {
        "Host": host,
        # The loopback peer is the fixture's configured proxy. Each pair shares a rate-limit key.
        "X-Forwarded-For": f"198.18.{sequence // 250}.{1 + sequence % 250}",
        "x-konae-tracing-probe": mode,
    }
    started = time.perf_counter_ns()
    connection.request("GET", "/api/probe", headers=headers)
    response = connection.getresponse()
    body = response.read()
    elapsed = time.perf_counter_ns() - started
    expected_actions = None if mode == "off" else "10"
    traceparent = response.getheader("traceparent")
    if (response.status, body) != (200, PAYLOAD):
        raise RuntimeError(f"{mode}: HTTP {response.status} {body!r}")
    for header in ("x-konae-probe-actions", "x-konae-probe-sqlite"):
        if response.getheader(header) != expected_actions:
            raise RuntimeError(f"{mode}: unexpected {header}: {response.getheader(header)}")
    if mode == "trace":
        assert re.fullmatch(r"00-[0-9a-f]{32}-[0-9a-f]{16}-01", traceparent or "")
    else:
        assert traceparent is None
    return (int(response.getheader("x-konae-probe-server-ns")), elapsed), traceparent


def percentile(values, p):
    return sorted(values)[math.ceil(len(values) * p / 100) - 1] / 1_000


def run(server, runner, port, host, rounds):
    connection = HTTPConnection("127.0.0.1", port, timeout=5)
    samples = defaultdict(list)
    randomizer = random.Random(20261123)
    pid = None
    try:
        for sequence in range(40):
            mode = MODES[sequence % len(MODES)]
            _, traceparent = request(connection, host, mode, sequence)
            if sequence == 0:
                pid = worker_pid(server, runner)
            if sequence == 2:
                print(f"GET /api/probe -> HTTP 200 {PAYLOAD.decode()} traceparent: {traceparent}",
                      flush=True)
        # Each block contains all six orders. Randomize their order between blocks.
        for block in range(rounds):
            orders = list(itertools.permutations(MODES))
            randomizer.shuffle(orders)
            for group, order in enumerate(orders):
                for mode in order:
                    timing, _ = request(connection, host, mode, 40 + block * 6 + group)
                    samples[mode].append(timing)
        assert worker_pid(server, runner) == pid
    finally:
        connection.close()

    print(f"Warm worker PID {pid}. {len(samples['off'])} requests per mode.")
    result = {}
    for index, clock in enumerate(("kernel", "loopback")):
        durations = {mode: [sample[index] for sample in samples[mode]] for mode in MODES}
        baseline = percentile(durations["off"], 50)
        limit = max(baseline * 0.02, 50)
        result[clock] = {mode: {
            "p50_us": percentile(values, 50),
            "p95_us": percentile(values, 95),
            "delta_p50_us": percentile(values, 50) - baseline,
            "paired_median_delta_us": statistics.median(
                other - off for off, other in zip(durations["off"], values)
            ) / 1_000,
        } for mode, values in durations.items()}
        result[clock]["budget_us"] = limit
    print(json.dumps(result, indent=2), flush=True)
    if any(result["loopback"][mode]["delta_p50_us"] >= result["loopback"]["budget_us"]
           for mode in ("metrics", "trace")):
        raise SystemExit("Loopback p50 exceeded the recorder budget")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kernel", type=lambda path: Path(path).resolve())
    parser.add_argument("runner", type=lambda path: Path(path).resolve())
    parser.add_argument("--rounds", type=int, default=200, help="six requests per mode per round")
    args = parser.parse_args()
    if not 1 <= args.rounds <= 5_000:
        parser.error("--rounds must be between 1 and 5000")

    with tempfile.TemporaryDirectory(prefix="konae-functions-tracing-design-", dir="/tmp") as scratch:
        folder = Path(scratch) / "app"
        deployment = prepare(folder)
        state = Path(scratch) / "state"
        (state / "apps").mkdir(parents=True)
        (state / "apps" / deployment).symlink_to(folder, target_is_directory=True)
        port = free_port()
        env = {key: value for key, value in os.environ.items() if not key.startswith("KONAE_")}
        env.update(
            KONAE_BIND="127.0.0.1", KONAE_DATA_DIR=str(state), KONAE_KERNEL_PORT=str(port),
            KONAE_HOST_SUFFIX="localhost", KONAE_PUBLIC_ORIGIN=f"http://localhost:{port}",
            KONAE_TRUSTED_PROXIES="127.0.0.1", KONAE_FUNCTIONS_RUNNER=str(args.runner),
            KONAE_FUNCTIONS_DEVELOPMENT="1", KONAE_EXECUTION_MODE="test",
            KONAE_MAIL_TRANSPORT="development",
        )
        with tempfile.TemporaryFile(mode="w+b") as log:
            with subprocess.Popen([args.kernel], env=env, stdout=log, stderr=log) as server:
                try:
                    wait_for_kernel(server, port)
                    with sqlite3.connect(folder / "data/database.sqlite") as connection:
                        connection.execute("INSERT INTO sample VALUES (1)")
                    record = json.loads((folder / ".konae/deployment.json").read_text())
                    run(server, args.runner, port, record["hostname"], args.rounds)
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


if __name__ == "__main__":
    main()
