#!/usr/bin/env python3
"""Extract the runtime strings passed by the ignored Kernel tests to IPC v2."""
import re
import sys
from pathlib import Path

kernel = Path(__file__).resolve().parents[2] / 'kernel/src'
out = Path(sys.argv[1])
http = (kernel / 'functions/http_tests.rs').read_text()
runtime = (kernel / 'functions/runtime.rs').read_text()
transfer = (kernel / 'app_folders/package.rs').read_text()
bundles = {}
for name in ('HANDLER', 'STREAM_HANDLER'):
    bundles[name] = re.search(rf'const {name}: &str = r#"(.*?)"#;', http, re.S)[1]
bundles['HANDLER_B'] = bundles['HANDLER'].replace("version:'A'", "version:'B'")
bundles['LIFECYCLE'] = re.search(r'path.join\("example.mjs"\),\s*r#"(.*?)"#,', runtime, re.S)[1]
bundles['TRANSFER'] = re.search(r'let bundle = if worker.is_some\(\) \{\s*r#"(.*?)"#', transfer, re.S)[1]
for name, source in bundles.items():
    (out / (name.lower() + '.mjs')).write_text(source)
