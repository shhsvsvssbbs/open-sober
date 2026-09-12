# Recon — the type-4 producer vector is NOT external glue; it's in-image TaskScheduler init we never trigger

Source: two parallel recon subagents (deleg_4333c8cb) — AOSP lifecycle research + read-only
disasm/readelf of the real libroblox.so (guest base = file vaddr + 0x100000000), cross-checked
against the public Cordial runtime and Sober/Gunbark app-bridge analysis. Full spec:
/home/hermes-worker/open-sober-framework-glue-spec.md

## The reframe (highest-leverage change)
SH46's scan found "no in-code store to [0x6829ea8]" — but that scan was done on a BARE boot that
NEVER REACHES TaskScheduler init. The vector is installed IN-IMAGE by the TaskScheduler's own init,
which only runs during `nativeGameGlobalInit` + `V2InitWithParams` under the right flags (proven by
the fatal string `"Can't initialize the TaskScheduler before flags have been loaded"` @0x379b27).
So the frontier changes from "forge invisible external glue" to: **drive the real framework boot
sequence IN ORDER and watch [0x106829ea8] populate.**

## Root cause of the json-string abort
The harness currently calls `nativeAppBridgeV2StartAppWithParams (0x258b144)` FIRST, bare and out
of order, with a JSON STRING where a JNI AutoValue jobject is expected. StartApp introspects the
params via GetObjectClass -> null -> AutoValue getters return nothing -> the AppStarted struct it
then re-serializes holds UNINITIALIZED guest-stack std::strings -> `RBX::json::Writer string length
overflow` (SH46's sp/sp-0x30 leak). Params content doesn't matter; the OBJECT LAYER + ORDERING do.

## The real ignition sequence (order is load-bearing)
1. nativeGameGlobalInit        (0x2206404)  + nativeUpdateAdapterInit (0x221c3ec)   [Main thread + globals]
2. setTaskSchedulerBackgroundMode(enable=false,"ASMA.start")  (export 0x2bb2380 -> setter 0x258aff0)
   *** the single most important missing call — a backgrounded scheduler is told not to render ***
3. nativeAppBridgeV2InitWithParams(InitParams)   (0x2365c54, 1812B bring-up)
4. nativeAppBridgeStartLuaAppDM()  (0x23efe2c)   [starts the Lua app shell / home-screen renderer]
5. once a Surface exists: nativeAppBridgeV2StartAppWithParams(StartAppParams) (0x258b144)
6. UpdateSurfaceAppWithPlatformParams / SendAppEventOnAppReady / OnGameLoaded
7. StartGameWithParam (0x2bb4b48) for a game/place.

## Param objects are AutoValue JNI classes (NOT JSON)
InitParams {baseURL,buildVariant,userAgent,deviceParams,platformParams,vrContext,...}
StartAppParams {appStarterPlace="",appStarterScript="",selectedTheme="Dark",username,appUserId,
  isUnder13,membershipType,platformParams,surface,vrContext}
DeviceParams needs osVersion="33" (below it the engine refuses Vulkan).
Lower-effort alternative: the V1 6-jstring path nativeAppBridgeAppStart__ (0x2338510) is still live.

## Immediate next step (Tier 1)
Drive the sequence in order and dump [0x106829ea8] + the type-4 dispatcher region (0x2853784)
after EACH step (existing JIT_FRAMEWORK_DUMP samples it). First non-zero vector = the gate opens.
Reuse the SH49 sustainable injector once the scheduler is foreground.

## Address quick-ref (guest addrs)
JNI_OnLoad 0x1017_... (0x2173ff4+base); nativeGameGlobalInit 0x102206404; setTaskSchedulerBG 0x102bb2380;
V2InitWithParams 0x102365c54; StartLuaAppDM 0x1023efe2c; V2StartAppWithParams 0x10258b144;
type-4 dispatcher/vector 0x102853784 / [0x106829ea8]; drain 0x102856e40; V1 AppStart__ 0x102338510.