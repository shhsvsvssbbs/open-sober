# SH45 — json::Writer string-length-overflow abort characterized (bare StartApp); elfjit now dumps abort-class crashes

## Summary

The bare-`--startapp` boot path (no render/lifecycle drive) deterministically
aborts with:

```
libc++abi: terminating due to uncaught exception of type std::runtime_error
  : RBX::json::Writer string length overflow: 140507968772592
```

This is the SH44 documented open item ("a harness LSM host-pointer seed leaking
into a string-length read on the no-render bootstrap path; the productized
`play --jit` recipe already bypasses it"). This cycle confirms and sharpens the
characterization with concrete evidence, and adds a real diagnostic.

## Confirmed facts

- The leaked "string length" is **run-variable** (e.g. `0x7f1f53ffe9f0`,
  `0x7f6f2bffe9f0`, different on every run), and sits in the **host mmap region**
  (`0x7f...`). It is a **host pointer**, not a guest string length. It does NOT
  exactly equal either the seeded empty-LSM-map bucket array or the shared `sub`
  in any run (they differ by ~0x10 MB in the same `0x7f` segment), so the precise
  leaking allocation is **not positively identified** — the safe statement is that a
  host pointer is read where the guest expects a string length during StartApp's
  json serialization. SH44's "harness LSM host-pointer seed leaking" remains the
  leading hypothesis, not a proven identity. (The faulting read is in host/library
  fault-land beyond a guest block, so elfjit's guest SIGSEGV dumper does not fire on
  it — exit 139 without dump on the SIGSEGV variants.)*
- It is **params-independent**: `{"key":""}`, `{}`, `""`, `"X"` all abort with
  the same signature (different heap value each run). So it is NOT the params
  jstring content.
- The abort is a **guest C++ `std::runtime_error`** thrown by `RBX::json::Writer`
  when writing a string whose length exceeds the buffer — one of the 7507
  throw-with-value sites materializing `0x57765a` ("RBX::json::Writer string
  length overflow: %zu"). The specific throw in the bare path is reached from
  `nativeAppBridgeAppStart`-family code (e.g. disasm at 0x2355d98 / 0x2557dcc:
  `adrp x0,577000; add x0,x0,#0x65a; mov x1,<len>; bl 0x25fb6bc`).
- It is gated by the **absence of the render/lifecycle drive**: the full
  productized `play --jit` recipe (JIT_DRIVE_LIFECYCLE=1 + appcmd + ANativeWindow
  wiring + render-init) runs clean (exit 124, real triangle + textured-quad
  frames, zero overflow). Only the bare `--jni --startapp` path hits it. So the
  widget/harness-created framework context (LSM empty map + fake objects)
  serialized by StartApp's json writer is the leak source, and the productized
  path's lifecycle setup changes what StartApp serializes.

## Diagnostic improvement (committed)

`elfjit` previously installed the guest-state fault dumper for SIGSEGV/SIGILL
only. An abort-class crash (libc++ terminate → abort, or guest `abort()`) exited
without dumping guest PC/regs/backtrace. This commit adds SIGABRT to the signal
set (`name = "SIGABRT"`) and, before re-raising via `process::abort()` (which
itself delivers SIGABRT and would otherwise recurse into the handler), restores
SIGABRT's default disposition. Now any guest abort path yields a full guest
register + backtrace dump.

## Frontier (unchanged, next)

- The type-4 task producer vector `0x6829ea8` remains the structural wall: it is
  `.bss`, populated only by a real framework task-producer absent headlessly;
  `--taskv4-seed` proves the dispatch plane is live when seeded (SH44).
- The json::Writer abort is a **harness-bootstrap** artifact (host heap pointer
  seeded into the LSM empty map / fake-object context leaking into a guest
  string-length read), triggered only on the no-render path; the productized
  recipe bypasses it. If a future cycle needs the bare StartApp path to proceed
  without the render recipe, the next lever is to make the seeded LSM empty-map
  and fake-object memory **guest-shaped / zero-length** so StartApp's json
  serialization sees a valid (empty) string length instead of a host pointer
  (see `seed_static_empty_map` in elfjit.rs + the LSM reader 0x1d99e40).

## Verification

- `cargo test --workspace`: 491 passed / 0 failed (unchanged).
- `cargo build -p arm64jit --example elfjit`: clean.
- Productized render re-verified through the modified elfjit (exit 124, real
  indexed-triangle centroid red + textured-quad BL=RED/BR=GREEN/TR=WHITE/TL=BLUE
  + swap Ok(0x1)) — the deliverable is unaffected.
- Bare `--jni --startapp` still reproduces the documented json::Writer abort
  (expected; unchanged behavior).