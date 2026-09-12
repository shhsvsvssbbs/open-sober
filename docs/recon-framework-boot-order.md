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

---
## v2 (supersedes the in-image claim — deleg_ac538bda, 2 recon subagents)

### Correction from Task-1 (rigorous, APS2-decoded)
The type-4 producer vector [0x6829ea8] is **NOT installed in-image**. True .bss (NOBITS),
zero .text stores (full 18M-line disasm + register-tracking dataflow), zero data pointers,
zero load-time relocations (full ANDROID_RELA/APS2 decode of 568k relocs). The "TaskScheduler
init" at ~0x224f7xx operates on the 0x726a000 heap object, never page 0x6829000. SH46 + Cordial
are RIGHT: install is external framework glue. Smallest way to observe it live: direct guest
seed (--taskv4-seed) then inject a type-4 node so dispatcher 0x2853788 (ldr [x8,#3752]; cbz skips)
calls it — handler ABI (x0=node, x1=[node+32]&~1, x2=consumer), w4=4, x5=0.

### The ACTUAL json-abort root cause (Task-2) — fix this FIRST
jni.rs routes Call{Object,Boolean,Int,Long,Float,Void}Method -> typed-0. AutoValue params expose
data via getter methods (initParams.platformParams(), startAppParams.surface(), deviceParams.osVersion()),
so every nested object = null / scalar = 0 -> StartApp re-serializes an AppStarted struct with
uninitialized guest-stack std::strings -> RBX::json::Writer overflow (SH46's sp/sp-0x30 leak).
GetMethodID already returns cstr_handle(name) -> wire a method-name-dispatched VALUE REGISTRY for
the known getters; everything else stays typed-0. That unblocks the params layer with no new ABI.

### Param field maps (values) — authoritative shape from Cordial init_params.cpp
- InitParams: baseURL="https://www.roblox.com", buildVariant="release", userAgent=Cordial UA shape,
  deviceParams=nested, platformParams=nested, vrContext=fake Activity, isPotato=false, isTablet=false, isVrDevice=false.
- StartAppParams: appStarterPlace="", appStarterScript="", selectedTheme="Dark", username="", appUserId=0,
  isUnder13=false, membershipType=0, platformParams=nested, surface=NON-NULL android/view/Surface, vrContext=fake Activity.
- DeviceParams: osVersion="33" (VULKAN GATE; <33 refuses Vulkan), deviceName="Cordial", deviceSku="cordial",
  manufacturer="Cordial", country="US", networkType="WIFI", displayResolution="1280x720", appVersion="",
  cpu64Bit=true, isLowRamDevice=false, deviceTotalMemoryMB=8192, displayPhysical{Width,Height}Pixels=1280/720.
- PlatformParams: dpiScale=1.0 (read 3x; layout gate), isTouchDevice=false (read 2x), isKeyboardDevice=true,
  isMouseDevice=true (never read), assetFolderPath=assets dir, viewport{Width,Height}Mm=338/190.

### Java classes/methods to register (exact descriptors)
Activity.getDisplayMetrics()->DisplayMetrics + getResources()->Resources (path: act.getResources().getDisplayMetrics()).
DisplayMetrics fields: density=1.0, scaledDensity=1.0, xdpi=96, ydpi=96, densityDpi=160, widthPixels=1280, heightPixels=720.
Configuration fields (screenWidthDp=1280, screenHeightDp=720, orientation=2 LANDSCAPE, ...) + getLocales()->LocaleList (size=1, getLanguage="en", getCountry="US").
LocalStorageManager.getAllocatableBytes()->J = REAL free space (0 => engine thinks no disk => RbxStorage never builds cache).
NativeHelper gameActivity_* callbacks (onAppReady/onGameLoaded/onEngineInitialized/onFlagsLoaded...).
AssetManager: AAssetManager_fromJava/open currently return NULL (jni_stubs.h) — back with real APK zip; precondition for a frame.

### Surface from XID (already half-built in shims.rs)
set_anativewindow_xid(xid) -> ANativeWindow_fromSurface returns real XID -> eglCreateWindowSurface. The gap is JNI-side:
StartAppParams.surface jobject must be non-null class android/view/Surface (window identity comes purely from the hostcall return).
Order: set_anativewindow_xid(realXid) BEFORE nativeAppBridgeV2StartAppWithParams.

### Boot ladder (Order in this exact sequence)
1 nativeGameGlobalInit() (0x2206404)  2 nativeUpdateAdapterInit() (0x221c3ec)
3 nativeAppBridgeV2InitWithParams(InitParams) (0x2365c54)  4 nativeAppBridgeStartLuaAppDM() (0x23efe2c)
5 [surface wired] nativeAppBridgeV2StartAppWithParams(StartAppParams) (0x258b144)
6 UpdateSurfaceAppWithPlatformParams(surface, PlatformParams) (0x25f5fec) + SendAppEventOnAppReady/OnGameLoaded
7 StartGameWithParam(StartGameParams) (0x2bb4b48) for a game.
Do NOT drive AGDK onNativeWindowCreated (dead end per Cordial).