import json
import subprocess
import sys

runner = sys.argv[1]
env = {
    "KONAE_INTERNAL_FUNCTIONS_SANDBOX": "linux-v1",
    "KONAE_INTERNAL_FUNCTIONS_UID": "65534",
    "KONAE_INTERNAL_FUNCTIONS_GID": "65534",
}
process = subprocess.Popen(
    [runner, "--function-worker"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    cwd="/",
    env=env,
)
stdout, stderr = process.communicate(b"KFS1", timeout=10)
if len(stdout) < 4:
    raise SystemExit(f"worker returned no framed refusal: {stderr.decode(errors='replace')}")
length = int.from_bytes(stdout[:4], "big")
message = json.loads(stdout[4 : 4 + length])
assert message == {"kind": "Failure", "capability": None, "code": "sandbox_unavailable"}, message
assert process.returncode != 0, process.returncode
print(f"worker_refusal={message['code']} process_exit={process.returncode}")
print(stderr.decode(errors="replace").strip())
