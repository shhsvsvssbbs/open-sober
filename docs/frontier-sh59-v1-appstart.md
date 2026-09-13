# SH59 — drive the recon-v1 "still-live lower-effort" V1 start standalone

## One-line result
Added `--startapp-v1`: drives the legacy 6-jstring `nativeAppBridgeAppStart__`
(guest `0x102338510`, file `0x2338510`) as a **standalone primary** start on the
real `libroblox.so` — the FIRST time the V1 path has been exercised headlessly
(into the full render recipe, clean exit 124, no json-abort/crash). Recon-v1
line 38 named this the "still-live lower-effort alternative" that bypasses the
AutoValue params layer; SH54-56 only ever ran it as the tail of the v2boot
ladder, which stalls at rung 1 `nativeGameGlobalInit` every cycle — so the V1
entry's downstream had never actually executed.

## Why this cycle
The recon-v2 (docs/recon-framework-boot-order.md §v2, Task-2) claims the
StartApp json-abort is the JNI shim collapsing the AutoValue params layer. SH56
**empirically disproved** that (the leaked length is a guest-stack pointer,
params-independent; the getter registry never even fires on the V2 path). But
recon-v1's *other* escape hatch was never tried as the primary: the legacy
`nativeAppBridgeAppStart__` reads six **plain jstrings** straight from x2..x7
(`String, String, Z, String, String, String` per the mangled sym
`...AppStart__Ljava_lang_String_2Ljava_lang_String_2ZLjava_lang_String_2
Ljava_lang_String_2Ljava_lang_String_2`). No jobject, no Call*Method, no
AppStarted json re-serialization — so the params-collapse abort **cannot** fire
on it.

## What landed
- **elfjit `--startapp-v1`**: when set alongside `--startapp <hex>`, the target
  is swapped from V2StartAppWithParams to file `0x2338510` and the s2 ABI is
  built as 5 empty jstrings (x2,x3,x5,x6,x7) + jboolean false (x4=0). Everything
  else in the productized recipe (JIT_DRIVE_LIFECYCLE, render-init thunk, window
  wiring, persist roundtrip, kicker) runs identically.
- **Fixed a latent wrong ABI** in the (never-reachable) v2boot V1 fallback: it
  put a jstring handle in the `Z` boolean slot (x4) and left x7=0 — correct now
  to 5 strings + boolean-false, matching the descriptor.
- **New regression** `v1_app_start_six_arg_abi_is_five_strings_plus_boolean`
  pins the ABI (5 readable, guest-addressable jstring handles in the String
  slots; x4 Z-slot exactly 0; no String handle aliases the boolean slot).

## Verification (real libroblox.so, full productized recipe + `--startapp-v1`)
- Exit 124 (stable idle after the render prove).
- Engine's OWN frame-fn (`0x105b32c00`) renders a real indexed triangle
  (centroid RGBA(255,0,0,255)) + textured quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE)
  + 4 fresh quad-loop frames (swaps Ok(0x1)).
- Byte-exact persist roundtrip (45 B, `ROBLOSECURITY=_live_client_remembered_session`),
  real fsmap remaps under the armed SOBER_ANDROID_ROOT.
- **Zero** `RBX::json::Writer string length overflow`, zero SIGSEGV/SIGABRT, zero
  ENOSYS — the V1 path executes clean where V2StartApp's params layer was suspect.
- Artifacts: this commit's run captured in `runs/sh59-v1-appstart.txt`.

## Honest scope
V1 AppStart__ advances the engine to the **same** structural place as V2: the
main loop parks on the lifecycle-await futex (`guest_svc` FUTEX_WAIT at
lr=0x10284d134) and the type-4 producer vector `[0x106829ea8]` stays 0
(framework-glue-installed only) — so a self-driven home/session screen is not
yet reached; frames are still harness-driven on the live engine context. What
SH59 adds is empirical + corrective: the recon-v1 lower-effort alternative is
now proven **clean and standalone-callable** (a real secondary boot entry that
bypasses the params layer entirely), and its previously-wrong ABI is fixed and
pinned so future work on the V1 path starts from a correct register layout.
Workspace **504/0** (+1). Standing structural wall unchanged.