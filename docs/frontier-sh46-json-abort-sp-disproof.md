# SH46 — bare-StartApp `RBX::json::Writer string length overflow` root cause: a **guest stack pointer**, NOT the seeded LSM map (SH45's leading hypothesis DISPROVEN)

## Summary

The bare `--jni --startapp` boot still aborts with
`RBX::json::Writer string length overflow: <huge>` (SH45 documented). This cycle
drives the new `JIT_DUMP_PC` register dump at both the json string-append helper
entry and the throw helper entry, across **three independent runs**, and proves
the leaked "string length" is always the **guest stack pointer** (`sp`, or
`sp − 0x30`), not the seeded LocalStorageManager empty-map allocation. SH45's
documented leading hypothesis ("harness LSM host-pointer seed leaking") is
**empirically wrong** and should not be the target of future fixes; a stale /
uninitialized std::string on the guest stack during StartApp's launch-params json
serialization is the real (guest-internal) mechanism.

The productized `play --jit` recipe still runs clean (exit 124, real triangle +
textured-quad frames) — this abort is, and remains, a **harness-bootstrap-only**
artifact on the no-render path.

## Evidence (three runs, fresh ASLR each time)

Run A (`--startapp` + `JIT_DUMP_PC=0x1025fb6bc`, the throw helper 0x25fb6bc):

```
DUMPPC pc=0x1025fb6bc x0=0x10057765a x1=0xb3 x2=0xb3 ... x19=0xb3 x20=0x7f445802a330
  x21=0x7f4462ffdab0 x22=0x7f4462ffdb48 ... x29=0x7f4462ffd9f0 x30=0x102355da8 x31=0x7f4462ffd9f0
libc++abi: terminating ... RBX::json::Writer string length overflow: 139931695438320
```
`139931695438320` = `0x7f4462ffd9f0` = **exactly x29 == x31 (sp)** of the same
frame. (The `%zu` reads the first vararg; the engine's throw-with-value site at
`0x2557dcc`/`0x2355d98` does `mov x1,x19; bl 0x25fb6bc` with x19 = the length.)

Run B (`JIT_DUMP_PC=0x102355d40`, the append's check-fn entry):

```
DUMPPC pc=0x102355d40 ... x1=0x7f013802a330 x2=0xb3 x19=0xb3 ... x31=0x7f0142c40a20
libc++abi: terminating ... overload: 139643391838704
```
`139643391838704` = `0x7f0142c409f0` = **x31(sp) − 0x30** (a stack slot just
below the frame pointer).

Run C (same, separate process): leak `0x7f2c7fffe9f0` (sp-family), and a third
`0x7f1ebe4779f0` == sp − 0x30.

All three leaked values sit in the **guest stack region** and track `sp` to
within a small constant (−0x30), never near the seeded LSM bucket array
(`~0x7f3…d010`, which is **~0x260–0x2a0 MB away** in the host mmap/heap region).
The SH45 "host pointer leaked from the LSM seed" identity is therefore **not
the leaking allocation**: the value read as the string length is a live stack
address.

## Mechanical conclusion

Inside `nativeAppBridgeAppStart`-family code, StartApp serializes its launch /
framework context to json; one of the std::string length fields it reads is
uninitialized on the guest stack, so the json Writer's
`ldrsw x8,[0x7275000+1608]` bound-check (`cmp x8,x2; b.cc`) trips with the value
of sp (or sp−0x30) as the "length". This is a guest-internal bootstrap ordering
gap on the bare (no-render, no-lifecycle-drive) path — **not** our harness's
LSM seed. It differs from the productized path only in the lifecycle setup the
`--renderinit`/`JIT_DRIVE_LIFECYCLE` recipe performs, which changes what
StartApp serializes first.

## What still stands (unchanged)

- The type-4 task producer vector `[0x6829ea8]` remains 0 on the bare boot and
  (per SH44) is only populated by real Android-framework producer glue, absent
  headlessly. A static disassembly scan this cycle confirms **no guest .text
  instruction in libroblox.so stores to guest `0x106829ea8`** (the only `[x,#3752]`
  stores are struct-relative on heap/sp registers, never the `adrp 0x6829000`
  static base we confirmed the dispatcher `0x2853784` reads from). So "reverse
  what the framework installs into the vector in-code" is a confirmed dead end —
  the install is external glue (engine's framework/game-activity layer).
- Productized `play --jit` recipe re-verified green this cycle (exit 124, real
  indexed triangle, textured quad, quad-loop swaps Ok(0x1)).

## Verification
- `cargo test --workspace`: 491 passed / 0 failed (unchanged).
- `cargo build --workspace` clean.
- Productized render re-verified through the modified elfjit.
- Bare `--jni --startapp` reproduces the abort exactly as documented (unchanged),
  with the leaked value now pinned to the guest stack.