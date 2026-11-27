# ThirtyFTP vendor record

This directory contains the published ThirtyFTP 0.1.2 crate from crates.io. Its `src/` files are byte-identical to the published archive. The manifest changes only `rust-version`, from `1.96` to `1.91`; the source compiles with Rust 1.91.

The published archive SHA-256 is `2b178a77469ae5fa8d78b252b80b6352361e158e50d18a59dc28f048ed7ed4e5`.

Remove this copy when an upstream ThirtyFTP release declares Rust 1.91 or earlier. Change `kernel-rs/kernel/Cargo.toml` back to that exact crates.io release, update `Cargo.lock` with ordinary Cargo commands on Rust 1.91, compare the replacement `src/` tree with the vendored tree, then remove `kernel-rs/vendor/thirtyftp` and this record. Do not keep this manifest edit after upstream supports the shared toolchain.
