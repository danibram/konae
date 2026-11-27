#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
: "${WASI_SDK:?Set WASI_SDK to the wasi-sdk 30 installation}"
mkdir -p guest/out
if [ ! -d guest/out/quickjs-0.18.0 ]; then
    curl -fsSL https://codeload.github.com/quickjs-ng/quickjs/tar.gz/refs/tags/v0.18.0 -o guest/out/quickjs.tar.gz
    (cd guest/out && printf '%s\n' '28bcd89aa3683fd2a946537e5ce3f11cc46b5cbf3383c7561aaef3df5ff9f1dd  quickjs.tar.gz' | shasum -a 256 -c)
    tar -xzf guest/out/quickjs.tar.gz -C guest/out
fi
qjs=guest/out/quickjs-0.18.0
"$WASI_SDK/bin/clang" -O3 -DNDEBUG -mexec-model=reactor -Wl,-z,stack-size=524288 \
    -I"$qjs" guest/bridge.c "$qjs/quickjs.c" "$qjs/dtoa.c" "$qjs/libregexp.c" "$qjs/libunicode.c" \
    -o guest/out/quickjs.wasm
./node_modules/.bin/esbuild guest/bootstrap.js --bundle --minify --platform=neutral --main-fields=main,module --target=es2022 --format=iife --global-name=__konae --outfile=guest/out/bootstrap.js
