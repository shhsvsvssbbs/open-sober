# SH61 — JSON-ABORT FIX: recon-selfdrive-seed-jsonfix.md deliverable (2) implemented

## Summary

Deliverable (2) of docs/recon-selfdrive-seed-jsonfix.md — the
`RBX::json::Writer string length overflow` stack-leak fix — is IMPLEMENTED via a
host-side length clamp at the append bound-check, verified on the real
libroblox.so. A research subagent (read-only on the binary) refined the brief's
proposed stack write into a safer register clamp with identical semantics.

Workspace **506/0** (was 505/0, +1 regression). Commits landed on local `dev`.

## The fix

New env-gated hook in jit.rs run_loop (armed only with `JIT_JSON_ZERO_FIX=1`,
evaluated once via `json_zero_fix_enabled()`): on entering the guest append
bound-check block at pc == 0x102355d40, read the string LENGTH from reg x2 and
the writer CAPACITY cell at guest 0x107275648 (file 0x7275648, sign-extended i32
loaded `adrp x8,7275000; ldrsw x8,[x8,#1608]`). If `(cap as u64) < len` — the
exact `cmp x8,x2; b.cc` throw condition — force `x2 := 0`, turning the write
into a libc++ SSO EMPTY string (size()==0). The throw helper 0x1025fb6bc is then
never reached. The capacity cell is READ-ONLY (never raised — raising makes the
writer memcpy with len's low 32 bits ~1.6GB -> SEGV). Guest memory is
identity-mapped so the cap reads directly. ASLR-immune: it reads/writes only the
live CpuState register bank and the live cap cell, never a fixed stack address.

Why register-clamp over the brief's stack write: the recon proposed zero-filling
[append-entry-sp-0x38], but static disasm could not positively confirm that slot
lies inside a live frame (it is below the check-fn's own frame — red-zone /
caller-below); a stack write there is unverified. Clamping x2 delivers the SAME
"size()==0 -> SSO empty" effect with no out-of-frame memory write and no
frame-offset re-derivation.

## Verification (real libroblox.so, bare StartApp, no render recipe)

Before (JIT_JSON_ZERO_FIX unset), the documented abort reproduces:
```
libc++abi: terminating ... RBX::json::Writer string length overflow: 139734512814576
exit 139
```
(leaked value is a run-variable host heap pointer).

After (JIT_JSON_ZERO_FIX=1), the json overflow is eliminated and StartApp's
serialization proceeds:
```
[json-fix] append check 0x102355d40 would overflow (len=0xb3 cap=0) -> forcing len=0 (SSO empty append)
[json-fix] append check 0x102355d40 would overflow (len=0x24 cap=0) -> forcing len=0 (SSO empty append)
grep -c "json::Writer string length overflow": 0
```
Both observed leak modes are covered by the `would_throw` guard (cap < len):
the huge host-pointer length and the small-but-over-cap case (len=179, cap=0).
Benign lengths (len <= cap) are untouched.

**Honest boundary:** the fix eliminates *only* the json abortion. The bare
`--jni --startapp` path (NO render/lifecycle drive) then proceeds deeper into
boot and hits a DIFFERENT, PRE-EXISTING fault — SIGSEGV at guestpc 0x102175854
(`ldr x23,[x20,#8]`, null deref in a GameActivity/FMOD init region), exit
134/SIGABRT. That is the SH45-documented bare-path wall (only the full
productized recipe with JIT_DRIVE_LIFECYCLE + render-init boots clean, exit
124). The json fix is scoped to its target and does not regress the clean
productized path (re-verified green this cycle, below).

## Regression

`json_zero_fix_clamps_leaked_length_at_append_check` (jit.rs) pins the exact
disassembled addresses (append check file 0x2355d40, cap cell file 0x7275648,
throw helper file 0x25fb6bc), the `(cap as u64) < len` predicate for both leak
modes (huge host pointer, small-over-cap), asserts benign lengths are untouched,
and asserts len==0 never trips any cap.

## Productized baseline re-verified unchanged (runs/sh61-product-reverify.txt)

`open-sober play --apk --jit` exit 124 stable, real indexed triangle (centroid
red) + textured quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE exact texels) + quad-loop
frames, byte-exact persist roundtrip, zero json-abort/SIGSEGV/ENOSYS (the
JIT_JSON_ZERO_FIX hook is off by default, so the production path is untouched).

## Next frontier

Both recon-v3 deliverables are now implemented and measured headlessly:
(1) self-driven task frames via the type4_frame_thunk seed (SH60), (2) the
json-abort fix (this cycle). The next task per the SH60 frontier doc: bridge the
w4=4 dispatch rate into the post-ctx window (drive a drain cycle after
RENDERCTX), chase `swap Ok(0x0)` on the cross-thread path by serializing
presenters to one thread (the engine's 0x105b3b408 swap never binds; EGL
current-binding is thread-local), and feed a real engine session producer. The
engine's REAL frame-plane driver is guest 0x105b2ead4 (scene renderer: R+0x160
ctx / R+0x170 view / R+0x180 scene list) — it only renders real login/home once
the engine constructs a populated render-manager, which needs its game/UI setup.