"""Observe the pilot process, its staging tree and the disposable S3 server."""
from collections import Counter, defaultdict
from contextlib import contextmanager
import ctypes
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
from threading import Condition, Event, Thread
import time
from urllib.parse import parse_qs
import uuid


def tree_bytes(root):
    total = 0
    for base, _, names in os.walk(root):
        for name in names:
            try:
                total += os.stat(os.path.join(base, name)).st_size
            except FileNotFoundError:
                pass
    return total


def fd_counter():
    lib = ctypes.CDLL("/usr/lib/libproc.dylib")
    # Darwin's PROC_PIDLISTFDS returns an array of eight-byte proc_fdinfo records.
    def count(pid):
        capacity = lib.proc_pidinfo(pid, 1, 0, None, 0)
        if capacity <= 0:
            return 0
        buf = ctypes.create_string_buffer(capacity + 1024)
        return lib.proc_pidinfo(pid, 1, 0, buf, len(buf)) // 8
    return count


@contextmanager
def observed_server(binary, env, request, result):
    root = Path(env["KONAE_DATA_DIR"])
    wals = [db.with_suffix(".sqlite-wal") for db in root.rglob("*.sqlite")]
    fds = fd_counter()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    stop = Event()
    result.update(samples=0, walBytes=0, stagingBytes=0, descriptors=0, cuts=[])
    with tempfile.TemporaryFile() as log:
        process = subprocess.Popen([binary], env=dict(env, KONAE_KERNEL_PORT=str(port)), stdout=log, stderr=log)

        def observe():
            while not stop.is_set():
                wal = 0
                for path in wals:
                    try:
                        wal += path.stat().st_size
                    except FileNotFoundError:
                        pass
                result["walBytes"] = max(result["walBytes"], wal)
                result["stagingBytes"] = max(result["stagingBytes"], tree_bytes(root / ".copy-staging"))
                result["descriptors"] = max(result["descriptors"], fds(process.pid))
                result["samples"] += 1
                stop.wait(.1)

        watcher = Thread(target=observe)
        watcher.start()
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read().decode()[-1200:])
                try:
                    if request(port, "/_konae/health")[0]["ok"]:
                        break
                except OSError:
                    pass
                time.sleep(.025)
            else:
                raise TimeoutError("Pilot service did not start")
            yield port
        finally:
            stop.set()
            watcher.join()
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=15)
            log.seek(0)
            pattern = rb"admitted writers after (\d+) ms \(drain (\d+) ms, (\d+) databases, (\d+) open readers"
            result["cuts"] = [dict(zip(("pauseMs", "drainMs", "images", "readers"), map(int, match)))
                              for match in re.findall(pattern, log.read())]


class Traffic:
    """Count MinIO trace records at the server, grouped by phase and object prefix."""
    def __init__(self, mc):
        self.mc = mc
        self.phase = "setup"
        self.stats = defaultdict(lambda: defaultdict(Counter))
        self.marker = "pilot-marker-" + uuid.uuid4().hex
        self.seen = 0
        self.condition = Condition()

    def consume(self):
        for line in self.proc.stdout:
            row = json.loads(line)
            key = row["path"].removeprefix("/konae-test/") or parse_qs(row["query"]).get("prefix", [""])[0]
            with self.condition:
                if key == self.marker:
                    self.seen += 1
                    self.condition.notify_all()
                    continue
                group = key.split("/", 1)[0]
                target = {"files": "source", "copies": "s3", "copies-v3": "s3-v3"}.get(group, "bucket")
                counts = self.stats[self.phase][target]
                counts[row["api"]] += 1
                counts["receivedBytes"] += row["callStats"]["rx"]
                counts["sentBytes"] += row["callStats"]["tx"]

    def boundary(self, phase):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            with self.condition:
                previous = self.seen
            subprocess.run([self.mc, "stat", "lab/konae-test/" + self.marker], capture_output=True, timeout=5)
            with self.condition:
                if self.condition.wait_for(lambda: self.seen > previous, .4):
                    self.phase = phase
                    return
        raise TimeoutError("MinIO trace missed the phase marker")

    def __enter__(self):
        self.proc = subprocess.Popen([self.mc, "admin", "trace", "--json", "--call", "s3", "lab"],
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.thread = Thread(target=self.consume)
        self.thread.start()
        try:
            self.boundary("setup")
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def __exit__(self, *_):
        self.proc.terminate()
        self.proc.wait(timeout=10)
        self.thread.join(timeout=10)
        self.proc.stdout.close()
