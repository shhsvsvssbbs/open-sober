# SH55 — Complete the AutoValue params getter-value registry (Call{Object,Boolean,Int,Long,Float}Method)

## What landed
The recon-v2 Task-2 json-abort root cause (docs/recon-framework-boot-order.md) is that
`jni.rs` routed **every** `Call*Method` to typed-0, so the engine's StartApp json
serialization read uninitialized guest-stack std::strings. SH54 wired the String /
Bool / Int getters. This cycle completes the prescribed surface and fills the load-bearing
param shapes the recon names:

## Call*Method value registry (jni.rs)
- **String getters** (`CallObjectMethod`, already present) extended with the DeviceParams
  shape: `getOsVersion → "33"` (the **Vulkan GATE** — below 33 the engine refuses Vulkan),
  `getDeviceName→"Cordial"`, `getDeviceSku→"cordial"`, `getManufacturer→"Cordial"`,
  `getCountry→"US"`, `getNetworkType→"WIFI"`, `getAppVersion→""`. Plus the pre-existing
  InitParams/StartAppParams strings (`getBaseURL`…, `getSelectedTheme→"Dark"`,
  `getUsername→""`, `getSurface` non-null, `getAppUserId→""`).
- **Boolean getters** (`CallBooleanMethod`) per recon v2: isUnder13/isPotato/isTablet/
  isVrDevice/isTouchDevice/isLowRamDevice → false; isKeyboardDevice/isMouseDevice/
  isCpu64Bit → true.
- **Long getters** (`CallLongMethod`, NDK slot **52**, newly wired): getAppUserId→0,
  getDeviceTotalMemoryMB→8192.
- **Float getters** (`CallFloatMethod`, NDK slot **55**, newly wired): getDpiScale→1.0
  (read 3×; the layout gate).

## The float-return bridge (jit.rs) — the non-obvious part
`jfloat CallFloatMethod(env, obj, mid, …)` passes its args in the **integer** registers
(x0..x2) but returns the `jfloat` in the FP register **s0** (AAPCS64). The existing float
bridges can't serve it: `HostFloat32Call` marshals only v-register args (dropping the
methodID) and `HostGlesCall` writes its return to x0, not s0. Added a dedicated region +
type:

- `HostJniF32 = extern "C" fn(*mut CpuState) -> u32` — reads the whole guest state
  (so it can recover the methodID from x2) and the dispatcher writes the `u32` into the
  low lane of guest **s0** (`s.v[0] = (s.v[0] & !0xffff_ffff) | ret`), then resumes at x30.
- `HOST_JNI_F32_CALLS` table after the GLES region + `host_jni_f32_base()` /
  `register_jni_f32_call()` / `host_jni_f32_call_at()` + a run-loop branch mirroring the
  GLES one.
- `CALL_FLOAT_METHOD` stores a `register_jni_f32_call` address (not an int hostcall), so a
  guest `blr` through env->functions[55] lands in the new region.

## Regressions (workspace 498/0 → 499/0, +1)
- `jni_auto_value_params_getters_resolve_via_fn_table` extended: covers osVersion/device
  strings (length checks), the new booleans, Long getAppUserId — all through the **official
  NDK table** (GetMethodID → Call*Method), plus the legacy fallback unchanged.
- **NEW** `jni_call_float_method_returns_jfloat_in_s0_via_jit`: JIT-runs real guest aarch64
  (`ldr funcs,[env]; ldr slot,[funcs,#440]; blr slot; fmov w0,s0; brk`) and asserts s0
  (v0 low 32) == 1.0f32 — the float value survives a real table dispatch into s0.
  (First version ended in `ret`, whose x30 was the post-`blr` address → infinite loop;
  ending with `brk #0` makes jit_run terminate at pc=0.)
- `jni_fake_object_surface_offsets_match_android_ndk` pins CALL_LONG_METHOD=52,
  CALL_FLOAT_METHOD=55.

## Empirical: the ordered V2 ladder, with a COMPLETE params layer, still stalls at rung 1
Ran the full stable productized recipe + `--v2boot` on the real `libroblox.so`
(runs/sh55-v2boot-ladder.txt, exit 124):
- Render + persist baseline INTACT (persist roundtrip byte-exact, drawprobe + triangle
  via the engine's own wrapper, swaps Ok(0x1)) — the registry doesn't regress the product.
- Zero SIGSEGV/SIGABRT, zero `RBX::json` string-overflow.
- Ladder reached only `driving nativeGameGlobalInit`; **`nativeGameGlobalInit` does not
  return** (runs real init then parks), so rungs 2–6 (setTaskSchedulerBM, V2InitWithParams,
  StartLuaAppDM, V2StartAppWithParams) never run on the detached driver thread, and
  `[0x106829ea8]` stays **0** throughout.

This is the same wall SH54 saw — but SH54 measured it DURING GlobalInit with only the
partial registry. Now measured with the full Call{Object,Boolean,Int,Long,Float} registry
(so StartApp's serialization would have real values), the ladder STILL cannot reach the
install site. It re-confirms the recon Task-1 (rigorous, APS2) verdict at runtime with the
params layer complete: the ordered-ladder reframe does not populate the type-4 producer
vector headlessly.

## Standing structural wall (unchanged)
The type-4 producer vector `[0x106829ea8]` is framework-glue-seeded only — no in-image
store, and no ordered native sequence populates it headlessly. The ONLY proven live
dispatch plane stays the host seed: `--taskv4-seed <guest-frame/session-handler>` +
`--deque-node-live` (SH44 proved the plane live when seeded; SH49 made it sustainable at
197 pops). Next frontier: feed a REAL engine frame/session producer address into the seed so
a sustainably-dispatched task node advances the engine toward its own frame/screen.