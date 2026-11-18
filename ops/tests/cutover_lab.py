"""Sign a synthetic receipt with separate disposable keys for each role."""
import hashlib
import json
import subprocess


def sign_receipt(root, receipt):
    path = root / "restricted.json"
    signers = receipt["signers"] = {}
    keys = {}
    for role in ("operations", "maintenance"):
        private = root / (role + ".pem")
        public = root / (role + ".pub")
        subprocess.run(["openssl", "ecparam", "-genkey", "-name", "prime256v1", "-noout", "-out", private], check=True)
        subprocess.run(["openssl", "pkey", "-in", private, "-pubout", "-out", public], check=True)
        der = subprocess.check_output(["openssl", "pkey", "-pubin", "-in", public, "-outform", "DER"])
        signers[role] = dict(authority="lab", keySha256=hashlib.sha256(der).hexdigest())
        keys[role] = public
    path.write_text(json.dumps(receipt, sort_keys=True) + "\n")
    for role in keys:
        subprocess.run(["openssl", "dgst", "-sha256", "-sign", root / (role + ".pem"), "-out",
                        str(path) + "." + role + ".sig", path], check=True)
    return path, keys
