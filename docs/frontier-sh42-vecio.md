# Frontier SH42 — preadv/pwritev/sync: closing the vectored-I/O + durability data-plane gap

## What this cycle proves
A real raw-SQLite session datastore does its paged I/O through **preadv(69)** /
**pwritev(70)** (batched vectored positional read/write of db+shm pages) and its
durability barrier through **sync(81)** (issue under `PRAGMA synchronous=FULL`
before reporting a commit durable). All three were previously unhandled in
`guest_svc` → **-ENOSYS**, so a store doing vectored paged I/O failed and an
unhandled sync made every commit look non-durable. This was the one remaining
syscall hole in the data-plane (SH38 openat/mkdirat/unlinkat/renameat/faccessat/
newfstatat, SH40 statx/statfs/truncate/chdir/linkat/symlinkat/utimensat/readlinkat,
SH40b flock/fallocate all already closed).

## Changes (commit 7351654)
- `crates/arm64jit/src/jit.rs` guest_svc:
  - `69 => SYS_preadv(fd, iov, iovcnt, a[3], a[4])` — aarch64 passes the loff_t
    offset as TWO syscall words (lo, hi); that is exactly x86-64's `__NR3264`
    preadv form, so a raw 5-arg forward is ABI-correct.
  - `70 => SYS_pwritev(fd, iov, iovcnt, a[3], a[4])` — same shape.
  - `81 => { libc::sync(); 0 as c_long }` — libc::sync returns `()`, so the arm
    must not `as c_long` the unit value (E0605 made that explicit on first build).
  - `struct iovec` is byte-identical ({base: *mut, len: usize}) on aarch64 and
    x86-64, so no layout conversion is needed — a raw forward writes the guest's
    iovec array in place.

## Regression (crates/arm64jit/tests/fsmap_persist.rs)
`fsmap_preadv_pwritev_sync_support_sqlite_durability_path` drives the REAL
`guest_svc` ABI under a configured store root (no APK):
1. `openat(O_CREAT|O_RDWR)` a `/data/user/0/com.roblox.client/databases/session.db`.
2. `pwritev(fd, [page0, page1], 2, 0, 0)` — two pages at distinct offsets; asserts
   the exact combined byte count.
3. `sync(81)` → asserts 0 (an old build returned -ENOSYS).
4. close + reopen a fresh fd (no in-process cache).
5. `preadv(fd2, [buf], 1, pos=16, 0)` reads page1 back; asserts byte-exact content.

This completes the SQLite lifecycle data-plane:
```
create → write → statx-exists → flock → fallocate → truncate → preadv/pwritev → sync → readlink → read
```

## Verification
- `cargo test --workspace` → **488/0** (was 487/0; +1 regression).
- `cargo build --workspace` clean (only pre-existing E0133/warning noise).
- Real boot re-verified through the modified dispatch
  (run-log `/home/hermes-worker/runs/sh42-boot-reverify.txt`): exit 124 stable,
  real indexed glDrawElements triangle centroid RGBA(255,0,0,255), textured quad
  BL=RED/BR=GREEN/TR=WHITE/TL=BLUE exact texels, 3 fresh quad-loop frames,
  swap Ok(0x1). Baselines unchanged (--jni exit 0; stable idle exit 124).

## Why this unblocks objective 2b
The client "remembers sign-in" is objective 2b. The session/login datastore is
SQLite-backed; its full write-commit path now funnels through the persistent
store with correct semantic transparency (vectored paged I/O + a real durability
barrier), so a guest session that reaches its store will persist correctly across
a restart rather than -ENOSYS-ing in the middle of a commit.

## Next
Data-plane is now complete for the raw SQLite store. The standing structural wall
(SH14, re-confirmed SH41) is unchanged: the engine's own main-loop producer never
enqueues a render-task type (the w4=4 cap), so frames are harness-driven. Two
directions: (a) drive the confirmed-live deque-maintenance globals
(0x1068262e8/300/308) via --deque-node-live and see whether a maintenance
dispatch advances the session past idle; or (b) harden the JNI/network surface a
logged-in session touches — the socket/TLS (connect/sendto/recvfrom/setsockopt
all forward, but DNS/TLS handset state and the Android-framework JNI
Call*Method/framework-shim paths are the next touch-points once a session
actually reads/writes its store).