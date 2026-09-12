# SH52 — Close the real client's last 11 unresolved data imports (AMEDIAFORMAT_KEY_*)

## Summary

The productized boot line was `bound 534 JUMP_SLOT + 67 GLOB_DAT/ABS64
(0 unresolved), 11 unresolved`. `dlsym` could not resolve 11 data-object
imports because they are Android bionic/mediandk-only symbols with no host
equivalent. This cycle binds the 10 that are *string constants* to live host
C strings, leaving exactly one legitimately-unresolved import (`__sF`) that
is better left untouched.

## The mechanism (why the slots mattered)

`libroblox.so` is built against Android `libmediandk.so`, which does not exist
on the host. Its 10 `AMEDIAFORMAT_KEY_*` symbols are `OBJECT UND` — they are
the NDK media-format string constants. An `R_AARCH64_GLOB_DAT` relocation
writes **the address of the constant** into the GOT slot; the guest does

```
adrp x0, 0x67cf000
ldr  x0, [x0, #off]      ; x0 = &"mime" (the key string constant)
; ... x0 passed to AMediaFormat_setString/getString as const char*
```

with the slot value (0 before this change) used as a string pointer. A real
video/audio-decoding session reads each key as the first arg of an
`AMediaFormat_*` call — left NULL that is the SH19/SH24 crash class for data
reads: NULL deref or NULL-string read instead of a valid key.

Disasm evidence (file 0x1d9adf8 region):
`adrp x0,67cf000; ...; ldr x0,[x0,#1648]` — loads the media-key constant.

## The fix

- **`resolver::resolve_android_data(name) -> Option<u64>`** — maps each of the
  10 key names to its exact NDK constant value (`"mime"`, `"width"`, `"height"`,
  `"color-format"`, `"stride"`, `"bitrate"`, `"frame-rate"`,
  `"i-frame-interval"`, `"channel-count"`, `"sample-rate"`) built as an
  immortal leaked `CString`. The pointer is cached, so repeat resolves (JUMP_SLOT
  + GLOB_DAT + test) return the same stable address. Names outside the table
  return `None`.
- **`plt::bind_glob_dat`** STT_OBJECT branch now consults
  `resolve_android_data` before the `dlsym`/0 fallthrough.

## Why `__sF` is deliberately left unbound

`__sF` is bionic's `FILE __sF[3]` (stdin/stdout/stderr) array base. It has no
glibc export. Pointing the slot at a *host glibc `FILE_`* would be wrong: the
`fwrite`/`vfprintf` shims decide whether a stream is a real host `FILE_`
(high, 16-aligned in 0x55../0x7f..) vs a bionic/guest stream (low) and divert
the latter to host fd 2. Binding `__sF` to a host `FILE_` would make those
shims misclassify bionic streams as host streams and SIGSEGV (glibc reading a
bionic `FILE_`). Leaving the slot at its low/0 value is exactly what the shim's
fd2-diversion path already handles. So `__sF` remains the **1 legitimate**
unresolved import and is documented, not a bug.

(The extra `AMediaFormat_delete`/`AMediaCodec_delete` lines in the JIT_TRACE
diagnostic are FUNC imports that already stub-bind via the `is_func` path —
that's why only 1 is counted in the unresolved total.)

## Verification

- Productized `open-sober play --apk roblox-android.apk --jit`:
  `[plt] bound 534 JUMP_SLOT + 77 GLOB_DAT/ABS64 (0 unresolved), 1 unresolved`
  (was 67 / 11). exit 124 stable, real indexed triangle (centroid red) +
  textured quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE) + 6 quad-loop frames
  (swaps Ok(0x1)), `--persist-roundtrip` byte-exact, zero ENOSYS.
- The direct-`--renderinit` SIGSEGV/abort on manual elfjit invocation is the
  documented SH46 pre-existing harness-bootstrap artifact (reproduced
  identically pre-change); the canonical productized recipe (via sober-core
  `play --jit`, which sets `RENDERINIT_WARMUP_MS=5000` + `JIT_DRIVE_LIFECYCLE=1`
  and its own Xvfb) is clean.
- New regression `android_media_format_key_data_imports_resolve_to_live_strings`
  (arm64jit/src/resolver.rs): all 10 keys resolve to live non-null,
  NUL-terminated pointers whose bytes match the NDK constant exactly; the
  resolved address is stable across repeat calls; unrelated object names
  (`__sF`, unknown) are not claimed. Workspace 497/0.

## Frontier / next

Standing structural wall unchanged (SH14/SH46): the type-4 producer vector
`[0x6829ea8]` is framework-glue-installed only, so the engine never
self-produces a session/render task and frames stay harness-driven on the live
engine context. The media-plane *data* surface a self-driven session would read
is now live. `__sF` remains the one legitimately-unbound data import (by
design).