# Frontier SH38 — guest filesystem path remap: the datastore persistence enabler

The JIT's `guest_svc` passes guest file-path pointers *verbatim* to the host
libc (guest vaddr == host addr, no translation). A real Roblox Android session
reads/writes its datastore, shared_prefs, cache and login/session cookies under
Android mount roots `/data/data/com.roblox.client/...`, `/sdcard/...`,
`/storage/emulated/0/...`, `/cache/...`. On the host those absolute paths hit the
host root — ENOENT (does not exist) or EPERM (not writable) — so the client
**cannot persist anything**. That is the FS gap behind the objective "the client
REMEMBERS sign-in like the real app".

## What landed

`crates/arm64jit/src/fsmap.rs` (new): a guest→host path remap. When a root is
configured (`SOBER_ANDROID_ROOT`, or a test setter) an absolute guest path under a
writable Android mount root is rewritten to `{root}/data/...` ·
`{root}/storage/...` · `{root}/sdcard/...` · `{root}/cache/...`; `ensure_parents`
recursively scaffolds the intermediate `/data/user/0/com.roblox.client/...`
chain so O_CREAT / mkdirat on a deep path never hits ENOENT. Input-off (root
unset) → paths pass through unchanged, so the existing boot behavior is
untouched. Relative paths and non-writable/virtual roots (`/system`, `/proc`, ...)
are deliberately NOT remapped.

`crates/arm64jit/src/jit.rs`: the path-taking syscalls now route through the
remap — `openat(56)`, `mkdirat(34)`, `unlinkat(35)`, `renameat(38)`,
`faccessat(48)`, `fstatat(79)`. `read/write/readv/writev` on the returned fd are
unchanged (the fd already references the real host file).

## Proof (hermetic, no APK)

`crates/arm64jit/tests/fsmap_persist.rs` drives `guest_svc` directly through the
real ABI:

- **Persistence across restart**: `openat("/data/user/0/com.roblox.client/
  files/session.dat", O_CREAT|O_RDWR)` → write `ROBLOSECURITY=_abc...` → close,
  then a *fresh* `CpuState` reopens the same guest path read-only and reads the
  exact bytes back. Asserts the real host file also landed on disk under the
  root. The store survives a new "boot".
- **Mapping correctness**: the four writable roots map to
  `root.join(guest_sans_leading_slash)` preserving the full subtree; `/system`,
  `/proc`, and relative paths are untouched.
- **Parent scaffolding**: `mkdirat` on `/data/user/0/com.roblox.client/db` and a
  deep `openat(O_CREAT)` into `.../db/nested/even/deeper/state` both succeed with
  intermediates auto-created.

Workspace 482 passed / 0 failed (was 479). Real boot unchanged (JNI_OnLoad
0x10006, StartApp driven, stable idle main loop).

## Next (closest unblocked)

This closes the *data-plane* persistence gap so a real session's datastore and
login cookie can land on persistent host disk once the engine reaches a session
that reads/writes them. The standing structural frontier is unchanged (SH14,
SH37): the engine's own main-loop producer still never enqueues a render task, so
the engine renders what the harness drives. With the persistence root in place,
the next concrete step toward a remembered, usable session is wiring
`sober-core open-sober play` (or elfjit) to arm a persistent `SOBER_ANDROID_ROOT`
under the runtime's data dir, then re-opening the producer/deque wall so the boot
actually enters a session that exercises the now-persistent store.