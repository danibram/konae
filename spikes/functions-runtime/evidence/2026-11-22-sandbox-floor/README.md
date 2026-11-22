# Sandbox floor evidence

Docker Desktop ran every container on VM kernel `7.0.14-linuxkit`. The amd64 containers reported `x86_64`; arm64 containers reported `aarch64`. Debian 12 and Ubuntu 24.04 images would use the same VM kernel, so this host could not provide a kernel below the new floor.

The executor requires Landlock ABI 2 because ABI 2 adds `LANDLOCK_ACCESS_FS_REFER`, which lets the empty ruleset deny link and rename operations. ABI 2 first appeared in Linux 5.19. The seccomp filter uses `SECCOMP_RET_KILL_PROCESS`, available since Linux 4.14, so seccomp does not raise the floor. The [kernel Landlock documentation](https://docs.kernel.org/userspace-api/landlock.html) describes ABI-based right masking. The [seccomp manual](https://man7.org/linux/man-pages/man2/seccomp.2.html) dates the process-kill action.

## Results

- Executor tests passed 11/11 on amd64 and arm64.
- Runner tests passed 11/11 on arm64. On amd64, 10 passed and `v8_watchdog_reuses_a_synchronized_worker_after_a_loop` failed. The failure reproduced when run alone in Docker Desktop's x86_64 emulation. The VM uses Rosetta for amd64 containers.
- The ignored 100 MB stream test and published bundle export/import test passed on arm64 with the delegated cgroup. The ignored HTTP authentication test reached its final oversized-body case and failed because it returned HTTP 504 where the test expected HTTP 413. The test's 3.5 second deadline may be too short on this VM. Its temporary published app remained available and supplied the artifact fixture below.
- `cargo test -p kernel -p konae` passed. The Kernel library ran 624 tests with 10 ignored. The Konae suite ran 92 tests. Inventory checks passed 5 tests, and two subprocess checks passed.
- `bun run build` passed. The full Bun suite ran 1,733 tests across 138 files: 1,717 passed, 15 skipped, and one failed test reported an unhandled error because Bun 1.3.11 could not resolve `node:sqlite` in the baseline test.
- The ARM64 `Dockerfile.kernel` image served an authenticated SQLite query with the production entrypoint and `KONAE_FUNCTIONS_CGROUP` set. The Kernel process had `CapEff=0xc0`. The request returned HTTP 200 and row quantity 7.
- A second image run denied `landlock_create_ruleset` with a disposable seccomp profile. The authenticated mutation returned HTTP 503 `sandbox_unavailable`, left the SQLite row at quantity 7, and left no worker cgroup.

The image runs used kernel `7.0.14-linuxkit`, above the Linux 5.19 floor. The seccomp profile forced the Landlock refusal path; it did not replace an older kernel run.
