# SH41 — faccessat(48) arg-order fix + runtime deque-maintenance-globals correction

## One-line result
Fixed a real data-plane arg-order bug: the guest_svc handler for aarch64
`faccessat(48)` passed the **dirfd** (e.g. `AT_FDCWD = -100`) as the pathname
char-pointer and the real pathname pointer as the mode, so any guest
datastore-accessibility probe on a `/data` path read garbage against the
host root. Now `(dirfd, pathname, mode)` is routed correctly (dirfd=a[0],
pathname=a[1] remapped, mode=a[2]) and the full product boot+render still
reproduces (exit 124, real triangle + textured quad, zero unhandled syscalls).
Workspace 487/0 (was 486/0). Commit xxxxx.

## The bug (same class as SH40's readlinkat fix)
On aarch64, raw syscall 48 is `faccessat(dirfd, pathname, mode)` —
x0=dirfd, x1=pathname, x2=mode. The old handler was:

```rust
48 => {
    let (p, _keep) = mappath(a[0] as *const c_char, false);       // a[0] is DIRFD
    unsafe { libc::faccessat(libc::AT_FDCWD, p, a[1] as c_int, 0) } // a[1] is PATHNAME ptr
}
```

Two faults: it passed the dirfd integer (often AT_FDCWD = -100, an invalid
address) as the pathname C-string to `remap_path`/libc, and it passed the real
pathname pointer value truncated to `c_int` as the mode. A real Android
accessibility check — `faccessat(AT_FDCWD, "/data/.../session.db", R_OK)` — the
exact primitive bionic/Java use to answer "is my datastore there?" before
writing it, therefore resolved a garbage path (likely -EFAULT / wrong result)
instead of reaching the persistent store.

## The fix
```rust
48 => {
    let (p, _keep) = mappath(a[1] as *const c_char, false);
    unsafe { libc::faccessat(a[0] as c_int, p, a[2] as c_int, 0) as c_long }
}
```

## Hermetic regression (tests/fsmap_persist.rs)
`fsmap_faccessat_uses_true_pathname_and_remaps_into_store` — through the real
guest_svc ABI under a configured Android root:
1. openat+write a real shared_prefs `prefs.xml` into the store;
2. `faccessat(AT_FDCWD, path, R_OK)` and `(W_OK)` both return 0 (proves the true
   pathname is read AND resolved through the store);
3. `faccessat(AT_FDCWD, ghost, R_OK)` on a missing store path returns -ENOENT
   (proves store-index resolution, not a host-root fallthrough);
4. a real dirfd: `openat(O_DIRECTORY)` on the store's shared_prefs dir, then
   `faccessat(dirfd, "prefs.xml", R_OK)` with a RELATIVE pathname returns 0 —
   proving dirfd is honored (not hardcoded AT_FDCWD) and relative paths resolve
   against the store fd.

Under the old handler, case 2/3 read the dirfd as the path (garbage) and case 4
would have failed the relative probe.

## Corrects a stale documented premise (deque-maintenance globals ARE populated)
While re-verifying the standing producer wall (SH14/SH39b: "the type-4
maintenance handler blrs through framework-owned BSS globals 0x1068262e8/300/308
— all statically 0 on this box"), a live `JIT_FRAMEWORK_DUMP` under the stable
boot (exit 124) shows those three globals hold **real .text addresses**:

```
[elfjit:fw] deque-fwd 0x1068262e8=0x10620db24 0x106826300=0x102176bfc 0x106826308=0x1022199e0
```

(whole boot + full render recipe reproduces the same values). So SH39b/SH14's
"statically 0, not host-drivable" premise is **wrong at runtime** — the
maintenance handler already has live forward edges. That does NOT unlock the
engine's self-driven render: the three targets are thin bionic/atrace-ish
upkeep functions (each reads TLS via `adrp 0x67d1000[#1776]`, matching the
Log/atrace maintenance pattern), not render/session producers, and the drain's
pop-loop still hardcodes `w4=4` (maintenance type) when dispatching a node
(0x2856ffc). So the structural wall is unchanged (the engine never enqueues a
render-task type), but future cycles should stop treating those globals as an
impossible NULL — a seeded node's maintenance dispatch DOES execute real engine
code (PR short: this re-opens `--deque-node-live` as a reachable path, matching
SH13's live-drainer result).

## Verification
- `cargo test --workspace` : 487 passed / 0 failed (was 486/0; +1 regression).
- `cargo build --workspace` : clean (only pre-existing non_snake_case/dead_code
  warnings; none in the edited lines).
- Full real-boot render (runs/sh41-boot-render-verify.txt): exit 124 stable,
  real indexed glDrawElements triangle (centroid RGBA(255,0,0,255)) + textured
  quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE exact texels) + 4 fresh quad-loop
  frames, swap Ok(0x1), zero ENOSYS/unhandled syscalls / json-Writer terminate.

## Next (closest unblocked)
Data-plane path coverage for the real datastore is now complete across
openat/mkdirat/unlinkat/renameat/faccessat/fstatat/newfstatat/statx/statfs/
truncate/chdir/linkat/symlinkat/readlinkat/utimensat + flock/fallocate — the
full SQLite session-datastore lifecycle. The engine's own main-loop producer
still never enqueues a render-task type (the `w4=4` structural cap), so frames
remain harness-driven on the live engine context. Next: either (a) use the
now-confirmed populated maintenance globals to drive `--deque-node-live` toward
real engine framework code (SH13's live-drain path) to see if a maintenance
dispatch advances the session, or (b) continue hardening the JNI/network surface
the client touches as a real session reads/writes its now-persistent store.