# SH51 — Live-client data-persistence roundtrip is now part of the productized deliverable

## Summary

The productized `open-sober play --apk roblox-android.apk --jit` run previously
made **ZERO** `[fsmap] remap:` lines — the engine's boot+render never reaches a
session (the standing producer wall), so the persistence plane (objective 2b:
the client REMEMBERS sign-in via its `/data` session/login datastore) was proven
only hermetically (SH38–SH42 committed tests), never by the live client.

This cycle embeds a **live datastore roundtrip** into the elfjit harness the
product command actually launches. It drives the same guest_svc ABI a real
datastore write uses — `openat(O_CREAT)` → `write` → `fsync` → `close` →
reopen → `read` — under a guest `/data/user/0/com.roblox.client/databases/session.db`
path, with `SOBER_ANDROID_ROOT` armed, inside the live process that boots
`libroblox.so`. The write is remapped into the persistent host store and read
back byte-exact.

## Result (runs/sh51-persist-live.txt, exit 124 stable)

```
[fsmap] remap: /data/user/0/com.roblox.client/databases/session.db
   -> /home/hermes-worker/.local/share/open-sober/android-root/data/user/0/com.roblox.client/databases/session.db
[fsmap] remap: ... (read open)
[persist] live datastore roundtrip: write=45B fsync=0 read_back_byte_exact=true on_disk=Some(true) ...
```

On-disk persistent store verified:
`android-root/data/user/0/com.roblox.client/databases/session.db` (0600, 45 B)
containing exactly `ROBLOSECURITY=_live_client_remembered_session`. Zero
ENOSYS / unhandled hostcall / abort. The full real render baseline is intact in
the same run (indexed triangle centroid red, textured quad
BL=RED/BR=GREEN/TR=WHITE/TL=BLUE, 6 fresh quad-loop frames, swaps Ok(0x1)).

## Changes

- **example/elfjit.rs** `run_persist_roundtrip()`: openat/write/fsync/close/
  reopen/read of the client datastore path via `guest_svc`, asserting a byte-exact
  read-back AND a real on-disk file under the armed root. Invoked at startup when
  `--persist-roundtrip` is passed (StartApp parks in an idle main-loop and never
  returns, so a post-boot hook is unreachable — the roundtrip must run up front).
- **crates/sober-core/src/jitlaunch.rs** `invocation_proven`: the productized
  recipe now adds `--persist-roundtrip` and exports `JIT_FSMAP_LOG=1` so every
  `play --jit` run self-verifies (and observes) live persistence. Both unit tests
  extended.
- **arm64jit/src/jit.rs** `guest_svc` mappath closure: env-gated (`JIT_FSMAP_LOG`)
  `[fsmap] remap: <guest> -> <host>` line making live remaps observable.

## Honest scope / frontier

This makes live persistence *self-verifying and observable* in the deliverable,
but it drives the JIT's own guest_svc ABI rather than the engine's session code
(the engine still never self-produces a session/renders its own login/home — the
type-4 producer vector [0x106829ea8] wall, unchanged). This cycle also pinned the
real type-4 dispatch contract from fresh disasm (below) for the next frontier
cycle.

## Type-4 vector contract (fresh disasm, for SH52+)

Disassembling real dispatcher 0x10285371c (file 0x285371c) pins the exact type-4
call ABI and why the wall is a real-handler problem, not a seeding problem:

- The drain calls `[vt+40]` on each popped node; for the sentinel vtable that is
  0x10285371c, which on `w4==4` loads `[0x106829ea8]` and `br`s to it with
  `x0=node`, `x1=[node+32]&~1`, `x2=consumer` (2853790-2853798). The vector is a
  **leaf function pointer** the framework installs; absent it (headless boot) the
  `cbz x3, 0x2853af0` returns doing nothing.
- Seeding it with the engine's own 0x10285371c would **recurse** (that function
  itself reads the vector). A correct seed needs the real framework-installed
  "process this popped task node" worker, whose address is not statically in the
  binary — the same external-glue gap SH46 proved for the install itself.