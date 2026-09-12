# SH56 — Empirically close recon Task-2: the AutoValue value registry does NOT fix the StartApp json-abort

## Question
The recon (docs/recon-framework-boot-order.md, Task-2) claims the `RBX::json::Writer`
string-length-overflow abort's **root cause** is the JNI shim: `Call*Method` returned
typed-0, collapsing the AutoValue params layer so StartApp re-serialized uninitialised
guest-stack std::strings. SH55 wired the full getter-`VALUE` registry
(`Call{Object,Boolean,Int,Long,Float}Method`). But that registry had **never been
observed live** against StartApp's serialization: SH55's `--v2boot` ladder stalled at
rung 1 (`nativeGameGlobalInit` parks) before reaching StartApp, and the productized
recipe passes StartApp a bare **JSON jstring** (`{"key":""}`), not a jobject — so the
AutoValue getter path was never exercised. This cycle drives
`nativeAppBridgeV2StartAppWithParams (0x258b144)` with a **genuine AutoValue jobject**
params (new harness flag `--startapp-jobject`) and answers definitively.

## Setup / probe
- Real `libroblox.so` (v2.738.1397) extracted, guest base 0x100000000, SH55 HEAD.
- New elfjit flag `--startapp-jobject`: in the `--startapp` drive, pass
  `jni::new_fake_object()` (a real AutoValue-style jobject) as the params handle instead
  of the legacy JSON jstring, so StartApp's serialization routes through the SH55
  getter-value registry. Default (JSON jstring) unchanged — productized recipe untouched.

## Result — the registry does NOT unblock StartApp; the abort is params-independent

Both drives abort identically (exit 139, libc++ `std::runtime_error : RBX::json::Writer
string length overflow: <host-pointer>`):

| params handle | result |
|---|---|
| legacy JSON jstring `{"key":""}` (runs/sh56-startapp-json-abort.txt) | abort, leak `0x7fb52...` (host mmap), exit 139 |
| real AutoValue **jobject** + full value registry (runs/sh56-startapp-jobject-abort.txt) | abort, leak `0x7f19...` (host mmap), exit 139 |

Rigorous pin (runs/sh56-jsondump.txt, `JIT_DUMP_PC`):
- **Append bound-check `0x102355d40`**: `x1` holds a **host mmap pointer**
  (`0x7ffa...`) read as the std::string length → `ldrsw x8,[0x7275000+1608]; cmp x8,x2;
  b.cc` throws. The leaked "length" is a host pointer, run-variable, params-independent
  (identical mechanism to SH45's sp/sp-0x30 leak).
- **Throw helper `0x1025fb6bc`**: `x0=0x10057765a` (fmt `"RBX::json::Writer string
  length overflow: %zu"`), the printed value is the host pointer from the va_list.
- **`JIT_TRACE=1`: ZERO `[jni] Call*Method` getter lines fire** during StartApp's
  serialization — the AutoValue getter registry is never even reached. So the params
  layer is not the leaking allocation.

## Conclusion (redirects the frontier)
Recon Task-2's root-cause claim is **empirically disproven on the real binary**: the
value registry is necessary for valid params IF StartApp ever gets far enough to use
them, but it is NOT the json-abort's cause — the overwrite/leak is an in-memory host
pointer read where the guest expects a std::string length, independent of whether the
params is a JSON jstring or a full AutoValue jobject with live getter values. This
confirms SH45's original characterization ("gated by the absence of the
render/lifecycle drive", a harness-bootstrap artifact): the **productized** recipe
(drives lifecycle + ANativeWindow + render-init warmup before/WITH StartApp) never hits
the abort and renders real frames — re-verified green this cycle (runs/sh56 product
verify: persist roundtrip byte-exact, real indexed triangle centroid red, textured quad
BL=RED/BR=GREEN/TR=WHITE/TL=BLUE, 6 quad-loop frames, swaps Ok(0x1)).

Future work on the bare StartApp path should target the guest-stack/memory state that
the leaked length reads (the allocation whose length field holds a host pointer), NOT
the AutoValue getter registry. The `--startapp-jobject` probe is kept as a reusable
diagnostic for any future params-layer test. Standing structural wall unchanged: the
type-4 producer vector `[0x106829ea8]` is still framework-glue-seeded only; frames
remain harness-driven on the live engine context.

## Verification
- `cargo test --workspace` **499/0** (unchanged).
- Productized `play` render re-verified green with the new flag compiled in (default
  JSON path unchanged, no regression).
- **Productized + `--startapp-jobject`** (real AutoValue jobject under the full
  lifecycle+render drive, runs/sh56-product-jobject.txt): runs clean, exit 124 stable
  idle, same real render (triangle centroid red, textured quad
  BL=RED/BR=GREEN/TR=WHITE/TL=BLUE, 6 quad-loop frames, swaps Ok(0x1)), persist
  roundtrip byte-exact, **zero** json-abort — i.e. identical to the JSON baseline. So
  the params layer is not load-bearing for progression either: StartApp reaches the
  same idle platform whether the params is a bare JSON jstring or a fully-armed
  AutoValue jobject with live getter values.
- Artifacts: runs/sh56-startapp-json-abort.txt, runs/sh56-startapp-jobject-abort.txt,
  runs/sh56-jsondump.txt, runs/sh56-product-jobject.txt (worker ledger
  /home/hermes-worker/runs/).