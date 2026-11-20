import json
import os
import struct
import subprocess
import sys


def send(process, message):
    payload = json.dumps(message, separators=(",", ":")).encode()
    process.stdin.write(struct.pack(">I", len(payload)) + payload)
    process.stdin.flush()


def receive(process):
    prefix = process.stdout.read(4)
    if len(prefix) != 4:
        raise RuntimeError("runner closed stdout before a complete frame")
    size = struct.unpack(">I", prefix)[0]
    payload = process.stdout.read(size)
    if len(payload) != size:
        raise RuntimeError("runner returned a truncated frame")
    return json.loads(payload)


assert not os.path.exists("/repo")
assert not os.path.exists("/root/.cargo/registry/src")
assert not os.path.exists("/tmp/cargo")
process = subprocess.Popen(
    [sys.argv[1], "--function-worker"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
)
try:
    generation = "packaged-clean-smoke"
    send(
        process,
        {
            "kind": "Init",
            "abi": "konae-functions/1",
            "function": "smoke",
            "source": "export function handle(request) { return new Response(JSON.stringify({ url: request.url, fetch: typeof fetch })); }",
            "generation": generation,
            "limits": {
                "heap_bytes": 67108864,
                "response_bytes": 2097152,
                "rss_bytes": 268435456,
                "operation_limit": 64,
            },
        },
    )
    ready = receive(process)
    assert ready == {"kind": "Ready", "abi": "konae-functions/1", "generation": generation}, ready
    capability = {"generation": generation, "request": 1, "token": "smoke-token"}
    send(
        process,
        {
            "kind": "Invoke",
            "capability": capability,
            "input": {
                "url": "http://app.local/smoke",
                "method": "GET",
                "headers": [],
                "body": [],
                "actor": "",
            },
        },
    )
    reply = receive(process)
    assert reply["kind"] == "Reply", reply
    assert reply["output"]["status"] == 200, reply
    body = json.loads(reply["output"]["body"])
    assert body == {"url": "http://app.local/smoke", "fetch": "undefined"}, body
    send(process, {"kind": "Shutdown", "grace_ms": 100})
    closed = receive(process)
    assert closed["kind"] == "ShutdownComplete", closed
    code = process.wait(timeout=10)
    assert code == 0, process.stderr.read().decode(errors="replace")
    print(json.dumps({"ready": ready, "reply": reply, "shutdown": closed, "exit": code}))
except BaseException:
    process.kill()
    process.wait()
    raise
