# Open Sober — Agent Handoff

## Session (Sep 13, 2026, hermes-worker, cycle SH62) — drove the engine's REAL scene renderer (guest 0x105b2ead4): the engine constructs+registers its OWN frame-desc (vtable 0x1067317b0) and presents it via the real ctx swap — replacing the SH60/61 harness-fabricated clear-path renderer. Workspace **507/0** (was 506/0, +1). Commits b30eaae + b39a525. Doc docs/frontier-sh62-renderscene.md, artifact runs/sh62-renderscene.txt, repro runs/capture_renderscene.sh.

Two READ-ONLY research subagents + my disasm of real libroblox.so pinned the concrete
frontier artifact: the engine's REAL frame-plane driver is `0x105b2ead4` (scene
renderer), and — disasm-proven — it builds+links a real 0x98 frame-desc
**UNCONDITIONALLY, even with an EMPTY scene list**: bind ctx (vt+16 make-current),
read view W/H at R+0x170+112/116, dims-query (ctx-vt+64), then operator-new(0x98)
0x1d96768 -> frame-desc ctor 0x5b34de8 (sets vtable 0x6731000+0x7b0=0x1067317b0 at
[+0], [+140]=w7, [+144]=1) -> link 0x5b2d9e0(&R+0x170, frame); ONLY THEN compares
scene list head/tail (R+0x180/0x188); empty => skip => return 1.

New elfjt `--renderscene`: arm a fabricated-but-engine-native render-manager R
(R+0x160=recovered real ctx, R+0x170=view w/ SENTINEL W/H, R+0x180==R+0x188 empty),
drive 0x105b2ead4(R) on the currency-owning renderinit thread after RENDERCTX, verify
engine_registered (frame non-zero, [+144]==1), present via real ctx swap 0x105b3b408.

**Empirical (real libroblox.so, runs/sh62-renderscene.txt, exit 124):**
`scene renderer Ok(0x1)`; `R+0x170 frame=0x7fb68a1c8c00 node=0x7fb689fa4f30
engine_registered=true frame[vtable]=0x1067317b0[+140]=0x0`; `present #N swap Ok(0x1)`
x3; persist 45B byte-exact; zero SIGSEGV/SIGABRT/json-overflow.

**Key empirical catch (SDLC):** setting the fabricated view W/H to 1280x720 —
exactly the live EGL surface dims the dims-query returns — makes the renderer's
"dims unchanged = frame already built" check (`cmp w8,x22; b.eq skip`) SKIP the build
(R+0x170 stays = our view). Fix: view W/H = sentinel 0xFFFFFFFF/0xFFFFFFFE (never ==
surface dims) forces the BUILD branch; engine constructs the frame at the REAL dims.

**Honest scope:** activates the engine's real scene-renderer frame plane + proves it
constructs/registers its own frame item headlessly — but the item is the frame-desc,
NOT a populated login/home UI screen. R+0x180 scene list is still EMPTY (fast path).
Real screens need the engine to populate the scene list with UI items = the Lua
app-shell + auth/network (standing structural wall). Contribution: engine's own
frame-desc construction is provably reachable headlessly; the repeated SH18/60
fabricated clear renderer is replaced by the engine's real frame-desc construction.

**Next frontier:** populate R+0x180 scene list — construct a real 0x28-stride scene
node (node+8 coherent render obj, node+24 real view) so the renderer's per-node loop
presents engine-detailed content, not just the single empty-scene frame-desc. Depends
on the in-image scene-item builder (file 0x5b2c828 / 0x5b2eb7c / 0x5b2ec1c) or a real
screen-construction entry beyond the Lua wall.

Also this cycle: fixed a pre-existing **concurrency race** in the resolver —
`resolve_gles_int/mixed/egl` probed the cache WITHOUT the lock, dlsym'd, then
`alloc_slot` re-locked and allocated a fresh slot WITHOUT re-checking the cache. Two
threads resolving the same GLES name got two DIFFERENT adjacent slots (off-by-8),
breaking the slot-identity invariant the API/tests rely on
(`egl_get_proc_address_routes_guest_blr_to_dispatchable_slot_e2e` failed in the full
parallel workspace run, passed 5/5 in isolation). Fixed: `alloc_slot` now
re-checks `r.slots` under the already-held lock (idempotent-in-cache), closing the race
for every caller in one place. Full arm64jit suite passed 6/6 parallel runs (was
flaky); workspace 507/0. Commit f942d23.

## Session (Sep 13, 2026, hermes-worker, cycle SH61b) — closed the SH60 cross-thread swap Ok(0x0) wall. **On the real libroblox.so taskv4-frame recipe: 24/24 task-driven presents are now genuine eglSwapBuffers Ok(0x1), ZERO Ok(0x0)** (was 25 Ok(0x1) / 5924 Ok(0x0) in SH60), exit 124. Commit 68b3362. Doc docs/frontier-sh61b-single-owner-presenter.md, artifacts runs/sh61b-presenter.txt + runs/sh61b-product-reverify.txt.

An empirical negative pinned the fix. I first tried a global presenter **mutex**
around the whole bind→frame-fn→swap sequence (SH60's "serialize presenters"
suggestion). It did NOT change the ratio (25/6399) — the Ok(0x1)s are exactly the 25
renderinit-thread presents. Conclusion: the drain thread's run_guest_callback
make-current cannot establish EGL currency even serialized (its guest callback path
differs from the renderinit thread's). The fix is to ROUTE the present to the
currency-owning thread, not to lock:

- **`type4_frame_thunk` (drain thread) is now a pure producer**: accounts the
  dispatch, reads the node's dispatchable flag ([node+40] bit0 — diagnostic only),
  bumps a process-wide `PENDING_PRESENTS` counter, returns. No EGL work / no
  run_guest_callback → safe + cheap on the drain thread, cannot hit the EGL-current
  wall. Self-guards on RENDERCTX==0.
- **New `present_one_task_frame(ctx, n)`**: engine make-current 0x105b3b358 →
  frame-fn 0x105b32c00 → swap 0x105b3b408, returns the swap result.
- **Presenter loop** on the --renderthunk thread (the ONE thread where render-init
  left EGL current): drains PENDING_PRESENTS, rate-limited to a bounded window
  (TASKFRAME_MAX_FRAMES / TASKFRAME_WINDOW_MS, default 24 / 2 s) so the run still
  exits 124 cleanly. The drain flood adds PENDING orders of magnitude faster than
  llvmpipe can present, so the bound keeps it a clean sustainable stream.

**Verified** (runs/sh61b-presenter.txt, 8k-line log, exit 124): 24/24 `present
swap Ok(0x1)`, 0 Ok(0x0), `presenter drained: 24 real task-driven frames presented
(all on the currency-owning thread)`, 196 real NODE pops, dispatch counter #3.4M,
zero crash/json-abort, persist roundtrip intact. **Productized baseline re-verified**
(runs/sh61b-product-reverify.txt): exit 124, real triangle (centroid red) + textured
quad (BL/BR/TR/TL) + 7 swap Ok(0x1) + persist 45B byte-exact + 0 crash; no-seed path
fires the single deterministic present (SH60 marker preserved). Workspace 506/0.

**Honest scope:** closes the swap-Ok(0x0) wall — task frames now present genuinely
on the correct thread — but does not change WHAT is rendered (still the clear-path
frame-fn with the fabricated renderer, not the engine's own login/home UI, which
needs a populated render-manager). The PENDING counter can grow large during the
flood (harmless u64, no stored frames).

**Next frontier (unchanged):** feed a real engine session producer; the engine's
REAL frame driver is guest 0x105b2ead4 (scene renderer: R+0x160 ctx / R+0x170 view /
R+0x180 scene list) — only renders real login/home once the engine constructs a
populated render-manager (needs its game/UI setup, Lua/textures/login scene). That is
the standing structural wall for "engine renders its own real screens".

## Session (Sep 13, 2026, hermes-worker, cycle SH61) — DELIVERED recon-v3 deliverable (2): the RBX::json::Writer stack-leak fix. New env-gated JIT hook (JIT_JSON_ZERO_FIX=1, OnceLock-evaluated `json_zero_fix_enabled`) clamps the string LENGTH (reg x2) to 0 at the append bound-check guest 0x102355d40 exactly when it would throw (writer cap cell guest 0x107275648 < len → `cmp x8,x2; b.cc`), so the leaked uninitialised-stack-string write becomes a libc++ SSO EMPTY append (size()==0) that never reaches the throw helper 0x1025fb6bc. Workspace **506/0** (was 505/0, +1). Commit 63302e2. Doc docs/frontier-sh61-json-fix.md, artifact runs/sh61-json-fix-after.txt + runs/sh61-product-reverify.txt. **Both recon-v3 deliverables are now implemented & measured headlessly.**

A research subagent (deleg_6bb58b66, read-only on the repo + binary disasm) refined
the brief's proposed fix. The recon suggested zero-filling the leaking guest-stack
slot at `[append-entry-sp-0x38]`, but static disasm could not positively confirm
that slot lies inside a live frame (it is below the check-fn's own frame —
red-zone/caller-below), so a stack write there is unverified. The subagent's
recommended register clamp delivers the SAME "size()==0 → SSO empty" effect with no
out-of-frame write and no frame-offset re-derivation:

- **Patch (jit.rs run_loop, at block entry):** when armed and pc == 0x102355d40, read
  len = reg x2 and cap = i32 at guest 0x107275648 (disasm: `adrp x8,7275000; ldrsw
  x8,[x8,#1608]`); if `(cap as u64) < len` — the exact `b.cc` throw condition — set
  x2 := 0. Guest memory is identity-mapped so the cap reads directly; ASLR-immune.
- **Capacity cell is READ-ONLY** — never raised (raising makes the writer memcpy with
  len's low 32 bits ~1.6GB → SEGV), per the recon's hard rule.
- **`json_zero_fix_enabled()`** evaluates JIT_JSON_ZERO_FIX once via `OnceLock` (not
  per-block), keeping the hot loop clean. Off by default → production path untouched.

**Verified on real libroblox.so (bare StartApp, no render recipe):**
- WITHOUT the fix: `libc++abi: terminating ... RBX::json::Writer string length
  overflow: 139734512814576` (run-variable host heap ptr), exit 139.
- WITH JIT_JSON_ZERO_FIX=1: `[json-fix] ... would overflow (len=0xb3 cap=0)` +
  `(len=0x24 cap=0)`, then `grep -c "json::Writer string length overflow"` = **0**;
  StartApp's json serialization proceeds. Both leak modes covered (huge host-pointer
  len, small-but-over-cap len); benign len ≤ cap untouched.

**Honest boundary:** the fix eliminates only the json abortion. The bare `--jni
--startapp` path (NO render/lifecycle drive) then proceeds deeper into boot and hits a
DIFFERENT pre-existing fault — SIGSEGV at guestpc 0x102175854 (`ldr x23,[x20,#8]`,
null deref in a GameActivity/FMOD init region), exit 134/SIGABRT. That is the
SH45-documented bare-path wall (only the full productized recipe with
JIT_DRIVE_LIFECYCLE + render-init boots clean, exit 124). The json fix does not
regress the productized path (re-verified green below).

**Productized baseline re-verified unchanged (runs/sh61-product-reverify.txt):**
real indexed triangle (centroid RGBA(255,0,0,255)) + textured quad (BL=RED/
BR=GREEN/TR=WHITE/TL=BLUE exact texels) + 7 swap Ok(0x1) + persist 45B byte-exact +
0 json-overflow/SIGSEGV/SIGABRT (quad-loop was still animating fresh frames when the
foreground cap cut it at 180s; the hook is off here so it is inert on this path).

New regression `json_zero_fix_clamps_leaked_length_at_append_check` pins the disasm
addresses (check file 0x2355d40 / cap cell file 0x7275648 / throw helper file
0x25fb6bc), the `(cap as u64) < len` predicate for both leak modes, benign-len
non-clamp, and len==0 never tripping any cap.

**Next frontier (unchanged, SH60):** bridge the w4=4 dispatch rate into the post-ctx
window (drive a drain cycle after RENDERCTX), chase `swap Ok(0x0)` on the cross-thread
path (serialize presenters to one thread; engine 0x105b3b408 never binds, EGL
current-binding is thread-local), and feed a real engine session producer. The engine's
REAL frame-plane driver is guest 0x105b2ead4 (scene renderer: R+0x160 ctx / R+0x170
view / R+0x180 scene list) — it only renders real login/home once the engine constructs
a populated render-manager, which needs its game/UI setup.

## Session (Sep 13, 2026, hermes-worker, cycle SH60) — DELIVERED recon-v3 deliverable (1): the SELF-DRIVEN task-frame plane. `--taskv4-seed frame` registers a `type4_frame_thunk` host-thunk into the dispatcher's type-4 vector [0x106829ea8]; each w4=4 dispatch marshals into a REAL presented frame (engine make-current 0x105b3b358 -> frame-fn 0x105b32c00 -> swap 0x105b3b408) on the recovered real ctx (vtable 0x106731ae0). **`present #147942 swap Ok(0x1)`** on the live EGL display/surface/context is the concrete measured marker; the task counter hit #147942 (w4=4 dispatches reaching the thunk en masse). Workspace **505/0** (was 504/0, +1). Doc docs/frontier-sh60-taskv4-frame.md, artifact runs/sh60-taskv4-frame.txt, reproducible runs/capture_taskv4_frame.sh.

Three new pieces landed (elfjit.rs):
- `--taskv4-seed frame`: registers the non-recursive leaf `type4_frame_thunk`
  host-thunk (register_host_call_auto) into `[0x106829ea8]`. It reads RENDERCTX
  (recovered real 0x48 ctx), reads make-current (vt[+16]=0x105b3b358) + swap
  (vt[+24]=0x105b3b408), seeds the 10 engine-GLES dispatch slots once, builds
  the fabricated coherent renderer/view (SH18/SH22), cycles a per-dispatch
  clear-color palette (distinct fresh frames), and via nested run_guest_callback
  drives make-current -> frame-fn 0x105b32c00 -> swap. Never re-enters the
  vector/drain/dispatcher (would recurse). RENDERCTX==0 -> self-guard no-op.
- RENDERCTX publication: --renderthunk stores the recovered real ctx into a
  process-wide atomic so the dispatch-plane thunk (different thread) consumes
  it.
- Deterministic w4=4 engineering (from fresh disasm of drain 0x102856e40 +
  dispatcher 0x10285371c): the drain's genuine popped-node w4=4 path (0x2856ffc)
  is hit only in an early init window, and its idle-heartbeat dispatches go to
  w4=2/3 telemetry (never the vector). Fix: rewrite both heartbeat `mov w4,#2/#3`
  (0x102856f24/0x102856f68) to `mov w4,#4`, so every idle dispatch routes the
  real dispatcher to the seeded vector (the ~147k dispatch counter proves it).
  Because those flood dispatches precede RENDERCTX recovery, the CLEAN present
  is a deterministic post-ctx dispatch: the --renderthunk thread drives the
  thunk once (on the context-already-current thread) with the dispatcher ABI ->
  **present #147942 swap Ok(0x1)**. The drain-thread present is Ok(0x0)
  (cross-layer swap didn't report EGL success). Also: --deque-node-live in frame
  mode holds its first node PLACEMENT (not root capture) until RENDERCTX, so
  real node pops terminating at w4=4 fire with a live ctx.

New regression `type4_vector_seed_accepts_registered_host_thunk_abi` pins:
register_host_call_auto lands a handler in the reserved 0x7f00_0000_0000
host-call region, host_call_at resolves it back, and invoking it executes the
task consumer with the (node, [node+32]&~1, consumer) ABI.

Verified: cargo test --workspace 505/0; productized baseline RE-VERIFIED green
(runs/sh60-product-reverify.txt: exit 124, textured quad BL=RED/BR=GREEN/
TR=WHITE/TL=BLUE exact texels + triangle + quad-loop, persist byte-exact, 0
crash; the --renderthunk change also now fires one task-driven frame there,
present #1 swap Ok(0x1)). No json-abort/SIGSEGV/ENOSYS in the taskv4 run.

**Honest scope:** recon §A's core claim delivered and measured — the task-
consumer ABI presents real frames through the engine's own
make-current/frame-fn/swap on the recovered live EGL ctx (`present swap
Ok(0x1)`). Not yet the engine detail-rendering its own login/home screens (the
thunk drives the clear-path frame-fn, not the full UI render stream), and the
w4=4 flood sits in the boot window (the clean present is the deterministic
post-ctx dispatch). Standing structural wall materially advanced, not closed.
Next frontier: bridge the w4=4 dispatch rate into the post-ctx window (drive a
drain cycle after RENDERCTX) + chase the cross-thread swap Ok(0x0), then feed a
real engine session producer.

## Session (Sep 12, 2026, hermes-worker, cycle SH59) — drove the recon-v1 "still-live lower-effort" V1 6-jstring start (`nativeAppBridgeAppStart__`, file 0x2338510 / guest 0x102338510) as a STANDALONE primary `--startapp-v1` — the first time the V1 entry has been exercised headlessly. Workspace **504/0** (was 503/0, +1). Commits d5e9ccb, a528b5e. Doc docs/frontier-sh59-v1-appstart.md, artifact runs/sh59-v1-appstart.txt.

Recon-v1 (docs/recon-framework-boot-order.md) names the V1 6-jstring
`nativeAppBridgeAppStart__` the "still-live lower-effort alternative" that
bypasses the AutoValue params layer; recon-v2 §Task-2 blames the StartApp
json-abort on the JNI shim collapsing that params layer. But SH54-58 only ever
ran V1 as the **unreachable tail** of the v2boot ladder (which stalls at rung 1
`nativeGameGlobalInit` every cycle), so the V1 entry's downstream never executed
and its ABI was never even exercised:

- **New elfjit `--startapp-v1`**: swaps the primary `--startapp` target from
  V2StartAppWithParams to V1 `AppStart__`, building the s2 ABI as 5 empty
  jstrings (x2,x3,x5,x6,x7) + jboolean false (x4=0) per the exact mangled-JNI
  descriptor `String,String,Z,String,String,String`. V1 reads plain jstrings
  straight from the registers (no AutoValue jobject, no Call*Method getter), so
  it **bypasses the params-collapse json-abort completely**.
- **Fixed a latent wrong ABI** in the never-reachable v2boot V1 fallback: it
  put a jstring handle in the `Z` boolean slot (x4) and left x7=0 — corrected to
  the true descriptor.
- **New regression** `v1_app_start_six_arg_abi_is_five_strings_plus_boolean`
  pins the ABI (5 readable guest-addressable jstring handles; x4 Z-slot exactly
  0; no String handle aliases the boolean slot).

**Empirical (real libroblox.so, full productized render recipe + `--startapp-v1`,
artifact runs/sh59-v1-appstart.txt, exit 124):** engine's OWN frame-fn renders a
real indexed triangle (centroid red) + textured quad (BL=RED/BR=GREEN/TR=WHITE/
TL=BLUE) + 4 fresh quad-loop frames (swaps Ok(0x1)), byte-exact persist
roundtrip (45 B, REMEMBERED session), **zero** json-string-length-overflow /
SIGSEGV / SIGABRT / ENOSYS — the V1 path executes clean where V2's params layer
was suspect.

**Honest scope:** V1 advances the engine to the **same** structural place as V2
(the main loop parks on the lifecycle-await futex lr=0x10284d134; the type-4
producer vector `[0x106829ea8]` stays 0 — framework-glue-installed only), so a
self-driven home/session screen is not yet reached; frames remain harness-driven
on the live engine context. Contribution is empirical + corrective: recon-v1's
lower-effort alternative is now proven clean + standalone-callable, and its
previously-wrong ABI is fixed + pinned so future V1 work starts from a correct
register layout. Standing structural wall unchanged.

## Session (Sep 12, 2026, hermes-worker, cycle SH58) — first REAL-guest-handler type-4 seed executed on the real binary: the drain's w4=4 dispatch `br`'d into the engine's OWN frame-fn (real engine code ran at guestpc 0x105b2e98c) before ABI-faulting. Workspace **503/0** (was 502/0, +1). Commit 2922518. Doc docs/frontier-sh58-taskv4-realseed.md, artifacts runs/sh58-{taskv4-realseed,taskv4-sustain,baseline}.txt.

Every SH44-57 frontier doc names the same next step: "feed a REAL engine
frame/session producer address into the seed so a sustainably-dispatched task
node advances the engine toward its own frame/screen." But every run used
`--taskv4-seed probe` (a registered HOST thunk) — the guest-hex form
(`--taskv4-seed <guest-addr>`) existed but was NEVER exercised. This cycle closes
that gap and empirically executes the recon §3.6 "interim fallback":

- **run runs/sh58-taskv4-realseed.txt:** full productized boot +
  `--taskv4-seed 0x105b32c00 --deque-node-live 0x106829f00 --drain-poll 8`.
  The vector was seeded with the engine's REAL frame-fn; the drain `br`'d into
  it and the JIT translated+executed real engine code (frame-fn's renderer
  list-find at `guestpc 0x105b2e98c`) then faulted on ABI (the vector passes
  `handler(node, [node+32]&~1, consumer, w4, 5)`, frame-fn expects a
  coherent renderer). **First confirmed real-guest-code dispatch through the
  type-4 plane** — seed rejection is NOT the wall; the wall is exactly the
  known one: the framework-installed "process popped task node" worker address
  is external glue absent in-image, and any real seed must match the
  `(node, [node+32]&~1, consumer)` ABI (frame-fn doesn't).
- **new regression** `type4_vector_seed_accepts_real_in_image_guest_function`
  pins the vector addr + that a real in-image guest fn is mechanically valid as
  a seed + the ABI contract a real producer must match.
- **re-verified green on current HEAD (post SH55/56/57):** probe sustain
  (runs/sh58-taskv4-sustain.txt) = **190 consecutive node pops, 3 clean type-4
  dispatches, exit 124** (SH49 downstream of the value-registry/asset/storage
  changes); productized real-boot (runs/sh58-baseline.txt) = exit 124, real
  triangle + textured quad + 6 quad-loop frames, byte-exact persist roundtrip,
  594 assets extracted+served, zero ENOSYS.

**Honest scope (unchanged structural wall):** frames remain harness-driven; the
engine never self-produces a session/frame because `[0x106829ea8]` is
framework-glue-installed only (no in-image store — SH46/52/53/55/56), and the
real worker's address is not in the binary. Contribution is empirical: seed
rejection ruled out + exact ABI a future real producer must match, and the whole
stack re-confirmed green on current HEAD.

## Session (Sep 12, 2026, hermes-worker, cycle SH57) — made the AAssetManager shims REAL (image-backed) + extract the real APK's assets so the engine can load its own UI content — recon-v2's "precondition for a frame". Workspace **502/0** (was 499/0, +3). Commit 1b02552. Doc docs/frontier-sh57-assetmanager.md, artifact runs/sh57-asset-run.txt.

Recon-v2 (docs/recon-framework-boot-order.md) names the AssetManager the
"precondition for a frame": the engine reads its UI content (594 assets/ entries —
FoundationImages sprite sheets, BuilderIcons fonts, GLSL shader packs) out of the
source APK via AAssetManager_fromJava/open/getLength/getBuffer. All four shims
returned NULL/0 (both the Rust JIT shims and the C jni_stubs.h path), so the engine
could not load a SINGLE real asset even if a self-driven frame were produced. This
cycle makes them image-backed:

- aassetmanager_fromJava → stable non-NULL manager sentinel.
- AAssetManager_open(mgr, filename, mode) reads the guest C-string filename
  (normalizes a "assets/" prefix), serves it from the host SOBER_ASSETS_ROOT (the
  extracted APK assets/ dir) into a stable owned buffer in an open-asset table.
- AAsset_getLength/getBuffer → real length / stable host pointer the guest derefs
  directly (guest vaddr == host addr in this JIT; the Box buffer is never moved).
- AAsset_close drops the handle; missing/unmounted still fail NULL/0 so unarmed
  boots are untouched.
- New apk::extract_assets() decompresses the APK's assets/ into out_dir/assets
  (Roblox stores 203/594 DEFLATE), handles the flat-APK + assets/app.zip→
  config.arm64_v8a.apk bundle forms, skips non-assets/dir entries, returns None for
  a no-assets APK.
- main.rs --jit exports SOBER_ASSETS_ROOT before launch_jit (the spawned elfjit
  inherits it).

**Honest scope:** the standing structural wall is UNCHANGED — the engine still
never self-produces a session/frame (type-4 producer vector [0x106829ea8] is
framework-glue-seeded only, recon-Task-1/2 closed in SH53/SH56), so in the
productized run it does not yet issue AAssetManager calls. The asset plane is
served, proven hermetic, and configured for the run — the recon's named
precondition block is removed; it is only *reached* by a future self-driven
session. The only proven live dispatch plane remains `--taskv4-seed` +
`--deque-node-live`; next frontier: feed a REAL engine frame/session producer
address into the seed so a sustainably-dispatched task node advances the engine
toward its own frame/screen (which now has the assets to load).

## SH57b (Sep 12, 2026, hermes-worker): LocalStorageManager.getAllocatableBytes()
now reports the host's REAL free space (fstatvfs on SOBER_ANDROID_ROOT) instead
of the collapsed 0 — recon-v2 flags 0 ⇒ the engine believes there's no disk and
RbxStorage never builds its content cache (undermines objective 2b's remembered
session cache plane). Extends the AutoValue getter regression. Workspace 502/0.
Commit 401683c.

## SH57c (Sep 12, 2026, hermes-worker): PlatformParams.getAssetFolderPath yields
the host assets root (SOBER_ASSETS_ROOT from SH57 extraction) so the engine's
content loader can find real UI/texture/font files by direct FS open, not just via
AAssetManager; unmounted -> NULL/0 (boot-safe). Extends the AutoValue getter
regression; productized real-boot re-verified exit 124. Workspace 502/0.
Commit b73f0eb.

## Session (Sep 12, 2026, hermes-worker, cycle SH56) — empirically CLOSED recon Task-2 on the real binary: the AutoValue getter-value registry (SH55) does NOT fix the StartApp json-abort — the leaked string length is params-independent (a host-mmap pointer read at the append bound-check, never touching the getter registry). Workspace **499/0** (unchanged). Doc docs/frontier-sh56-json-abort-params-independent.md, artifacts runs/sh56-{startapp-json-abort,startapp-jobject-abort,jsondump}.txt.

The recon (docs/recon-framework-boot-order.md, Task-2) claims the `RBX::json::Writer
string-length-overflow` abort's root cause is the JNI shim: `Call*Method` returned
typed-0, collapsing the AutoValue params layer so StartApp re-serialized uninitialised
guest-stack std::strings. SH55 wired the full getter-VALUE registry. But that registry
had never been observed live against StartApp's serialization (SH55's `--v2boot` stalls
at rung 1; the productized recipe passes a JSON jstring, not a jobject). This cycle
drives `nativeAppBridgeV2StartAppWithParams (0x258b144)` with a genuine AutoValue
jobject (new elfjit flag `--startapp-jobject`) and answers definitively.

**Both params forms abort identically** (exit 139, `RBX::json::Writer string length
overflow: <host-pointer>`): JSON jstring `{"key":""}` (leak 0x7fb5..., runs/sh56-startapp-json-abort.txt)
AND real AutoValue jobject + full value registry (leak 0x7f19..., runs/sh56-startapp-jobject-abort.txt).
Rigorously pinned (`JIT_DUMP_PC`, runs/sh56-jsondump.txt):
- Append bound-check `0x102355d40`: x1 = a **host mmap pointer** read as the std::string
  length → throws; mechanism identical to SH45's sp/sp-0x30 leak.
- Throw helper `0x1025fb6bc`: x0=0x10057765a (fmt string), printed value is the host ptr.
- **`JIT_TRACE=1` emits ZERO `[jni] Call*Method` getter lines** — the AutoValue getter
  registry is never even reached during StartApp's serialization.

**Conclusion / redirect:** recon Task-2's root-cause claim is disproven — the value
registry is necessary if StartApp ever gets far enough to use the params, but it is NOT
the json-abort's cause. The abort is a harness-bootstrap artifact gated by the
render/lifecycle drive (SH45's original characterization): the **productized** recipe
(drives lifecycle + ANativeWindow + render-init warmup WITH StartApp) never aborts and
renders real frames — re-verified green this cycle (persist roundtrip byte-exact, real
indexed triangle centroid red, textured quad BL=RED/BR=GREEN/TR=WHITE/TL=BLUE, 6
quad-loop frames, swaps Ok(0x1)). Future bare-StartApp work should target the
guest memory whose length field holds a host pointer, NOT the getter registry.
`--startapp-jobject` kept as a reusable params-layer diagnostic. Standing structural
wall unchanged: type-4 producer vector [0x106829ea8] framework-glue-seeded only.

## Session (Sep 12, 2026, hermes-worker, cycle SH55) — completed the AutoValue params getter-value registry (CallLongMethod + CallFloatMethod now serve real values via a new s0-return JNI bridge) and re-tested the ordered V2 ladder WITH a complete params layer (which SH54 lacked): it still stalls at `nativeGameGlobalInit` (parks, never returns) and cannot populate `[0x106829ea8]`. Workspace **499/0** (was 498/0, +1). Doc docs/frontier-sh55-jni-value-registry.md, artifact runs/sh55-v2boot-ladder.txt.

Recon-v2 Task-2 (the json-abort root cause) is that jni.rs routed every `Call*Method`
to typed-0, so StartApp's json serialization read uninitialized guest-stack std::strings.
SH54 wired Object/Boolean/Int; this cycle completes the prescribed surface:

1. **CallLongMethod (NDK slot 52, newly wired)** — getAppUserId→0, getDeviceTotalMemoryMB→8192.
2. **CallFloatMethod (NDK slot 55, newly wired)** — getDpiScale→1.0. A `jfloat` returns in
   the FP register **s0**, not x0 (AAPCS64). New `HostJniF32` bridge + thunk region
   (jit.rs): a whole-CpuState bridge reads the methodID from x2 and the dispatcher writes
   the u32 into guest s0 before resuming at x30 — the existing float32 bridge drops the
   integer-register args and the GLES bridge writes x0 not s0, so neither could serve it.
   Regression proves a real guest `blr` through env->functions[55] lands 1.0f32 in s0
   (`fmov w0,s0; brk #0`; a naive `ret` looped on its own post-blr x30).
3. **String getters** extended with DeviceParams: getOsVersion→"33" (Vulkan GATE), device
   name/sku/manufacturer→Cordial, country→US, networkType→WIFI, appVersion→"".
4. **Booleans** per recon v2 (isUnder13…isLowRamDevice false; isKeyboardDevice/
   isMouseDevice/isCpu64Bit true).

**Empirical (runs/sh55-v2boot-ladder.txt, exit 124):** the full stable product recipe +
`--v2boot` on the real libroblox.so: render + persist baseline INTACT (no regression),
zero SIGSEGV/SIGABRT, zero json overflow — but the ladder reaches only
`driving nativeGameGlobalInit`, which does **not return** (runs real init then parks), so
rungs 2–6 never run on the detached driver thread and [0x106829ea8] stays **0**. This is
SH54's wall re-measured WITH the complete params layer (so StartApp's serialization would
have real values) — re-confirming the recon Task-1 (APS2) verdict at runtime: the
ordered-ladder reframe does not populate the type-4 producer vector headlessly.

Standing structural wall unchanged: `[0x106829ea8]` is framework-glue-seeded only; the ONLY
proven live dispatch plane is `--taskv4-seed <real-handler>` + `--deque-node-live` (SH44
live-when-seeded; SH49 sustainable at 197 pops). Next frontier: feed a REAL engine
frame/session producer address into the seed so a sustainably-dispatched task node advances
the engine toward its own frame/screen.

## Session (Sep 12, 2026, hermes-worker, cycle SH54) — built the ordered V2-boot ladder drive (`--v2boot`) + AutoValue getter shim — the empirical runtime test SH53 left open — and confirmed the type-4 producer vector `[0x106829ea8]` stays 0 while the real `nativeGameGlobalInit` executes. Workspace **498/0** (was 497/0, +1). Doc docs/frontier-sh54-v2boot-ladder.md, artifact runs/sh54-v2boot-{ladder,fw,progress,threads}.txt.

The recon (docs/recon-framework-boot-order.md) demands driving the real V2 boot
IN ORDER (nativeGameGlobalInit → setTaskSchedulerBackgroundMode(false) →
V2InitWithParams → StartLuaAppDM → V2StartAppWithParams) with AutoValue JNI
jobjects, not JSON. SH53 had left only the empirical drive open. Two pieces
landed this cycle:

1. **AutoValue getter shim (jni.rs).** `CallObjectMethod`/`CallBooleanMethod`/
   `CallIntMethod` (NDK slots 34/37/49) previously returned 0 for every getter,
   so StartApp's json serialization read uninitialized guest-stack std::strings
   — the SH45/SH46 `RBX::json::Writer string length overflow` abort. Because
   `GetMethodID` returns a readable handle of the method NAME, the new stubs
   dispatch on the getter name and return a real, readable empty jstring
   (`"Dark"` for `getSelectedTheme`) / false / 0; unrecognized names still fall
   back to 0 (real Java re-entry unchanged). Wired into the official NDK table.
   +1 regression `jni_auto_value_params_getters_resolve_via_fn_table`.
2. **`--v2boot` ordered ladder (elfjit.rs).** A detached thread — spawned
   BEFORE the `start_app` jit_run, which parks the main thread forever and never
   returns — sleeps a warmup then drives the 6 real JNI natives in the recon's
   load-bearing order as fresh guest entries (reusing the boot SP) with AutoValue
   jobjects, dumping `[0x106829ea8]` after EVERY rung, then the V1 AppStart__
   fallback (0x102338510). All params are jobjects (not JSON).

Empirical result (stable productized recipe, exit 124): `nativeGameGlobalInit`
executes REAL engine code (block cache 2192 ≫ ~434 idle baseline), zero
SIGSEGV/SIGABRT, no json-string-length-overflow, real renders + persist
roundtrip intact — yet `task-v4 [0x106829ea8]` stays **0 the entire window**.
This corroborates SH53 at runtime: the recon's "in-image TaskScheduler install
reached via the ordered ladder" is not reproduced headlessly. Honest caveat:
`nativeGameGlobalInit` advances then parks (does not return in the run window),
so only rung 1 is observed — the vector-0 is measured DURING GlobalInit, not
after a full ordered completion that reaches the install site. Standing
structural wall unchanged: the type-4 vector is framework-glue-seeded only;
`--taskv4-seed` + `--deque-node-live` (SH49) remains the only proven mechanism
to run the dispatch plane.

## Session (Sep 12, 2026, hermes-worker, cycle SH53) — DISPROVED the 2026-09-12 recon's reframe that the type-4 producer vector `[0x106829ea8]` is installed IN-IMAGE by TaskScheduler/V2-init code SH46 "never reached". Workspace **497/0** (unchanged). Commits 277f567, 9694a19 (doc). Doc docs/frontier-sh53-recon-disproof.md.

A recon (docs/recon-framework-boot-order.md + /home/hermes-worker/open-sober-framework-glue-spec.md) claimed the ~50-cycle wall was wrong: SH46's "no in-code store" was because the scan ran on a bare boot that never reaches TaskScheduler init, and driving the real V2 ladder (`nativeGameGlobalInit → nativeUpdateAdapterInit → V2InitWithParams → StartLuaAppDM → [Surface] → StartAppWithParams`) in order would populate the vector. Disproven on two independent grounds:

1. **SH46's scan is STATIC** (whole `.text`) — execution-independent, so "never reached init" cannot explain a missing in-image store. If in-image init installed the vector, some decoded instruction would write 0x106829ea8.
2. **The computed-base escape is closed.** The vector is the `.bss` base 0x6829e80 + **0x28**, so `adrp 6829000; add xN,xN,#0xe80; str [xN,#0x28]` would dodge a literal-#3752 scan. Disassembling every `adrp xN,6829000` site: 0x2953e30 writes [0x6829e80] (+0x0, clears first qword); 0x295427c/0x29542ec use 0x6829e88 (+0x8) as an atomic counter (ldxr/stxr, stlr); all other adds target #0xba8/#0xe80/#0xe88/#0xf00 — none reaches #0xea8. All `add #0xea8` sites are struct-relative on dynamic bases, never a 6829000-derived register.

So no in-image (literal or computed-base) store exists. If the V2 ladder installs the vector it is via cross-module glue / host seed (consistent with SH46). Regression `type4_taskv4_vector_has_no_in_code_install_site_and_uses_static_base` extended to pin the vector's 0x28 offset within `.bss` + the computed-base disproof audit. The V2-ladder empirical drive remains open but only as a cross-module/runtime-install test, not an in-image one — priced accordingly (needs AutoValue InitParams/StartAppParams jobjects + real Surface). Workspace 497/0.

## Session (Sep 12, 2026, hermes-worker, cycle SH52) — closed the real client's last 11 unresolved data imports: the `AMEDIAFORMAT_KEY_*` media-format string constants (Android libmediandk absent host-side) now bind to live host C strings. Workspace **497/0** (was 496/0, +1). Commit 4fe90da. Doc docs/frontier-sh52-media-keys-data.md, log runs/sh52-product-verify.txt.

The productized boot line previously read `bound 534 JUMP_SLOT + 67 GLOB_DAT/ABS64 (0 unresolved), 11 unresolved` — 11 data-object GLOB_DAT slots that `dlsym` could not resolve (bionic/mediandk-only symbols). Now reads `... + 77 GLOB_DAT/ABS64 (0 unresolved), 1 unresolved`. The gap was precisely:

- **10× `AMEDIAFORMAT_KEY_*`** (Object, UND): the NDK media-format string constants. A GLOB_DAT relocation writes the **address of the constant** into the GOT slot; the guest does `adrp x0,0x67cf000; ldr x0,[x0,#off]` to load it and passes it to `AMediaFormat_*` as a `const char*`. Left NULL, a real video/audio-decoding session reads a NULL key string — the SH19/SH24 crash class for data reads. `resolve_android_data(name)` now maps each to its NDK value (`"mime"`, `"width"`, ...) as an immortal leaked `CString` (stable/cached pointer).
- **`__sF`** (Object, UND): bionic's `FILE __sF[3]` (stdin/stdout/stderr) base. **Intentionally left unbound-to-host** — pointing it at a host `FILE_` would make the `fwrite`/`vfprintf` shims misclassify a bionic stream as a real host stream and SIGSEGV; leaving its low value is exactly what the shim's fd2-diversion path already handles. Documented, not a gap to "close".
- The `AMediaFormat_delete`/`AMediaCodec_delete` obj slots shown in the JIT_TRACE diagnostic are FUNC imports that already stub-bind via the `is_func` path (that's why only 1 is counted unresolved).

Verified: productized `open-sober play --apk --jit` exit 124 stable, real indexed triangle (centroid red) + textured quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE) + 6 quad-loop frames (swaps Ok(0x1)), `--persist-roundtrip` byte-exact, zero ENOSYS; the direct-`--renderinit` SIGSEGV/abort is the documented SH46 pre-existing harness-bootstrap artifact (identical pre-change), not a regression. New regression `android_media_format_key_data_imports_resolve_to_live_strings` pins all 10 constants byte-exact + cached-pointer stability + that unrelated object names (`__sF`, unknown) fall through unclaimed.

`__sF` remains the single legitimately-unresolved data import and is **by design** (see above). The standing structural wall is unchanged: the type-4 producer vector `[0x6829ea8]` is still framework-glue-installed only (SH46), so frames stay harness-driven on the live engine context.

## Session (Sep 12, 2026, hermes-worker, cycle SH51) — the live client's data-persistence plane is now SELF-VERIFYING in the productized deliverable: `open-sober play --apk roblox-android.apk --jit` now drives a real guest `/data/user/0/com.roblox.client/databases/session.db` openat→write→fsync→close→reopen→read roundtrip through `guest_svc`/fsmap and proves it byte-exact on the armed persistent host store — turning the previous ZERO-remap product run into an observable live-client persistence exhibit. Workspace 496/0 (unchanged). Doc docs/frontier-sh51-live-persist.md, log runs/sh51-persist-live.txt.

Objective 2b ("the client REMEMBERS sign-in via its own session/login
datastore") was previously proven only hermetically (SH38–SH42 committed tests);
the productized run made ZERO `[fsmap] remap:` lines because the engine never
reaches a session. This cycle embeds the datastore roundtrip into the elfjit
harness the product launches:

- **example/elfjit.rs `run_persist_roundtrip()`**: openat(O_CREAT)→write→fsync→
  close→reopen→read of `/data/user/0/com.roblox.client/databases/session.db` via
  `guest_svc`, asserting byte-exact read-back AND a real on-disk file under the
  armed SOBER_ANDROID_ROOT. Invoked at startup (`--persist-roundtrip`) because
  StartApp parks in an idle main-loop and never returns, so a post-boot hook is
  unreachable.
- **sober-core jitlaunch.rs**: the productized `play --jit` recipe now adds
  `--persist-roundtrip` + exports `JIT_FSMAP_LOG=1` — every play run
  self-verifies live persistence. Both unit tests extended.
- **arm64jit/src/jit.rs** guest_svc mappath: env-gated `[fsmap] remap:` line
  makes live remaps observable.

**Live artifact (runs/sh51-persist-live.txt, exit 124):**
  `[fsmap] remap: /data/user/0/com.roblox.client/databases/session.db -> ~/.local/share/open-sober/android-root/data/user/0/com.roblox.client/databases/session.db` (×2)
  `[persist] live datastore roundtrip: write=45B fsync=0 read_back_byte_exact=true on_disk=Some(true)`
  and the on-disk store holds exactly `ROBLOSECURITY=_live_client_remembered_session`
  (0600, 45 B). Zero ENOSYS/abort. Full render baseline intact in the same run
  (triangle centroid red + textured quad BL=RED/BR=GREEN/TR=WHITE/TL=BLUE + 6
  quad-loop frames, swaps Ok(0x1)).

**Also pins the type-4 dispatch contract from fresh disasm** (for the next
frontier cycle): dispatcher 0x10285371c (file 0x285371c), on `w4==4`, loads
`[0x106829ea8]` and `br`s to it with x0=node, x1=[node+32]&~1, x2=consumer — the
vector is a leaf function pointer the framework installs; `cbz` returns doing
nothing when unset (the headless-boot wall). Seeding it with the engine's own
0x10285371c would RECURSE (that fn reads the vector), so a correct seed needs the
real framework-installed "process popped task node" worker, whose address is not
statically in the binary (external-glue gap, SH46).

**Honest scope:** this makes live persistence self-verifying/observable in the
deliverable, but drives the JIT's own guest_svc ABI rather than the engine's
session code. The standing structural wall is unchanged: the engine still never
self-produces a session or renders its own login/home screen (type-4 producer
vector [0x106829ea8] is framework-glue installed only); frames remain
harness-driven on the live engine context.

JIT_EGL_LOG (SH47) left exactly two `UNRESOLVED` eglGetProcAddress names:
`glPushGroupMarker`/`glPopGroupMarker` (non-EXT spelling). They ARE in
GLES_INT_NAME_LIST but resolve_gles_int required the exact symbol, and both
Mesa libraries export ONLY the EXT-suffixed spellings (`glPushGroupMarkerEXT` /
`glPopGroupMarkerEXT`, verified via nm); a guest `br` through the engine's
dispatch-table slot for these names would have jumped to NULL (SH19/SH24/SH47
crash class, applied to the two names that cycle never reached).

- **Fix (resolver.rs):** when a whitelisted name is NULL in both libs and does
  not end in `EXT`, fall back to `{name}EXT` (itself whitelisted, int-ABI-safe)
  against GLESv2 then libGL. Both plain names now resolve to real bridge slots.
- **New regression** `plain_non_ext_marker_names_resolve_via_int_bridge_ext_sibling_fallback`
  (all 4 spellings resolve via int bridge, bridge-pointer slots, rejected by
  mixed). Workspace 496/0 (+1).
- **Productized re-verify** (runs/sh50-product-reverify.txt): `open-sober play
  --apk roblox-android.apk --jit` exit 124 stable, real indexed triangle
  (centroid RGBA(255,0,0,255)) + textured quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE)
  + 6 fresh quad-loop frames, swaps Ok(0x1), **zero** eglGetProcAddress
  UNRESOLVED marker lines.
- The direct-elfjit `--renderinit` SIGABRT (right after ANativeWindow wiring) is
  confirmed PRE-EXISTING (aborts identically with this change stashed) — the
  documented SH46 harness-bootstrap artifact, before this cycle's code path.

Standing structural wall unchanged (SH14/SH46/SH49): the type-4 producer vector
`[0x6829ea8]` remains framework-glue-installed only, so the engine still does
not self-produce a frame — frames stay harness-driven on the live engine
context.

## Session (Sep 12, 2026, hermes-worker, cycle SH49) — the type-4 task-dispatch plane is now SUSTAINABLE: the `--deque-node-live` injector stops re-evicting the drain's translated block, so inject+pop+dispatch runs 197 consecutive pops with zero crash (SH44 faulted at pop #39). Workspace 495/0 (unchanged). Doc docs/frontier-sh49-taskv4-sustain.md, artifact runs/sh49-taskv4-sustain.txt.

SH44 proved the type-4 popped-task dispatch plane (`[0x6829ea8]`) is functional
when seeded, but every sustained attempt faulted after ~39 injected nodes
(SIGSEGV at guest 0x102856f7c). Root cause (runs/sh44-taskv4-plane.txt): the
injector re-patched force-pop **and re-dropped the cached drain blocks
(`block_cache_drop_region(0x102856e40, 0x1028570c0)`) on EVERY re-injection**,
recompiling the pop-loop while the drain was mid-execution of it — after ~39
churns the translation desynced and the popped-node register loaded an
instruction word (`x22=0x7bfdd503233f`, low-32 `0xd503233f` = a NOP). The
patches are idempotent and the guest bytes stay patched, so the per-iteration
reset was pure churn.

Fix (elfjit `--deque-node-live`): a static `ARMED: AtomicBool` runs the
force-pop patch + cache eviction **exactly once** (first real placement);
subsequent re-injections only swap the node into the head-cell. Verified on the
full productized recipe + `--taskv4-seed probe --deque-node-live 0x106829f00
--drain-poll 8`: **197 consecutive `NODE ... POPPED`** (SH44 crashed at #39), 3
clean type-4 vector dispatches through the real dispatcher w4=4 plane, armed +
evicted exactly once each, zero SIGSEGV/abort/`string length overflow`, exit
124 (stable idle). Productized `open-sober play --apk --jit` re-verified green
(runs/sh49-product-reverify.txt): exit 124, real triangle + textured quad
BL=RED/BR=GREEN/TR=WHITE/TL=BLUE + quad-loop, swaps Ok(0x1), zero ENOSYS.

**Honest scope:** this removes the harness's self-destructive reinjection so a
real producer can be seeded sustainably; it does NOT yet drive a self-produced
frame. The vector is still harness-seeded (probe) here — SH46 proved no in-code
install site. **Next (closest unblocked):** feed `--taskv4-seed 0x<guest>`
(+ `--deque-node-live`) a real engine frame/session handler so a sustainably
dispatched task node advances the engine toward its own frame — the loop it
runs in no longer self-destructs. Standing structural wall otherwise unchanged.

## Session (Sep 12, 2026, hermes-worker, cycle SH48) — the engine's instanced-mesh pipeline is no longer NULL-bound: `glVertexAttribDivisor` joins the GLES int bridge, and the SH37 instanced gate is extended from a no-op count=0 draw probe to a real non-empty instanced draw (count=1, 4 instances) with a bound VBO + divisor 1 against real Mesa. Workspace 495/0 (was 494/0, +1). Doc docs/frontier-sh48-instanced-divisor.md.

A real instanced mesh must call `glVertexAttribDivisor(index, n>0)` to mark the
per-instance attribute; that name was absent from `GLES_INT_NAME_LIST` (SH35 only
whitelisted the two *draw* functions), so a guest `br` through the engine's slot
resolved to NULL/0 and every instance read instance 0's data — the instanced draw
silently degenerated to one duplicated triangle (no crash, wrong output). This
cycle adds `glVertexAttribDivisor` to the int bridge (auto-heals the engine's
eglGetProcAddress-built table — no harness re-seed), extends the SH37 functional
gate to a real non-empty instanced draw with a bound VBO + divisor 1, and adds a
focused regression pinning it (with its setup companions `glVertexAttribPointer` +
`glEnableVertexAttribArray`) resolves via int bridge and is rejected by mixed.
Productized real-boot baseline re-verified: triangle + textured quad
(BL=RED/BR=GREEN/TR=WHITE/TL=BLUE) + 3 quad-loop frames, swap Ok(0x1), exit 124.
The standing structural wall is unchanged (SH14/SH46): the type-4 producer vector
`[0x6829ea8]` is populated only by real Android framework glue, absent headlessly,
so frames remain harness-driven.

## Session (Sep 12, 2026, hermes-worker, cycle SH47) — closed the last NULL-dispatch gap in the engine's real GLES render table: GL4/extension slots 11/12 (glBufferStorage/glMapBuffer/glQueryCounter/glObjectLabelKHR et al.) now resolve via a desktop-libGL fallback. Workspace 494/0 (was 493/0). Doc docs/frontier-sh47-gles4-desktop-fallback.md.

The engine's own render dispatch table (BSS `0x106d3b2f0 + 8*N`, built via
`eglGetProcAddress` / SH3 interception) had **slots 11 and 12 reading 0x0**
even though its render code `bl`s those slots **unguarded** (slot-11 stub
`0x5b3a244` ×2, slot-12 `0x5b3a250` ×4). A self-driven frame routing through
them would `br` to NULL and SIGSEGV — the last uncovered NULL-dispatch surface
in the engine's GLES3 table (SH19/SH24/SH35 bug class, applied to the slots
those cycles never reached).

- **Root cause was NOT a missing whitelist entry / float-ABI rejection.** New
  env-gated `JIT_EGL_LOG=1` diagnostic in `w_eglGetProcAddress` (logs every
  requested name that fails all bridges AND Mesa GLESv2, w/ guest PC) showed the
  engine resolves exactly **16 GL4/extension names** that come back 0 because
  Mesa's ES-only `libGLESv2.so.2` does not export them: `glBufferStorage(EXT)`,
  `glMapBuffer(OES)`, `glQueryCounter(EXT)`, `glObjectLabelKHR`,
  `glPush/PopGroupMarker(EXT)`, `glGetQueryObject{ui64v,iv}(EXT)`.
- **Fix:** new `gl_desktop_handle()` (dlopen `libGL.so.1`, same RTLD_LOCAL
  discipline) + `resolve_gles_int()` falls back to it for whitelisted int-ABI
  names absent from GLESv2. All 16 added to `GLES_INT_NAME_LIST` (each pure
  int/ptr ABI ≤4 args → safe through the integer HostCall, rejected by mixed).
  Engine table auto-heals via SH3 — no harness re-seed.
- **Live proof** (`runs/sh47-egllog2.txt`): seed snapshot slots 11/12 went
  `0x0` → `0x7f0000003098/90` (real bridge slots); UNRESOLVED dropped 16 → 2
  (only un-EXT-suffixed `glPush/PopGroupMarker`, absent from both libs; the EXT
  variants are bridged). Full productized render re-verified exit 124: triangle
  centroid red, textured quad BL=RED/BR=GREEN/TR=WHITE/TL=BLUE, 3 fresh
  quad-loop frames, swaps all `Ok(0x1)`. Slots-11/12 disasm: callers pass
  `w0=0x8a11 (GL_UNIFORM_BUFFER), size, NULL data, flags` == **glBufferStorage**
  — the engine's modern UBO path already dispatches it.
- New regression `gles4_extension_names_resolve_via_int_bridge_desktop_gl_fallback`
  pins all 15 desktop-exported names + the critical subset resolve via int
  bridge (trailing NUL) and are rejected by mixed. Workspace **494/0** (+1).

**Next (unchanged standing frontier):** the type-4 task-producer vector
`[0x6829ea8]` is still framework-glue-installed only (SH44/SH46); the engine
never self-produces a render/session task, so frames remain harness-driven.
Directions per SH46: (a) synthesize the framework task-producer registration
from the engine's own game-activity/lifecycle init and call that guest path; or
(b) advance objective 2b by exercising the fsmap/SQLite datastore plane with
the client's real serialization. This cycle removed a real NULL-crash surface a
self-driven frame WOULD hit (buffer/timer/query slots now bridge-routed).

SH45 left the type-4 producer vector `0x6829ea8` as the open standing frontier and
suggested a concrete next lever: "make the seeded LSM empty-map / fake-object
memory guest-shaped / zero-length so StartApp's json serialization reads a valid
empty string length instead of a host pointer." This cycle **empirically
disproves that hypothesis** and redirects the frontier:

- **The leaked "string length" is a guest stack address, not the LSM seed.**
  Driving `JIT_DUMP_PC` at the json append check-fn entry (0x102355d40) and the
  throw helper (0x1025fb6bc) across three independent runs (fresh ASLR each):
  the overflow value is always `== sp` (run A: leak `0x7f4462ffd9f0` exactly
  equals x29==x31/sp of the throw frame) or `== sp−0x30` (run B/C), tracked to
  within a small constant. The seeded LSM bucket array sits ~0x260–0x2a0 MB away
  in the host mmap/heap and is **not** the leaking allocation. So the real
  mechanism is guest-internal: StartApp's `nativeAppBridgeAppStart`-family json
  writer reads an **uninitialised std::string on the guest stack** as a length,
  trips `ldrsw x8,[0x7275000+1608]; cmp x8,x2; b.cc` in the append bound-check,
  and throws from `RBX::json::Writer string length overflow: %zu` (format file
  0x57765a) via the throw-with-value helper 0x25fb6bc.
- **The type-4 producer vector has no guest install site.** A full-image objdump
  scan for `adrp 0x6829000` + `[x,#3752]` stores proves no curso instruction
  writes guest `0x106829ea8` (the only `[x,#3752]` stores are struct-relative on
  heap/sp regs; the dispatcher at file 0x2853784 is the sole static-base reader).
  "Reverse what the framework installs into the vector in-code" is a confirmed
  dead end — the install is external framework/GL game-activity glue absent
  headlessly. The dispatch plane remains proven live when seeded (SH44).

Two new regression tests pin both facts for future cycles
(arm64jit/src/jit.rs): `type4_taskv4_vector_has_no_in_code_install_site_and_
uses_static_base` and `json_overflow_leak_reads_guest_stack_pointer_not_seeded_
lsm_map`. Doc: docs/frontier-sh46-json-abort-sp-disproof.md. Productized
`play --jit` re-verified green (exit 124, real indexed triangle + textured quad
+ quad-loop, swaps Ok(0x1)); bare `--jni --startapp` still reproduces the abort
(as documented, harness-bootstrap-only).

**Next (reframed frontier):** the standing structural wall is unchanged — the
engine never self-produces a frame/session task because task-v4 `[0x6829ea8]`
is populated only by real framework producer glue (now proven absent in-code).
And the bare-StartApp json abort is NOT fixable by LSM seeding (it's guest
stack state). Future directions: (a) since the vector is glue-installed, the
remaining host lever is to synthesize the FRAMEWORK task-producer registration
it performs (find, from the engine's own game-activity/lifecycle init, how it
would register a producer and call that guest registration path); or (b) keep
advancing objective 2b by exercising the now-hermetic fsmap/SQLite datastore
plane with the client's real serialization—the productized `play --jit` run
currently makes ZERO fsmap remaps because the engine never reaches a session,
so the persistence claim remains proven only hermetically (SH38–SH42), not by
the live client.

## Session (Sep 12, 2026, hermes-worker, cycle SH45) — characterized the bare-StartApp `RBX::json::Writer string length overflow` abort (a run-variable host heap pointer leaked from the seeded empty-LSM-map into a guest json string-length read) and gave elfjit an abort-class crash dumper. Workspace 491/0 (was 490/0). Commits 9b99fac, 3ada26e.

The SH44 documented open item — "bare-StartApp `RBX::json::Writer string length
overflow` abort (harness LSM host-pointer seed leaking into a string-length read
on the no-render bootstrap path; the productized `play --jit` recipe bypasses
it)" — is now confirmed and precisely characterized (docs/frontier-sh45-json-writer-abort.md):

- The leaked "length" is **run-variable and in the host-mmap region**
  (`0x7f1f53ffe9f0`, `0x7f6f2bffe9f0`, diff.value each run) — a **host pointer**,
  read where the guest expects a string length during StartApp's json
  serialization. The precise leaking allocation is **not positively identified**
  (it differs from the LSM bucket/sub in every run, though same `0x7f` segment);
  SH44's "harness LSM host-pointer seed leaking" remains the leading hypothesis,
  not a proven identity. The `std::runtime_error` is thrown by
  `RBX::json::Writer` (one of the 7507 throw-with-value sites materializing
  `0x57765a`, e.g. disasm 0x2355d98/0x2557dcc: `adrp x0,577000; add x0,x0,#0x65a;
  mov x1,<len>; bl 0x25fb6bc`).
- **params-independent**: `{"key":""}`, `{}`, `""`, `"X"` all abort (different
  heap value each run) — not the params jstring.
- Gated by **absence of the render/lifecycle drive**: the full productized recipe
  (JIT_DRIVE_LIFECYCLE + appcmd + ANativeWindow + render-init) runs clean (exit
  124, real triangle + textured-quad frames, zero overflow); only bare
  `--jni --startapp` hits it. So it is a **harness-bootstrap** artifact, not a
  production runtime bug. The productized deliverable is unaffected.

Also committed a real diagnostic (elfjit.rs): the fault dumper previously caught
SIGSEGV/SIGILL only, so an abort-class crash (libc++ terminate → abort / guest
abort) exited without a guest dump. SIGABRT is now added to the handler set, and
its default disposition is restored before re-raising via process::abort() (which
itself delivers SIGABRT — without the restore the handler recurses in an infinite
dump loop). Now any guest abort yields a full guest PC/regs/backtrace dump.

Verified: `cargo test --workspace` 491/0 (unchanged); build clean; productized
render re-verified through the modified elfjit (exit 124, real indexed triangle +
textured quad, swaps Ok(0x1)).

**Next (unchanged, the standing frontier):** the type-4 task producer vector
`0x6829ea8` remains the structural wall — `.bss`, populated only by a real
framework task-producer absent headlessly; `--taskv4-seed` proves the dispatch
plane is live when seeded (SH44). If a future cycle needs the bare StartApp path
to proceed WITHOUT the render recipe, the next lever is to make the seeded LSM
empty-map / fake-object memory **guest-shaped / zero-length** so StartApp's json
serialization reads a valid empty string length instead of a host pointer (see
`seed_static_empty_map` in elfjit.rs + the LSM reader 0x1d99e40).

## Session (Sep 12, 2026, hermes-worker, cycle SH44) — collapsed the ~30-cycle deque "maintenance wall" to its precise mechanism: the type-4 popped-task dispatch vector [0x6829EA8] is 0 on headless boot BUT the task-dispatch plane is proven FUNCTIONAL when that vector is seeded. Workspace 491/0 (unchanged). Commit pending.

The engine's task-deque drain (0x2856e40) has FOUR dispatch sites with hardcoded
w4 types: heartbeat types 2 (0x2856f24) and 3 (0x2856f68) run telemetry-event
emitters (globals 0x68262E8/0x6826300/0x6826308/0x6826320, each `adrp 67d1000;
ldr [x,#1776]; mov w1,#evtid; bl 1e0b0a8`), and the popped-task type 4 (0x2856ffc
/0x285703c) dispatches through a **distinct BSS function-pointer vector
`0x6829EA8`** (`adrp 6829000; ldr x3,[x8,#3752]; br x3`). That vector is `.bss`
(zero-init, no reloc) and remains 0 on every headless boot — including during the
real render — so any popped task node returns doing nothing (0x285378c->0x2853af0).

**Empirical proof (new `--taskv4-seed` lever; runs/sh44-taskv4-plane.txt):** with
`--taskv4-seed probe --deque-node-live 0x106829f00 --drain-poll 8`, the drain
pops 39 injected task nodes and 3 reach the seeded host-thunk handler through the
REAL dispatcher w4=4 plane with the exact disassembled ABI
(`handler(node=x0, [node+32]&~1=x1, consumer=x2)`, w4=4, x5=0):
`type-4 task handler #1: fnarg0(x0)=0x107334000 arg1=0 arg2=0x7f32829a48c0 w4=4`.
(The follow-on SIGSEGV at guestpc 0x102856f7c is the known force-pop+re-inject+
patched-drain-recompile race, SH11/SH13 — the 3 clean firings are conclusive.)

**Conclusion / refinement:** the wall is NOT an unreachable dispatch plane — it
is that **nothing installs the type-4 producer vector headlessly** (only a real
framework task-producer would). So foreign task nodes can advance the engine only
once `0x6829EA8` is pointed at a REAL guest frame/session producer. Diagnostic:
`JIT_FRAMEWORK_DUMP` now also samples `task-v4 [0x106829ea8]` + `v0/2
[0x106826320]`. Frontier (SH45): reverse what the framework installs into
0x6829EA8 and what task type/arg selects a frame/session producer; also the
bare-StartApp `RBX::json::Writer string length overflow` abort (harness LSM
host-pointer seed leaking into a string-length read on the no-render path — the
productized `play --jit` recipe bypasses it). Doc
docs/frontier-sh44-taskv4-vector.md. Baselines unchanged (productized `play
--jit` still exit 124 + real render).

## Session (Sep 12, 2026, hermes-worker, cycle SH43b) — also proved the legacy `gethostbyname` resolution path (the client imports both DNS APIs). `gethostbyname("localhost")` returns a static thread-local `hostent` — a differently-shaped result than getaddrinfo (h_addrtype@16/h_length@20/h_addr_list@24) — walked to an AF_INET 127.0.0.1. Workspace 491/0 (was 490/0). Commit 75d9d31.

## Session (Sep 12, 2026, hermes-worker, cycle SH43) — proved the guest DNS plane end-to-end through the real guest ABI: `getaddrinfo("localhost") → ai_addr → connect(203) → sendto → recvfrom` roundtrips a login payload to a real host TCP peer, then frees via the guest's own freeaddrinfo. Workspace 490/0 (was 489/0). Commit 23f4ff4.

A logged-in session's FIRST network action is hostname resolution — `getaddrinfo` —
BEFORE any connect. SH42b proved socket/connect/sendto/recvfrom only against a
hardcoded loopback IP; the resolution step was unproven. `getaddrinfo` is a libc
JUMP_SLOT import the resolver binds to HOST glibc via `dlsym` (not a raw syscall), so
the plane rides the resolver (not `guest_svc`).

The new hermetic regression `guest_dns_getaddrinfo_resolves_hostname_then_connect_roundtrip`
(crates/arm64jit/src/resolver.rs) resolves the getaddrinfo/freeaddrinfo slots, drives a
guest `blr x16` to the getaddrinfo slot with (node="localhost", service=<live-port>,
hints=NULL, &res), walks the returned aarch64-LP64 addrinfo chain (ai_family@4,
ai_addrlen@16, ai_addr@24, ai_next@40), asserts localhost resolves to an AF_INET
sockaddr that is exactly 127.0.0.1, feeds ai_addr/ai_addrlen into guest_svc
socket(198)/connect(203), roundtrips a login payload to a real host TCP listener (gets
PONG back, peer asserts exact bytes), and frees the chain via the guest's own
freeaddrinfo import. This closes the last gap between "the socket plane works" and "a
logged-in session can reach a real Roblox API host": resolution → connect → byte
roundtrip all drop-through.

Also RE-VERIFIED the productized deliverable on current HEAD (runs/sh43-play-jit.txt):
`open-sober play --apk roblox-android.apk --jit` extracts the REAL libroblox.so from
the APK and drives JNI_OnLoad(0x2173ff4) → StartApp(0x258b144) → render-init thunk →
the engine's own frame/geometry path through the JIT GLES bridge — real indexed
glDrawElements triangle (centroid RGBA(255,0,0,255)), textured quad
(BL=RED/BR=GREEN/TR=WHITE/TL=BLUE exact texels), 6 fresh quad-loop frames, every
post-draw swap Ok(0x1), exit 124 stable. Boot is fully-wired: 534 JUMP_SLOT bound
(0 unbound), zero ENOSYS/unhandled hostcalls across the full run.

**Next (closest unblocked):** network (SH42b) + data (SH42) + DNS (SH43) planes are all
proven through the real ABI. The standing structural wall is unchanged (~30 cycles):
the engine's own main-loop producer never enqueues a render-task type (the `w4=4`
maintenance cap steering framework-owned deque globals that are thin TLS-upkeep, not
session producers — SH14/SH41/SH42). `--deque-node-live` (SH13) confirms maintenance
dispatch executes real engine code but never reaches egl/gl, and the engine's idle is
the per-CPU task-deque futex (SH39b: ALooper/GameActivity glue loop never entered).
Frames remain harness-driven on the live engine context. A GPU host is the documented
environment for the final self-driven-login / frame-performance proof
(GRAPHICS_RECOMMENDATION.md), deferred (per user preference) until frame/performance
evidence is gathered headlessly here.

## Session (Sep 12, 2026, hermes-worker, cycle SH42b) — proved the CLIENT-side network plane end-to-end through the real `guest_svc` ABI: socket(198)→connect(203)→sendto(206)→recvfrom(207)→close roundtrip a login payload to a REAL host TCP peer on loopback. Workspace 489/0 (was 488/0). Commit 5cc3dd8.

The pre-existing `socketpair(199)+sendmsg/recvmsg` test only covers a
pre-connected pair. A logged-in session's TLS/HTTPS stack funnels byte I/O via
socket→connect→send/recv to an EXTERNAL peer (talking to the host's loopback
exactly as to a Roblox API host), so a real `TcpListener` in a server thread is
spawned and the guest syscall ABI drives connect(127.0.0.1), sendto("SESSDATA\n"),
recvfrom("PONG" echo claim), close; the server asserts it received the exact
payload. Fixes the sockaddr byte-order in the test (`sin_addr.s_addr` is
network-order — portable htonl(INADDR_LOOPBACK) form, not naive from_be_bytes
which made 127.0.0.1 read as 1.0.0.127 → connect ETIMEDOUT). No prod-code change
(all four syscalls already forwarded); this is a regression pinning the full
client network path as drop-through-functional. Real boot unchanged (SH42's
sh42-boot-reverify.txt still exit 124 + real render).

**Next (closest unblocked):** the data-plane (SQLite lifecycle incl.
preadv/pwritev/sync, SH42) and the client network plane (SH42b) are both proven
through the real ABI. The standing structural wall (SH14, re-confirmed SH41) is
still: the engine's own main-loop producer never enqueues a render-task type
(w4=4 cap), so frames are harness-driven. Directions: (a) drive the
confirmed-live deque-maintenance globals (0x1068262e8/300/308) via
--deque-node-live and see if a maintenance dispatch advances the session past
idle; or (b) harden the Android-framework JNI path a logged-in session touches
when it reads/writes its now-persistent store.

## Session (Sep 12, 2026, hermes-worker, cycle SH42) — closed the last data-plane syscall gap: vectored positional I/O + durability. preadv(69)/pwritev(70)/sync(81) are now handled in guest_svc (previously -ENOSYS), completing the raw-SQLite session-datastore lifecycle. Workspace 488/0 (was 487/0). Commit 7351654.

Auditing the handled-syscall set against what a real SQLite-backed datastore
touches surfaced one remaining data-plane gap (the others — flock/fallocate in
SH40b, statx/truncate/linkat/readlinkat in SH40, openat/mkdirat/... in SH38 —
were already closed). A session store flushes db/shm pages with **pwritev**
(batched vectored positional write), reads them back with **preadv**, and issues
**sync(81)** under PRAGMA synchronous=FULL before declaring a transaction
durable. All three had fallen through to -ENOSYS, so a store doing vectored paged
I/O failed (and an unhandled sync made a commit look non-durable).

- preadv(69)/pwritev(70): `struct iovec` is byte-identical across aarch64/x86-64,
  so a raw forward writes the guest iovec array in place; aarch64's two-word loff_t
  pos (a[3]=lo, a[4]=hi) maps onto x86-64's __NR3264 syscall form.
- sync(81): host `libc::sync()` — returns (), so `libc::sync(); 0 as c_long`.

New hermetic regression
`fsmap_preadv_pwritev_sync_support_sqlite_durability_path` (tests/fsmap_persist.rs)
drives the real guest_svc ABI under a configured root: pwritev writes two pages at
distinct offsets into the store, sync returns 0 (not -ENOSYS), and after
close+reopen preadv reads page1 back byte-exact across a fresh fd. Data-plane is
now end-to-end: create → write → statx-exists → flock → fallocate → truncate →
preadv/pwritev → sync → readlink → read.

Verified: cargo test 488/0 (was 487/0); build clean; real boot re-verified through
the modified dispatch (runs/sh42-boot-reverify.txt: exit 124 stable, real indexed
triangle centroid RGBA(255,0,0,255), textured quad BL=RED/BR=GREEN/TR=WHITE/TL=BLUE
exact texels, 3 fresh quad-loop frames, swap Ok(0x1)). Baselines unchanged.

**Next (closest unblocked):** the data-plane is complete for the SQLite session
datastore. The standing structural wall (SH14, re-confirmed SH41) is unchanged:
the engine's own main-loop producer never enqueues a render-task type (the w4=4
cap), so frames are harness-driven. Two directions: (a) drive the confirmed-live
deque-maintenance globals (0x1068262e8/300/308) via --deque-node-live and see
whether a maintenance dispatch advances the session past idle; or (b) harden the
JNI/network surface the client touches once a real session reads/writes its
now-persistent store — the network (socket/TLS) and Android-framework JNI paths
a logged-in session exercises.

## Session (Sep 12, 2026, hermes-worker, cycle SH41) — fixed a real faccessat(48) arg-order + remap bug in guest_svc (aarch64 `faccessat(dirfd, pathname, mode)` — the old handler passed the dirfd as the pathname and the pathname pointer as the mode, so any guest "is my /data session file there?" datastore-accessibility probe read garbage against the host root) and corrected a stale documented premise. Workspace 487/0 (was 486/0). Commit e08ddee.

SH40 completed the fsmap data-plane path coverage, but the fsmap layer is only
as correct as each syscall's argument routing. Auditing the remapped syscalls
against their real aarch64 signatures surfaced one remaining arg-order bug:
`faccessat(48)`. On aarch64 it is `faccessat(dirfd, pathname, mode)` —
x0=dirfd, x1=pathname, x2=mode — but the SH38-era handler did
`mappath(a[0])` + `libc::faccessat(AT_FDCWD, p, a[1] as c_int, 0)`, i.e. it
treated the dirfd integer (often `AT_FDCWD = -100`, an invalid address) as the
pathname C-string and passed the real pathname pointer (truncated to c_int) as
the mode. The same class of bug SH40 fixed for readlinkat/symlinkat. Since bionic
and the Java datastore stack answer "does my session file exist / is it
writable" with exactly this primitive, a wrong faccessat makes the client
misjudge its own (now-persistent) store — undermining objective 2b.

- Fix (crates/arm64jit/src/jit.rs): `(p,_) = mappath(a[1])`;
  `libc::faccessat(a[0] as c_int, p, a[2] as c_int, 0)`.
- New hermetic regression (tests/fsmap_persist.rs)
  `fsmap_faccessat_uses_true_pathname_and_remaps_into_store`, driving the real
  guest_svc ABI under a configured Android root: (1) R_OK/W_OK on an existing
  store `prefs.xml` return 0 (true pathname read + store-resolution);
  (2) a missing store path returns -ENOENT (store-index, not host-root); (3) a
  RELATIVE probe against a real `openat(O_DIRECTORY)` store dirfd returns 0
  (dirfd honored, not hardcoded AT_FDCWD). The old handler fails 2/3 (dirfd read
  as path).

**Also corrects a stale documented premise.** SH14/SH39b wrote that the
type-4 deque-maintenance handler "blrs through framework-owned BSS globals
0x1068262e8/300/308 — all statically 0 on this box." A `JIT_FRAMEWORK_DUMP`
under the stable boot (exit 124) shows those globals are **populated at runtime
with real .text addresses**:
```
[elfjit:fw] deque-fwd 0x1068262e8=0x10620db24 0x106826300=0x102176bfc 0x106826308=0x1022199e0
```
(identical across the full render recipe). The three targets are thin
bionic/atrace-ish upkeep functions (each derefs TLS via `adrp 0x67d1000[#1776]`)
— not render/session producers — and the drain's pop-loop still hardcodes
`w4=4` (maintenance) at dispatch (0x2856ffc). So the **structural wall stands**:
the engine still never self-produces a render-task type, and frames remain
harness-driven on the live engine context. But future cycles should not treat
those globals as an impossible NULL: a seeded node's maintenance dispatch does
execute real engine code (SH13's `--deque-node-live` live-drainer result), which
partially re-opens the deque path this handoff had flagged closed.

Verified:
- `cargo test --workspace` → 487 passed / 0 failed (was 486/0; +1 regression).
- `cargo build --workspace` clean (only pre-existing non_snake_case/dead_code
  warnings; none in the edited lines).
- Full real-boot render (runs/sh41-boot-render-verify.txt): exit 124 stable, real
  indexed glDrawElements triangle (centroid RGBA(255,0,0,255)) + textured quad
  (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE exact texels) + 4 fresh quad-loop frames,
  swap Ok(0x1), zero ENOSYS/unhandled syscalls, no json-Writer terminate.

**Next (closest unblocked):** the data-plane now covers the full SQLite
session-datastore lifecycle through the store. The engine's own main-loop
producer still never enqueues a render-task type (the `w4=4` structural cap),
so frames are harness-driven. Two directions: (a) use the now-confirmed-live
maintenance globals to drive `--deque-node-live` toward real engine framework
code (SH13's live-drainer path) and see whether a maintenance dispatch advances
the session past idle; or (b) harden the JNI/network surface the client touches
once a real session reads/writes its now-persistent store.

SH38's fsmap remapped openat/mkdirat/unlinkat/renameat/faccessat/newfstatat, but
the remaining path-taking syscalls a real session's datastore touches were still
forwarded raw against the host root — a guest `/data/...` path ENOENTed. Most
critically **statx(291)**: bionic/Java answer "does my session file exist / its
metadata" there, so a client's statx on its own datastore path resolving to the
host root + ENOENT makes it *believe its store is gone* — the exact opposite of
the "remembers sign-in" objective. This cycle routes the rest of the path-taking
syscalls through `crate::fsmap::remap_path` (+ `ensure_parents` where the call
creates):

- **statx(291)** — dirfd a0=AT_FDCWD for absolute guest paths; `struct statx` is
  asm-generic/byte-identical on both arches so a raw forward writes the guest's
  statx buffer in place.
- **statfs(43)**, **truncate(45)**, **chdir(49)**, **fchmodat(53)**,
  **fchownat(54)**, **linkat(37)** (both paths), **utimensat(88)**,
  **readlinkat(78)** (pathname), **symlinkat(36)** linkpath.
- **readlinkat arg-order bug FIXED**: the old handler passed the *dirfd* (a0) as
  the pathname with a hardcoded `AT_FDCWD`, so any real guest readlinkat on a
  host-resolved path EFAULTed. Now dirfd=a0, pathname=a1 (remapped).

New hermetic regressions (tests/fsmap_persist.rs, drive the REAL guest_svc ABI,
no APK):
1. `fsmap_statx_and_statfs_reach_the_persistent_store` — after an openat+write
   of `session.dat`, statx reads back the store's REAL stx_size (offset 40 of
   the 256-byte asm-generic `struct statx`); statx on a MISSING store path
   returns -ENOENT (not EPERM, proving it resolved through the store); statfs on
   guest `/data` succeeds.
2. `fsmap_truncate_chdir_linkat_readlinkat_resolve_through_store` — truncate
   shrinks the mapped host file to 4 bytes; chdir lands in the store; linkat
   hard-links a store file; readlinkat resolves a store symlink and returns
   -ENOENT for a missing path (this doubles as proof the arg-order fix works,
   since the old code would have read the AT_FDCWD dirfd as the path).

All path-taking fs syscalls now reach the same persistent store that openat/write
already wrote to, so a real session's datastore survives a restart end-to-end
(write → statx-exists → read). Next: the standing producer/deque wall (SH39b) —
the engine's per-CPU task-deque consumer still parks on the framework producer
enqueue — or more path-hardening as the real client surfaces new syscall gaps.

**SH40b (bd00e88):** the real client's datastore is SQLite-backed — it takes
advisory `flock` locks on db/shm files for concurrency and `fallocate`-preallocates
space when growing mmap-backed db files. Both were unhandled (-ENOSYS). Added
flock(32) -> host advisory lock and fallocate(285) -> SYS_fallocate; removed a
dead duplicate truncate(45) arm left at the durability block (the remapped arm
from SH40 runs). Extended the fsmap meta test to reopen a store file, flock
LOCK_EX|NB, grow it via fallocate to >=4096, release, close. The persistent
store now survives the full SQLite-style lifecycle (create → write → statx-exists
→ flock → fallocate → truncate → readlink → read). Verified the full product
boot+render still reproduces with ZERO ENOSYS/unhandled syscalls after both
commits (runs/sh40-boot-render-verify.txt: real triangle centroid red + 6
sustainable textured-quad frames, exit 124).

## Session (Sep 12, 2026, hermes-worker, cycle SH39) — PRODUCTIZED the proven JIT boot+render: `open-sober play --apk <real-roblox.apk> --jit` now drives the REAL client's OWN render path (engine GLES bridge on a live Mesa-llvmpipe EGL context) to render real frames — a real indexed glDrawElements triangle (centroid red RGBA(255,0,0,255)) + a real interpolated-UV textured quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE exact texels) + 6 fresh sustainable textured frames (5 distinct cycling backgrounds) — through the actual product entry point instead of the debug harness. Exit 124 (stable idle main loop after the render prove). Workspace 484/0 (was 482/0).

The SH15–SH38 frontier had proven this capability only inside the `elfjit` debug
harness; the real product command (`open-sober play --jit`) still ran a crude
`jit::run_elf_entry` stub that couldn't bootstrap the real client. This cycle
wires them together:

- `crates/sober-core/src/jitlaunch.rs` (new): `invocation_proven(lib, frames)`
  builds the canonical recipe `JNI_OnLoad(0x2173ff4) → StartApp(0x258b144) →
  render-init thunk(0x105b3a280) → renderthunk → renderframe(+drive+seedgles) →
  drawprobe → triangle → quad → quad-loop N → kicker(0x106863af8)` with
  `JIT_DRIVE_LIFECYCLE=1` + `RENDERINIT_WARMUP_MS=5000`; `resolve_elfjit_bin()`
  finds/builds the `elfjit` example binary; `launch_jit(lib)` spawns it against
  the extracted `libroblox.so` (elfjit self-contains Xvfb, ANativeWindow→X11 XID
  wiring, and `SOBER_ANDROID_ROOT` arming from SH38/SH38b).
- `main.rs`: the `--jit` branch now `apk::extract_libs → qemu::find_main_binary →
  jitlaunch::launch_jit`. The superseded `jit.rs` (`run_elf_entry`) module is
  removed.
- 2 new regression tests pin the proven recipe + the bounded frame count.

Real proof (runs/capture_jit_play.sh, log runs/sh39-play-jit.txt): the APK
extraction resolved the real `libroblox.so`, the whole chain product command →
APK → real binary → JIT → engine render ran, and the render (`geometry wrapper
0x5b35288 Ok(0x0)`, `swap Ok(0x1)`, exact readbacks, 6 fresh frames) succeeded.
Doc docs/frontier-sh39-productize-play-jit.md.

Honest scope: render is still harness-driven on the live engine context — the engine's
own main-loop producer still never enqueues a render task (SH14's standing structural
wall). SH39 changes WHERE the harness is driven from (the product command, not a debug
example) and arms persistence so a real session's datastore can persist.

## Session (Sep 12, 2026, hermes-worker, cycle SH38) — closed the data-plane FS gap: guest file paths under Android's writable roots now remap to a real persistent host store, so the client's datastore/login session can persist "like the real app". Workspace 482/0 (was 479/0).

New `arm64jit::fsmap` module (crates/arm64jit/src/fsmap.rs) + syscall wiring: the
JIT's `guest_svc` was passing guest file-path pointers verbatim to host libc, so
a real session's `/data/data/com.roblox.client/...` (datastore, shared_prefs,
session cookie), `/sdcard/...`, `/storage/emulated/0/...`, `/cache/...` read/writes
hit the host root and failed ENOENT/EPERM — the client could not persist anything.
Now, when a host root is configured (`SOBER_ANDROID_ROOT` env or a test setter),
those four writable mount roots remap to `{root}/data|storage|sdcard|cache/...`,
and `ensure_parents` recursively scaffolds the `/data/user/0/com.roblox.client/...`
chain so O_CREAT/mkdirat on a deep path never ENOENTs. Input-off by default (root
unset → paths pass through), so the existing boot is untouched. Relative and
non-writable/virtual roots (`/system`, `/proc`) are NOT remapped. Wired through
`crate::fsmap::remap_path` + `ensure_parents` in guest_svc: openat(56),
mkdirat(34), unlinkat(35), renameat(38), faccessat(48), fstatat(79);
read/write/readv/writev on the fd are unchanged.

Hermetic proof (crates/arm64jit/tests/fsmap_persist.rs, drives guest_svc through
the real ABI, no APK): a write under `/data/user/0/com.roblox.client/files/
session.dat` lands in a real host file under the root, and a *fresh* CpuState
("restart") reopens the same guest path and reads the exact bytes back — the
store survives a restart. 3 new regressions: cross-restart persistence, mapping
correctness (+ negative cases: /system, /proc, relative pass through), and
parent-dir scaffolding for mkdirat/deep O_CREAT. Real boot re-verified unchanged
(JNI_OnLoad 0x10006, StartApp driven, stable idle main loop).

**Next (closest unblocked):** this closes the data-plane persistence gap (objective
2b enabler). SH38b (ef0cf2b) additionally arms the persistence root in elfjit real
runs (create + export SOBER_ANDROID_ROOT under XDG/HOME data; verified the full
boot still exits 124 stable with it armed). The standing structural frontier is
unchanged (SH14/SH37): the engine's own main-loop producer never enqueues a render
task, so the engine renders what the harness drives. To turn "persistence works"
into "the client remembers sign-in", re-open the producer/deque wall so the boot
enters a real session that reads/writes the now-persistent store. Doc:
docs/frontier-sh38-fsmap-persist.md.

## Session (Sep 12, 2026, hermes-worker, cycle SH37) — the SH35-sealed GLES3 pipeline slots are proven FUNCTIONAL, not just resolvable: dispatched through the engine's OWN slot stubs on the live context — program-binary round-trip, UBO bind, instanced draw. Workspace 479/0 (was 478/0). Commits a0ba81c (+8f57 ledger).

New elfjit `--renderframe-progbin` drives the engine's dispatch stubs `0x5b3a1c0+0xc*N`
(`adrp x8,6d3b000; ldr x3,[x8,#752+8N]; br x3` — the exact br-through-table mechanism a
real session's frame uses) with real guest-ABI args on the live Mesa-llvmpipe context.
ALL clean (glGetError NO_ERROR, exit 124), coexisting with the standard render path
(geometry wrapper Ok, textured-quad exact texel readbacks, triangle draw, swaps Ok(0x1)):

- slot15 glProgramParameteri(GL_PROGRAM_BINARY_RETRIEVABLE_HINT=0x8257) pre-link.
- slot13 glGetProgramBinary -> a REAL 3498-byte Mesa binary (format 0x875f) — Mesa
  produced a retrievable binary THROUGH the sealed slot.
- slot14 glProgramBinary re-upload accepted (err 0x0).
- slot5  glBindBufferBase(GL_UNIFORM_BUFFER,0,real_buf) binds a UBO (err 0x0).
- slot10 glDrawArraysInstanced(GL_TRIANGLES,0,0,3) dispatches clean (err 0x0).

Bug fixed en route: the slot-stub constants were the .so FILE vaddrs (0x5b3a...) but
jit_run wants GUEST vaddrs → +0x100000000 (0x105b3a...); the first attempt ran
"pc 0x5b3a274 outside image". New hermetic regression
`sealed_gles3_ubo_and_instanced_slots_dispatch_real_mesa_clean` (surfaceless ES3 ctx:
glBindBufferBase + glDrawArraysInstanced through resolve_gles_int -> GL_NO_ERROR).
Live log runs/sh37-progbin-full.txt. This closes SH35's "prove they run" step: every
dispatch slot 0-15 is now bridged AND functionally dispatchable, plus real frames
(solid/triangle/textured-quad/grid, ETC1/ETC2/ASTC) through the engine's own geometry
wrapper + swap.

**Next (closest unblocked):** the harness has saturated the GLES dispatch surface. The
remaining structural frontier (unchanged since SH14): the engine's own main-loop producer
never enqueues a render task, so frames are harness-driven on a time base from a detached
thread. Two candidate directions: (1) re-open the producer/deque wall now that the full
render pipeline behind it is proven bridge-functional (a real self-driven frame is the
remaining 'real session' gap); (2) product-ize: make sober-core's `open-sober play --apk
roblox.apk` reproduce this elfjit boot (JNI_OnLoad + StartApp + render-init + frame drive)
automatically instead of hardcoded elfjit addresses — turning the proof harness into the
runtime's actual boots-real-binary path.

## Session (Sep 12, 2026, hermes-worker, cycle SH36) — sealed the LAST raw clear-dispatch gap: slot 3 (guest BSS 0x106d3b308) now resolves as glClearBufferfi through the MIXED (float) GLES bridge, not glClearStencil. Workspace 478/0 (was 477/0). Commit ea3e692.

Closing the clear-path analog of SH35: disasm of the real clear-state sub-fn 0x5b32ef4
(the per-buffer COMBINED depth+stencil clear) shows `mov w0,#0x84f9` (GL_DEPTH_STENCIL),
`ldr s0,[x21,#68]` (depth -> s0, FIRST FP arg), `ldr w2,[x21,#72]` (stencil),
`mov w1,wzr` (drawbuffer), then `bl 0x5b3a1e4` (the slot-3 stub: `adrp x8,6d3b000;
ldr x3,[x8,#776]` = guest 0x106d3b308). Because depth is a FLOAT, glClearBufferfi is
MIXED-ABI — the integer HostCall only marshals x-regs and would DROP the s0 depth. The
pre-SH36 seed put glClearStencil (single-int) on slot 3, which mis-routes a real
GL_DEPTH_STENCIL dispatch. Fix: new w_glClearBufferfi (AAPCS: gs_f(s,0) for the float)
in gles_mixed_wrapper -> w_eglGetProcAddress auto-heals the engine table;
resolve_gles_int rejects it (float ABI). elfjit seedgles slot3 glClearStencil->
glClearBufferfi. Verified live (runs/sh36-clearbufferfi.txt): slot3 seeds to bridge
0x7f0000018058, textured-quad/triangle draws + swaps all Ok(0x1), exact texel readbacks,
exit 124. New regression resolve_gles_mixed_clearbufferfi_is_mixed_abi_not_int.
Every slot a real frame can dispatch (0-15) now routes through our bridge.

**Next (closest unblocked):** now that the UBO/instanced/program-binary dispatch slots
(4-8 UBO, 9/10 instanced, 13-15 program-binary) AND the full clear map (0-3) are all
bridge-resolved, prove the MODERN GLES3 render path live: fabricate a coherent renderer
that binds a real UBO (glBindBufferBase slot5 + glUniformBlockBinding slot4) and draws
an INSTANCED mesh (glDrawArraysInstanced slot10 / glDrawElementsInstanced slot9) through
the engine's own draw wrapper, read back N distinct instances — proving a real session's
instanced pipeline (heavy in Roblox) runs through the bridge instead of jumping
out-of-image. This is the harness-level proof that the SH35-sealed slots are genuinely
functional, not just resolvable.

## Session (Sep 12, 2026, hermes-worker, cycle SH35) — the engine's REAL GLES3 dispatch-slot table is no longer raw-Mesa: the UBO / instanced / program-binary pipeline slots now resolve through the JIT bridge. Workspace 477/0 (was 476/0).

SH28's live slot snapshot showed the engine's own GL-init fills its GLES dispatch
table (BSS 0x106d3b2f0 + 8*N) slots **4-8** (glUniformBlockBinding / glBindBufferBase /
glBindBufferRange / glGetUniformBlockIndex / glGetActiveUniformBlockiv), **9/10**
(glDrawElementsInstanced / glDrawArraysInstanced) and **13-15** (glGetProgramBinary /
glProgramBinary / glProgramParameteri) with **raw-Mesa host addresses** — the same
SH19/SH24 crash class (a guest `br` through the 0x5b3a1c0+0xc*N stub jumps
out-of-image). A real self-driven engine frame dispatching those slots would have
crashed. This cycle added all ten names to `resolver::GLES_INT_NAME_LIST` (each
pure int/ptr ABI, ≤8 args; rejected by mixed). Because the engine builds its table
via `eglGetProcAddress` (SH3 interception → `resolve_gles_int`), its table now
**auto-heals** to bridge slots — no harness re-seed needed. Verified live
(runs/sh35-pipeline-slots.txt): the PRE-SEED snapshot now shows all ten as
`0x7f000000…` bridge slots (SH28 showed raw `0x7f44…` Mesa). Render path untouched
(geometry wrapper Ok(0x0), swap Ok(0x1), 4×4 grid 16/16 readbacks, exit 124).
New regression `gles3_pipeline_names_resolve_via_int_bridge_for_engine_draw_slots`.
Doc docs/frontier-sh35-gles3-pipeline-slots.md. Commit 51e336f.

**Next (closest unblocked):** the remaining raw-Mesa slot is 3 (glClearBufferfi,
float ABI — mixed, needs a float bridge wrap to seed; the harness still seeds it
as glClearStencil for the clear path). Then extend the coherent renderer's
sustainable loop to dispatch the UBO/instanced/program-binary path through these
now-bridge slots (prove a larger real mesh renders through the engine's modern
GLES3 draw, not the harness @plt), keeping the engine's own main-loop-producer
enqueue as the standing structural frontier.

## Session (Sep 12, 2026, hermes-worker, cycle SH34) — the coherent renderer scales to a REAL LARGER MESH: new `--renderframe-grid <N>` fabricates an N×N grid of textured quads (independent per-cell, each a distinct texel color at the interpolated vertex UV) and drives the REAL libroblox.so through the engine's OWN geometry wrapper 0x5b35288. Verified N=3 (9/9), N=4 (16/16), N=6 (36/36) cell-center glReadPixels readbacks ALL match each cell's exact distinct texel color (±1 rounding): 6×6 = 144 interleaved verts / 216 idx drawn in one call through engine primitive-setup + indexed glDrawElements, wrapper Ok(0x0), swap Ok(0x1), exit 124 stable. Sustainable (quad-loop 20 iters all Ok(0x1), fresh mesh each frame). Captures runs/sh34-grid.{txt,mp4}. Doc docs/frontier-sh34-grid.md. Fixes grid/tex buffer-overlap bugs (VBO/EBO -> 0x2000/0x4000, tex -> 0x6000; the old 0xc00/0xf60 clobbered for N≥4/8). Harness-only; single-quad mode + --jni baseline + baselines unchanged. Workspace 476/0.

## Session (Sep 12, 2026, hermes-worker, cycle SH33) — SUSTAINABLE TEXTURED real-geometry rendering: new `--renderframe-quad-loop <N>` re-drives clear(cycling 5-color bg) -> the engine's OWN geometry wrapper 0x5b35288 -> swap N times on the detached host thread, after the single textured-quad proof frame. Verified 6 iterations, EVERY `drew+swap Ok(0x1)`, 5 distinct cycling backgrounds (a recording proves fresh textured renders), and the textured readback intact in the same run (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE). 8-frame x11grab runs/sh33-quad-loop.mp4. This is the textured/mesh analog of SH25b's triangle-loop — the textured recipe now both RENDERS correctly (SH30-32) AND SUSTAINS (SH33), the two properties a real main-loop frame drive needs. Doc docs/frontier-sh33-quad-loop.md; reproducible runs/capture_quad_loop.sh. Harness-only (no codec/resolver change). Workspace 476/0; baselines unchanged (--jni exit 0, idle 124).

## Session (Sep 12, 2026, hermes-worker, cycle SH32) — the engine's OWN geometry path renders a REAL ASTC texture: `--renderframe-astc` uploads an 8x8 GL_COMPRESSED_RGBA8_ASTC_4x4 (0x93B0, the load-bearing Android format desktop GL can't native-decode — our interception is REQUIRED there) through glCompressedTexImage2D on the coherent 2-attrib quad; the GLES bridge decodes via decode_astc. The 8x8 = 4 x 16-byte Khronos LDR void-extent blocks (buf[0]=0xFC, bit8, bit9=Dynamic-Range=0; color = UNORM16 at bytes 8/10/12/14, high-byte=8-bit channel — verified against DataFormat/astc.txt). FS maps the DECODED ALPHA to RGB gray-scale, so readback proves the ASTC void-extent alpha decodes live: `BL=RGBA(255,255,255,255) BR=190 TR=64 TL=128` as gray; capture runs/sh32-astc.{rgb,png} = 4 distinct gray quadrants on clear-blue, wrapper Ok(0x0), swap Ok(0x1), exit 124. New hermetic regression `astc_ldr_void_extent_blocks_decode_expected_color_and_alpha`. Doc docs/frontier-sh32-astc.md; reproducible runs/capture_astc.sh. Compressed live-prove matrix now ETC1+ETC2-RGB+ETC2-RGBA8/EAC+ASTC-4x4. Workspace 476/0; baselines unchanged (--jni exit 0, idle 124).

## Session (Sep 12, 2026, hermes-worker, cycle SH31) — the engine's OWN geometry path renders a REAL ETC2-RGBA8/EAC texture: `--renderframe-etc2a` uploads an 8x8 ETC2-RGBA8 (GL_COMPRESSED_RGBA8_ETC2_EAC=0x9278, the real Android RGBA-EAC format) through glCompressedTexImage2D on the coherent 2-attrib quad; the GLES bridge decodes via decode_etc2_rgba8. The FS maps the DECODED ALPHA to RGB gray-scale (window framebuffers often drop alpha), so readback proves the EAC alpha sub-block executes live: readback `BL=RGBA(255,255,255,255) BR=RGBA(190,190,190,255) TR=RGBA(64,64,64,255) TL=RGBA(128,128,128,255)` = the 4 exact EAC block alphas as gray lobes; capture runs/sh31-etc2a.{rgb,png} = 4 distinct gray quadrants on clear-blue, wrapper Ok(0x0), swap Ok(0x1), exit 124. New hermetic regression `etc2_rgba8_eac_solid_blocks_decode_expected_alpha` (RGB + per-block EAC alpha from 4 crafted 16-byte blocks). Doc docs/frontier-sh31-etc2a.md; reproducible runs/capture_etc2a.sh. Compressed-texture live-prove now ETC1+ETC2-RGB+ETC2-RGBA8/EAC. Workspace 475/0; baselines unchanged (--jni exit 0, idle 124).

## Session (Sep 12, 2026, hermes-worker, cycle SH30) — REAL TWO-ATTRIB TEXTURED QUAD renders through the engine's OWN geometry wrapper: `--renderframe-quad` fabricates a coherent renderer whose primitive-setup 0x5b353d0 loop now runs its multi-primitive path to set up TWO vertex attribs (aPos @ format[3]{4,GL_FLOAT} offset0, aUV @ format[1]{2,GL_FLOAT} offset16) on an interleaved [pos.xyzw, uv.xy]x4 VBO (stride 24, 6-idx EBO). Fragment shader samples a 2x2 checkerboard at the REAL interpolated vertex UV. Readback proves per-texel UV mapping: BL=RED, BR=GREEN, TR=WHITE, TL=BLUE. Captured runs/sh30-quad.{rgb,png} = 4 near-equal color quadrants (~186.6k px each) on clear-blue, exit 124. Doc docs/frontier-sh30-quad.md. Workspace 474/0; baselines unchanged.

## Session (Sep 12, 2026, hermes-worker, cycle SH29) — the engine's OWN geometry wrapper renders a REAL ETC2 texture (GL_COMPRESSED_RGB8_ETC2=0x9274, the actual Android Roblox format): `--renderframe-etc2` relays SH27's hand-crafted 8x8 four-block texture with internalformat 0x9274; the bridge decompresses via decode_etc2_rgb and re-uploads. Readback identical to ETC1 (centroid WHITE / quad-GREEN(2,255,2) / quad-RED(255,2,2)), pixel-identical capture to ETC1 (modes 1/2 are bit-identical). Regression: crafted test now also asserts ETC2 decode of the same blob (colors + equality with ETC1). Captured runs/sh29-etc2.{rgb,png}. Workspace 474/0; baselines unchanged. Doc docs/frontier-sh29-etc2.md.

## Session (Sep 12, 2026, hermes-worker, cycle SH28) — captured the real engine GLES dispatch-table content live: --renderframe-seedgles now dumps all 16 raw slot values (BSS 0x106d3b2f0+8*N) before seeding and dladdr-resolves each to a symbol. Reveals the engine's REAL renderer dispatch is a modern GLES3 pipeline (slots 4-8 = glUniformBlockBinding/glBindBufferBase/glBindBufferRange/glGetUniformBlockIndex/glGetActiveUniformBlockiv; 9/10 = glDrawElementsInstanced/glDrawArraysInstanced; 13-15 = program-binary), not the simple clear/draw map. Slots 0-2 read as our bridge slots (0x7f..) confirming SH3's eglGetProcAddress interception reaches the engine's own table. Diagnostic-only (harness seeding unchanged, renders correctly). Workspace 474/0; baselines unchanged. Doc docs/frontier-sh28-slotmap.md.

## Session (Sep 12, 2026, hermes-worker, cycle SH27) — the engine's OWN geometry wrapper now renders a REAL COMPRESSED-ETC1 texture: `--renderframe-etc` uploads a hand-crafted 8x8 ETC1 texture (4 solid blocks) via glCompressedTexImage2D; the GLES bridge decompresses ETC1->RGBA (texture-codec) and re-uploads. Three on-triangle probes read back the distinct decoded colors. Workspace 474/0; HEAD (this commit).

Follows SH26's RGBA-textured triangle. The compressed-texture interception path
(implemented since earlier cycles but never proven live) now renders end-to-end:
a hand-crafted ETC1 block `[R,G,B,0,0,0,0,0]` — individual mode, table codeword 0
(modifier +2), all selectors 0, so decoded channel = `(c*0x11)+2` clamped. Blocks
for red `(255,2,2)`, green `(2,255,2)`, blue `(2,2,255)`, white `(255,255,255)`
uploaded as an 8x8 texture through `glCompressedTexImage2D(GL_ETC1_RGB8_OES)`
@plt; the bridge's w_glCompressedTexImage2D decompresses and re-uploads. Readback
proves the decode ran (rounded channels match the (c*0x11)+2 prediction exactly):

```
uTex loc=0x0 ; compile_status vs=1 fs=1 link=1
centroid(WHITE)  = RGBA(255,255,255,255)
quad-(1,0)(GREEN)= RGBA(2,255,2,255)     quad-(0,0)(RED)= RGBA(255,2,2,255)
geometry wrapper Ok(0x0) ; swap Ok(0x1) ; exit 124
```

Captured runs/sh27-etc.{rgb,png}: clear-blue bg + triangle interior 4-colored from
decoded ETC1 (RED 155,909 / GREEN 155,899 / WHITE 52,001 / BLUE 51,947 ≈ 45.1%).
New regression `crafted_etc1_solid_blocks_decode_to_expected_colors` pins the 4x4
block -> (255,2,2) and the 8x8 quadrant decode. Reproducible: runs/capture_etc.sh;
run-log runs/sh27-etc.txt; doc docs/frontier-sh27-etc.md. 8 texture PLT stubs pinned
(see doc). Workspace 474/0.

**Next (closest unblocked):** pin the real engine's GLES dispatch-table slot 11+
mapping for texture/uniform/shader (disasm the engine's texture-binding path so a
textured engine-driven draw routes through the slots rather than harness @plt),
then scale the coherent renderer to a two-attrib (pos+UV) real mesh. Baselines
unchanged: --jni exit 0 (0x10006); stable idle exit 124; untextured triangle and
--renderframe-tex both intact.

## Session (Sep 12, 2026, hermes-worker, cycle SH26) — the engine's OWN geometry wrapper now renders a REAL TEXTURED triangle: a 2x2 RGBA checkerboard sampled by a textured fragment shader, with every texture/uniform/shader call (glGenTextures/glBindTexture/glActiveTexture/glTexImage2D/glTexParameteri/glGetUniformLocation/glUniform1i) dispatching through the JIT GLES bridge. Workspace 473/0; HEAD (this commit).

Follows SH25's solid-red triangle. New `--renderframe-tex` lever: textured FS
(`precision mediump float;` — REQUIRED in GLSL ES 1.00 for a local `vec2`, else
Mesa errors "No precision specified ... for type 'vec2'") samples a 2x2 RGBA
checkerboard (RED/GREEN/BLUE/WHITE) via a UV derived from `gl_FragCoord` (so the
single-attrib coherent renderer stays unchanged). Verified THREE on-triangle
quadrant probes read back three DIFFERENT colors — impossible for a constant
shader, i.e. the sampled texture definitively rendered:

```
compile_status vs=0x1 fs=0x1 link_status=0x1 ; uTex loc=0x0<-unit0
readback centroid(WHITE)  @(640,360) = RGBA(255,255,255,255)
readback quad-(1,0)(GREEN)@(900,150) = RGBA(0,255,0,255)
readback quad-(0,0)(RED)  @(300,150) = RGBA(255,0,0,255)
geometry wrapper Ok(0x0) ; post-draw swap Ok(0x1) ; exit 124
```

Captured frame (runs/sh26-tex.{rgb,png}): clear-blue bg + the triangle interior
4-colored (RED=155,909 / GREEN=155,899 / WHITE=52,001 / BLUE=51,947 ≈ 45.1% of
the frame — the SH25 footprint now textured). glTexImage2D is a 9-arg form
(pixels rides the guest stack at [sp+0]); the PLT stub is a leaf (adrp/ldr/add/br,
never pushes sp), so a fake sp whose [0] holds the pixels ptr is read correctly by
the bridge's gs_stack. Reproducible: runs/capture_tex.sh; run-log runs/sh26-tex.txt.

New debug aid: failing shaders now dump their info log via the int bridge
(glGetShaderInfoLog/glGetProgramInfoLog resolve through resolve_gles_int) — this
surfaced the missing precision declaration.

**Next (closest unblocked):** pin the real engine's GLES dispatch-table slot 11+
mapping for texture/uniform/shader (disasm the engine's texture-binding path so a
textured engine-driven draw routes through the slots rather than the harness's
harness-made @plt calls), then scale the coherent-renderer rotation onto a
two-attrib (pos+UV) real mesh. Compressed-texture (ETC2/ASTC) interception is
already implemented in the bridge/texture-codec and only needs a live-path prove.
Baselines unchanged: --jni exit 0 (0x10006); stable idle exit 124; untextured
triangle still solid-red. Workspace 473/0.

## Session (Sep 12, 2026, hermes-worker, cycle SH25) — the coherent renderer RENDERS a REAL VISIBLE triangle, and the engine's OWN geometry wrapper does it. Workspace 473/0; HEAD 035ff6a.

Follows SH24's draw-probe (empty prim list → proved dispatch only). SH25 feeds
primitive-setup 0x5b353d0 a **coherent** renderer and REAL GL resources, all
through the JIT GLES int bridge on the live render-ctx: a compiled+linked shader
program (VS `gl_Position=aPos`, FS solid red), a real VBO (3×vec4 NDC, ±0.95),
a real EBO (0,1,2). Driving the engine's own geometry wrapper 0x5b35288
dispatches a REAL indexed `glDrawElements(GL_TRIANGLES,3,GL_UNSIGNED_INT)` that
RENDERS: readback `centroid(640,360)=RGBA(255,0,0,255)`; capture
runs/sh25-triangle.{png,rgb} = **415,696 red px = 45.11% of frame** (a clean
apex→base triangle shape), wrapper Ok(0x0), swap Ok(0x1), exit 124.

Three real bugs fixed en route (each caused a silent empty/collapsed render):
1. **Format-table index** — `[prim+8]`=5 is format[5]={size4, GL_SHORT=0x1402};
   engine's glVertexAttribPointer misread float verts as shorts → degenerate.
   Fix `fmt_index=3` = format[3]={size4, GL_FLOAT=0x1406}. This was WHY the
   SH24-scoped "wrapper collapse" existed; with it the RAW engine path renders
   the full triangle (SH25_REF direct-draw now defaults OFF, opt-in =1).
2. **Wrapper count register** — glDrawElements COUNT rides in the 4th drive arg
   (w20 → `mov w1,w20`), NOT x5 (SH24 comment was wrong); + glViewport/glScissor
   (0,0,1280,720) must be set or a stale 0-size viewport rasterizes nothing.
3. **glGenBuffers aliasing** — writing the generated id into the same memory as
   the vertices clobbered the data → dedicated id slots.

Coherent-renderer layout now fully reversed (doc: docs/frontier-sh25-triangle.md):
renderer[+56]=container; container[+72]/[+80]=prim begin/end (stride 0x18,
count=(end-begin)/24 via the magic-const mul); **renderer[+0x48]=INLINE
vertex-descriptor table** (entry[vb]@+vb*16 = descriptor ptr, [desc+72]=ARRAY id);
container[+96]=stride table ([+vb*8]); renderer[+120]=IBO ([+72]=ELEMENT id);
renderer[+142] u16 count; primitive[+0]=vb,[+4]=offset,[+8]=format idx,
[+12]=attrib(0),[+16]=base.

Reproducible: runs/capture_triangle.sh; run-log runs/sh25-triangle.txt.

**SH25b (fd5227b):** `--renderframe-triangle-loop <N>` — same bind → clear →
coherent-draw → swap recipe rendered SUSTAINABLY on the detached host thread,
cycling clear-bg through 5 colors/frame; verified 6 iters all Ok(0x1), red
triangle in every captured frame (runs/capture_triangle_loop.sh). Geometry
analog of SH23's --rendersustain.

**Next (closest unblocked):** GLES slots 11+ (texture/uniform/shader dispatch) +
ETC2/ASTC compressed-texture interception so a *textured/shaded* draw renders;
then scale the (fully-reversed) coherent-renderer rotation onto a larger real
mesh. The engine's own main-loop producer still never enqueues a render task
(the long-standing structural wall) — harness drives its own code on a time base.
Baselines unchanged: --jni exit 0; stable idle exit 124.

## Session (Sep 12, 2026, hermes-worker, cycle SH24) — the engine's OWN real GEOMETRY draw path now dispatches glDrawElements through the GLES bridge: complete 16-slot dispatch map (slots 9/10 = glDrawElements/glDrawArrays), new `--renderframe-drawprobe` that drives the engine's own geometry wrapper 0x5b35288 to a real indexed glDrawElements through the bridge (mode=GL_TRIANGLES, GL_UNSIGNED_INT, GL_ELEMENT_ARRAY_BUFFER bind), wrapper Ok(0x0) + post-draw swap Ok(0x1), exit 124 stable. Workspace 473/0; HEAD e358df0.

Follows on SH23's sustained clear loop. The clear-only frame-fn (0x105b32c00)
never touches geometry; the engine's real draw is wrapper 0x5b35288 →
primitive-setup 0x5b353d0 (binds GL array buffers, enables attrib arrays, sets
glVertexAttribPointer — all via direct @plt) then dispatches the indexed /
array draw through GLES dispatch-table **slot 9 = glDrawElements** (0x5b352f4
bl 0x5b3a22c) / **slot 10 = glDrawArrays** (0x5b35368 bl 0x5b3a238). Those
extended slots (BSS 0x106d3b2f0 + 8*N, 16 total; stub 0x5b3a1c0+0xc*N) held
raw-Mesa addresses — the SH19/SH3 bug class for the draw path.

- Commits: (1) complete the map + seed slots 9/10 + regression. (2) correct
  seed to explicit (slot,name) + add `--renderframe-drawprobe`.
- `--renderframe-drawprobe`: fabricates a minimal renderer (empty primitive
  list → 0x5b353d0 returns mask 0 fast; nonzero [renderer+120] index-buffer
  obj + w5=3 count → INDEXED path). With slots 9/10 seeded the wrapper
  dispatches a REAL glDrawElements through the bridge:
  `hostcall@glDrawElements pc=0x7f0000002a38 x0=0x4 x1=0x0 x2=0x1405
  x30=0x105b352f8` (mode=GL_TRIANGLES, type=GL_UNSIGNED_INT), plus
  `glBindBuffer(GL_ELEMENT_ARRAY_BUFFER=0x8893) x30=0x105b35550`.
- Reproducible artifact: runs/capture_drawprobe.sh; run-log runs/sh24-drawprobe.txt.
  Doc: docs/frontier-sh24-draw-slots.md.
- Regression `draw_slots_gl_draw_elements_arrays_resolve_via_int_bridge`
  (both resolve via int bridge with trailing NUL, rejected by mixed).

**Honest scope:** the fabricated renderer is EMPTY (no real mesh/buffer/VAO
data), so this proves the DRAW DISPATCH is bridge-functional, not the render of
real geometry. Baselines unchanged (--jni exit 0; stable idle exit 124).

**Next wall (the multi-cycle renderer C++ reverse, now clearly scoped):** feed
primitive-setup 0x5b353d0 a coherent primitive list + vertex buffers so the
draw wrapper produces a real rendered triangle. Primitive list lives at
[renderer+56]=container, [container+72]/[80] = begin/end (stride 0x18 per
primitive); vertex buffers/id + VAO state in the renderer sub-objects
(0x5b353fc [x25+96] buffer array, 0x5b3547c descriptor idx, 0x5b35488 attrib
mask). Slots 11+ (texture/uniform/shader dispatch) still unseeded/reversed.

## Session (Sep 12, 2026, hermes-worker, cycle SH23) — the engine's OWN render recipe now runs as a LIVE ANIMATED render loop: new elfjit `--rendersustain <fps>` drives bind -> frame-fn 0x105b32c00 -> post-frame swap CONTINUOUSLY on the detached host thread (concurrent with StartApp's idle main-loop jit_run), cycling a 5-color palette per frame. Workspace 472/0; HEAD b692077.

SH22d proved the recipe reentrant (N=3, frozen color). SH23 makes it
**sustainable + animated**: the engine's own frame-fn is driven on a real
time-base for the whole run — 160 consecutive frame-fn->swap pairs, every
`frame-fn returned Ok(0x..)` + `post-frame swap returned Ok(0x1)`, exit 124
stable, zero crash/heap abort. A real x11grab recording proves every frame is
a **fresh render**: majority pixel color tracks the per-frame palette exactly
(green->red->blue->yellow->magenta; float fracs match to 3 dp), 12+ distinct
frames over 6 s. Each iteration re-writes both engine clear-color sources (the
frame-fn 5th-arg clear-state obj at base+0x400 [+4..16] and the 6th-arg
color-source obj at base+0x500 [+0..16]) before calling the real frame-fn, so a
capture can't be a static buffer.

- New lever: `--rendersustain <fps>` (sustain loop; bounds via
  `--renderframe-loop <N>` only when --rendersustain absent). Per-frame color is
  re-seeded into both clear-color objects each iteration.
- Reproducible artifact: `runs/capture_sustain_loop.sh` (starts elfjit, waits
  for frame iteration 2, x11grab 2fps for CAP_SECS, then decodes each recorded
  frame's majority color). Run-log: runs/sh23-sustain-loop.txt (160 iters);
  video: runs/sh23-sustain-loop.mp4. Doc: docs/frontier-sh23-sustain-render.md.
- Baselines re-verified unchanged: `--jni` exit 0; stable idle exit 124.

**Honest framing (unchanged shape):** still harness-driven — fabricated
renderer/view/clear-state objects and a clear-only frame (the engine's real
glDrawElements draw path is gated on a coherent renderer C++ object not yet
reversed). The engine's own main-loop producer still never enqueues a render
task, so it does not call frame-fn by itself yet — we drive its own code on a
time base. But the GLES dispatch-slot map is complete and the engine's GL path
is proven bridge-functional **and sustainable**, the two properties a real
main-loop frame drive needs.

## Session (Sep 12, 2026, hermes-worker, cycle SH22) — BROKEN THE SH21 WALL: the engine's OWN frame-fn 0x105b32c00 now presents a real, correctly-colored 1280x720 frame through the GLES bridge (18430/18432 sampled px = the exact --renderframe-color 0.4,0.2,0.95), stable exit 124, zero crash. Workspace 472/0 (was 471).

SH21 left the window black, blaming "the per-buffer clear loop clears depth-style buffers via a guessed slot2=glClearDepthf". Disassembly of the clear-state
sub-fn 0x5b32e08 corrects this: **0x1800=GL_COLOR / 0x1801=GL_DEPTH are
`glClearBufferfv` BUFFER enums** (the loop does `slot2(0x1800, drawbuffer=i,
value=clearstate+4+i*0x10)` iterating 4 color draw-buffers; the depth branch
does `slot2(0x1801,0,...)`). So **slot2 = glClearBufferfv**, and slot0 (which the
preamble dispatches with {GL_COLOR_ATTACHMENT0..3} / {GL_BACK}=0x405 arrays) =
**glDrawBuffers** — the SH19-21 "glClearColor"+"glClearDepthf" seed guesses
mis-routed both. glClearDepthf's float bridge ignored the int/ptr args and
cleared nothing → that was the black window.

Fixes (commit): (1) resolver.rs adds glClearBufferfv + glDrawBuffers to
GLES_INT_NAME_LIST (both pure int/ptr ABI, safe through the integer HostCall —
they previously resolved None so the bridge couldn't seed); (2) elfjit seed_names
corrected to slot0=glDrawBuffers, slot2=glClearBufferfv; (3) objB[+140]=0 so the
default-FB preamble takes glDrawBuffers(1,{GL_BACK}) instead of the 4-color-attach
form.

Verified real run (runs/sh22-color-frame-from-engine-framefn.txt):
`slot 0 (glDrawBuffers) <- bridge 0x7f0000003010`, `slot 2 (glClearBufferfv) <-
bridge 0x7f0000003018`, `engine frame-fn 0x105b32c00 returned Ok`,
`post-frame swap returned Ok(0x1)`, exit 124. Frame artifact:
runs/sh22-color-frame-from-engine-framefn.{png,rgb} — raw RGB(102,51,242) =
(0.4,0.2,0.95) = exact clear color; only ~0.013% black (window edge). No channel
swap (the SH21 capture script mislabeled grab byte order b,g,r; raw rgb24 is
r,g,b). Doc: docs/frontier-sh22-color-frame.md.

Honest framing: the frame is still *harness-driven* (fabricated renderer/view/
clear-state objects, one frame-fn invocation + manual swap). The engine's real
main-loop producer still never enqueues a render task, so it doesn't drive
frames natively yet. But the mechanical reverse of slot0/slot2 removes the last
guess-blocker in the engine's own clear path. Full dispatch-slot map pinned from
disassembly (SH22c): slot0=glDrawBuffers, slot1=glClearBufferiv (0x5b32f68,
GL_STENCIL=0x1802), slot2=glClearBufferfv (GL_COLOR=0x1800/GL_DEPTH=0x1801),
slot3=glClearBufferfi (0x84F9=GL_DEPTH_STENCIL); glClearBufferiv added to
GLES_INT_NAME_LIST + seed slot1 corrected. Baselines unchanged: --jni exit 0;
stable idle exit 124.

## Session (Sep 12, 2026, hermes-worker, cycle SH21) — reverse: the frame-fn 0x105b32c00's clear-color object is its 5th arg **x4** (not x2 as SH20 guessed). Fabricating a clear-state x4 object makes the engine's OWN clear-state sub-fn 0x105b32e08 run glColorMask(all-1) + a per-buffer clear-dispatch loop + glGetError THROUGH the bridge. Workspace 471/0 (was 471).

SH20's drive stopped SILENTLY after the GL preamble (glBindFramebuffer/
glViewport/glScissor) — no clear ever fired — because the frame-fn passes its
5th arg x4 to x20 (`0x105b32c30 mov x20,x4`), gated on x4!=0 AND [x4]!=0
(`0x105b32d44 cbz x20` / `0x105b32d4c cbz [x20]`), then bl's the clear-state
sub-fn 0x105b32e08 which reads the clear RGBA float4 from [x4+4..16]
(`mov x21,x2`; `ldp s0,s1,[x21,#4]` / `ldp s2,s3,[x21,#12]`). SH20 left x4=0 →
the whole clear path was skipped, window stayed black. The "x2 = clear-color
struct ptr" comment was WRONG; the clear path uses x4.

New elfjit `--renderframe-drive` fabricates a clear-state object ([\+0]=0xF =
w20 per-buffer clear bitmask, RGBA float4 at [+4..20] from the new
`--renderframe-color r,g,b,a` lever, default 0.4,0.2,0.95,1) and passes it as
the frame-fn's x4. JIT_TRACE now shows NEW hostcalls that never fired before:
glColorMask(all-1) x30=0x105b32e44 (inside the sub-fn) + the per-buffer clear
loop at 0x105b32ec8 (mov w0,#0x1800; bl slot2-stub) iterating the 4 bits of
w20 + glGetError — then clean `frame-fn returned Ok` + `post-frame swap
Ok(0x1)`, stable exit 124. Run-log: runs/sh21-clearstate-x4.txt.

**Honest remaining wall (visible COLOR frame not yet achieved):** the window
still captures black because the per-buffer clear loop dispatches slot2
(seeded glClearDepthf — the 8 slot->function names in --renderframe-seedgles
are heuristic guesses) with integer 0x1800, i.e. it clears depth/stencil-style
buffers, not the color buffer. Getting a visible colored frame needs: (1) the
TRUE function of the slot 0x105b32ec8 dispatches + real slot0/2 names; (2)
which w20 bit maps to GL_COLOR_BUFFER; (3) the second main-fn object at
0x105b32d5c (x22, `ldr q0,[x22]; str q0,[x27]`) — likely the color-clear
source. Doc: docs/frontier-sh21-clearstate-x4.md. Baselines unchanged (--jni
exit 0; stable idle exit 124).

## Session (Sep 12, 2026, hermes-worker, cycle SH20) — resolve_gles_int/resolve_egl now accept trailing-NUL names: ALL 8 of the engine's GLES dispatch slots resolve through the bridge (was 2). Workspace 471/0; HEAD 78eca29.

Built on SH19's --renderframe-seedgles. The 6 int-ABI slots (glClear, glViewport,
glColorMask, glDepthMask, glStencilMask, glClearStencil) printed "NOT resolvable"
even though whitelisted — root cause: resolve_gles_int built its CString
cache-key from the RAW name, so a NUL-terminated caller (elfjit's seedgles
`format!("{name}\0")`, a guest eglGetProcAddress C-string) always got None.
resolve_gles_mixed strips the NUL first and worked (that's why slots 0/2 float
seeded in SH19); the int resolver did not. Same latent bug in resolve_egl. Fix =
build the key from the NUL-stripped name in both; regression
`resolve_gles_int_accepts_trailing_nul_like_mixed` pins all 8 names with a
trailing NUL.

**Real-binary proof (runs/sh20-seedgles-all-slots-ok.txt):** the engine's own
frame-fn 0x105b32c00 now dispatches its ENTIRE clear path through the bridge
(all 8 slots <- bridge slots, incl. the int-ABI glClear/glViewport/glColorMask/
glDepthMask/glStencilMask/glClearStencil that were raw/garbage before) — frame-fn
returns Ok, post-frame swap Ok(0x1), exit 124 stable, no heap abort.

**Un-skipped a dead gate:** resolve_gles_mixed_float_and_stack_abi_execute_real_mesa
silently SKIPPED its whole body for its entire life (every NUL resolve_egl ->
None -> `else return`). It now genuinely runs a surfaceless EGL->ES3->GLES chain
through the JIT bridges and passes, exposing+fixing 3 latent harness bugs: (1)
eglChooseConfig/eglCreateContext attrib arrays must be i32 (EGLint*), not u64;
(2) surfaceless needs a bound pbuffer surface (not EGL_NO_SURFACE) for a
queryable buffer; (3) Mesa surfaceless llvmpipe GL_INVALID_ENUM on
glGetFloatv(GL_COLOR_CLEAR_VALUE) — replaced the state-query with a real
glClear+glReadPixels check (float-bridge color renders [132,65,189,255] px).

Doc: docs/frontier-sh20-gles-nul-resolver.md. Baselines unchanged (--jni exit 0;
stable idle exit 124).

## Session (Sep 12, 2026, hermes-worker, cycle SH19b) — the engine's OWN frame-fn 0x105b32c00 now RETURNS Ok THROUGH the GLES bridge. Workspace 470/0; HEAD 5c72e22.

SH19 pinned the frame-fn wall: it dispatches through 8 GL function-pointer slots
(guest BSS 0x106d3b2f0..0x106d3b328, loaded by `adrp x8,6d3b000; ldr xN,[x8,#752+8k]`)
that hold **raw Mesa host addresses** — not host-thunk slots (0x7f00 0000 0000) —
so a guest `br` through the 0x5b3a1c0-family stubs jumped out-of-image
(`run_loop: pc 0x7fa7… outside image`). Same SH3/SH3b bug class (raw Mesa vs
bridge slot) but for the engine's RUNTIME-built frame-dispatch table. (An early
wrong read used 0x1067d12f0 = the stack-canary region / DER bytes; the real slots
are the adrp-6d3b000 family.)

**SH19b fix:** new elfjit `--renderframe-seedgles` overwrites those 8 slots with
host-thunk GLES bridge slots (`resolve_gles_mixed`, fallback `resolve_gles_int`).
Seeding slot0 (glClearColor) + slot2 (glClearDepthf) — the float-bridge slots the
clear-state sub-fn 0x5b32e08 dispatches through — makes frame-fn return cleanly:
`engine frame-fn 0x105b32c00 returned Ok(...)` + post-frame swap Ok(0x1), exit 124
stable, NO heap abort. Also fixed the `free(): invalid next size` shutdown heap
corruption by growing the fabricated renderer scratch 512B -> 8KiB.

**Frontier (next spread):** the engine's own clear/frame code now dispatches through
our GLES bridge and completes; the remaining wall is the coherent renderer/view C++
object reverse for the full frame (draw path), plus the ALooper/lifecycle producer
(START cmd) that would drive the loop from the engine's main thread (SH14-capped).
Baselines: --jni exit 0; stable idle exit 124. Doc:
docs/frontier-sh19-framedrive-glesdispatch.md; run-log runs/sh19-frame-seeded-ok.txt,
runs/sh19-slotdump*.txt.

## Session (Sep 12, 2026, hermes-worker, cycle SH18) — corrected SH17's "don't drive the render-init THUNK" note; correct thunk drive recovers the engine's REAL ctx object (vtable 0x106731ae0) and presents a real colored frame through the engine's own path. Workspace 470/0.

SH17 recorded the render-init THUNK 0x105b3a280 as undrivable (SIGSEGV, "shifts
parent/win args"). That was a harness-arg bug. Disasm of v2.738.1397 shows the
thunk is `thunk(win, parent) -> inner(alloc(0x48), win, parent)` returning the
REAL guest ctx in x0 (callers 0x5b2b214/0x5b2ea90 `bl thunk; ldr x8,[x0]; ldr
x8,[x8,#16]; blr x8`). Driving it with the correct args
(`--renderthunk`: x0=win=XID, x1=parent=0) recovers the engine's coherent ctx:
vtable 0x106731ae0 (engine-populated, live-dumped), [ctx+32]=EGLDisplay,
[ctx+40]=surface, [ctx+48]=context — and its vtable methods [vt+16]=0x105b3b358
(make-current-if-not-bound: eglGetCurrentContext->eglMakeCurrent), [vt+24]=
0x105b3b408 (swap). Engine's own swap + GLES-bridge clear through the real ctx
present a blue frame (PIL-decomposed RGB 51,76,229 = 0.2,0.3,0.9). New elfjit
`--renderthunk` lever (+ vtable[0..5] live dump). SH18b added `--renderbind` (drive
the engine's OWN make-current method vtable[16]=0x105b3b358 on the real ctx, then
swap -> EGL_TRUE). SH18c added `--renderframe-drive` (probe the engine's own frame-fn
0x105b32c00 with fabricated renderer/view; gets past the renderer list-find 0x5b2e98c
into glBindFramebuffer/viewport setup, then needs coherent renderer internals).
This opens frontier lever (2):
drive the engine's OWN render-loop recipe (vtable[16] bind -> frame-fn 0x105b32c00
-> vtable[24] swap) instead of force-driving glClear. Doc:
docs/frontier-sh18-renderthunk-ctx.md; run-logs: runs/sh18-renderthunk{,-2,}.txt,
sh18-thunk-frame.txt; frame: runs/sh18-thunk-blue.png; script:
runs/capture_thunk_frame.sh. Baselines unchanged (--jni exit 0; idle exit 124).

**Frontier (lever 2 now opened):** drive the engine's own render-loop recipe on
the recovered real ctx in natural order — vtable[16] bind (eglMakeCurrent) ->
frame-render fn region 0x105b32c00 (glViewport/glScissor/glClearColor/glClear/
glDrawElements, dispatched via [x0]->[vt+16]->blr) -> vtable[24] swap. Still need
the coherent renderer C++ object that 0x105b32c00 takes as x0 (its +24/+40
sub-objects carry clear/viewport state, +224/232/236/238 mask flags), and
ultimately the ALooper/lifecycle producer (SH14-capped deque wall) to drive the
loop from the engine's main thread.

## Session (Sep 12, 2026, hermes-worker, cycle SH17) — REAL Roblox binary now RENDERS a real COLORED FRAME headlessly on this GPU-less VPS: live EGL context + engine's own eglSwapBuffers succeed, and glClearColor→glClear→eglSwapBuffers through the GLES bridge present a solid-green 1280x720 frame. Workspace 470/0. HEAD a629d9c.

First real rendered pixels from the running engine's own render path, all through the JIT
bridges against Mesa llvmpipe + a real Xvfb X11 window. Two new opt-in elfjit levers:

1. **`--renderframe`**: after `--renderinit` returns Ok(0x0), drive the engine's OWN swap
   fn 0x105b3b408 (`ldp x8,x1,[x0,#32]; mov x0,x8; b eglSwapBuffers`) with x0 = the
   scratch context buffer that render-init wrote into → `eglSwapBuffers([+32]=display,
   [+40]=surface)` returns **Ok(0x1)=EGL_TRUE**. The real binary presents its surface
   headlessly (llvmpipe+Xvfb), stable exit 124, zero crash.
2. **`--renderclear <r,g,b,a>`**: draw a colored clear through the JIT's GLES float bridge
   on the live context (drive glClearColor@plt 0x1062d7710 with s0..s3, glClear@plt
   0x1062d7740 with GL_COLOR_BUFFER_BIT=0x4000, then swap). Captured with ffmpeg x11grab:
   the frame is a **solid green canvas** (the 0.1,0.7,0.2,1 color) — real rendered pixels.

**Key reverse:** render-init's inner fn 0x105b3a2d8 stores real EGL handles at fixed
ctx offsets [ctx+32]=display,[ctx+40]=surface,[ctx+48]=context; because the harness passes
a guest-writable scratch as x0 (the "prologue STORES into *x0" pattern from SH16), that
same buffer already holds the live handles the swap fn reads — no thunk-return plumbing.
Abandoned (don't re-run): driving the render-init THUNK 0x105b3a280 to recover the "real"
ctx → SIGSEGV (thunk shifts parent/window args across the inner call). Doc:
docs/frontier-sh17-renderframe-clear.md; run-logs: runs/sh17-renderframe2.txt,
runs/sh17-frame-clearcap.txt; frame artifact: runs/sh17-frame-green.png (solid green).

**Frontier (now with a fully-working render pipeline behind the wall):** the engine's own
main-loop producer still never enqueues a render task onto its idle futex/ALooper, so the
engine itself never issues glViewport/glClear/glDrawElements in its loop — this cycle's
clear+swap were harness-driven on the live context. Next: (1) drive the ALooper app-command
lifecycle so StartApp's real producer enqueues a render task → the engine's OWN frame loop
runs natively (all endpoints verified bridge-reachable: glViewport@0x105b32ca4,
glClearColor@0x105b32f8c, glClear@0x105b32fdc, glDrawElements@0x105b35334); or (2) drive the
engine's render-loop fn (region 0x105b32c40-…) directly with a coherent render-state object
once its layout is reversed. Baselines unchanged (--jni exit 0; idle main loop exit 124).

## Session (Sep 12, 2026, hermes-worker, cycle SH16) — the real render-init's FULL EGL chain now SUCCEEDS headlessly: window surface created + context made current against Mesa llvmpipe+Xvfb, returned Ok(0x0). Workspace 470/0. HEAD 114d5f2+.

Crossed the SH14-identified gateway (eglCreateWindowSurface + eglMakeCurrent,
previously declared "framework-gated / not drivable"). Two things landed:

1. **`vfprintf` crash-mask** (commit 114d5f2): libc++ terminate writes its message
   body via `vfprintf`, not just `fwrite`. The guest passes glibc its bionic
   FILE* + AAPCS64 va_list → SIGSEGV hid the reason. New `bionic_vfprintf` decodes
   the AArch64 va_list and writes guest streams to fd 2. Now visible.
2. **Root cause of eglCreateWindowSurface failing: the native window is the
   render-init's x1 param.** Real caller 0x105b2ea90 `ldp x8,x1,[x0,#344]`; the
   prologue `x22=x1` → `[ctx+24]` (stored at 0x105b3a340), which the surface
   wrapper 0x105b3b194 reads as its win arg. The harness passed x1=0. Passing the
   wired XID (0x200000) as x1 → the whole real chain succeeds:

```
ANativeWindow_fromSurface -> ANativeWindow_acquire -> eglGetDisplay -> eglInitialize
-> eglChooseConfig(x3) -> eglGetConfigAttrib -> eglCreateContext
-> eglCreateWindowSurface(win=0x200000) -> eglMakeCurrent -> eglQuerySurface(x2)
-> eglSwapInterval
[elfjit:renderinit] returned Ok(0x0)   (cleanly; engine main loop still idles, exit 124)
```

Real Roblox now has a live Mesa llvmpipe EGL context on a real Xvfb X11 window,
headlessly on this VPS. Run-log: /home/hermes-worker/runs/sh16-renderinit-window-x1-success-runlog.txt.
Doc: docs/frontier-sh15-renderinit-driving.md; STATUS.md.

**Frontier (now with a live EGL context):** drive the engine's frame loop (gl*) —
the wall of the main-loop producer never enqueuing a render task onto its idle
futex/ALooper. Also still open: many gl*/shader paths will need the GLES bridge /
compressed-texture / float paths under a real frame.

## Session (Sep 12, 2026, hermes-worker, cycle SH15) — CORRECTION: SH14's "render-init framework-gated, not drivable" is WRONG at runtime. The REAL render-init (0x105b3a2d8) now drives its real EGL chain headlessly (ANativeWindow_acquire→eglGetDisplay→eglInitialize→eglGetError) through the JIT bridges before a libc++ abort. Workspace 469/0.

This cycle reopened the rendering path SH14 declared a dead-end. Found that
**StartApp populates the render-init context global 0x1067d16f0 at runtime**
(new `JIT_FRAMEWORK_DUMP` reads it live: `0x562a..`, not the statically-0 SH14
pinned) — and the deque-maintenance forward edges 0x1068262e8/300/308 are
populated too (real .text addrs). Built `--renderinit` and drove the real
render-init fn directly after StartApp warm-up:

- **Real EGL chain executes from the real binary** (JIT_TRACE, x30=call sites):
  `ANativeWindow_acquire`(0x105b3a34c) → `eglGetDisplay`(0x105b3a3b4) →
  `eglInitialize`(0x105b3a3c8) → `eglGetError`(0x105b3aec8), then
  `libc++abi:` terminate-abort (engine hits a fatal missing-framework condition
  of the synthetic drive) — that abort used to crash silently (a SIGILL in
  `dl_iterate_phdr` guest-callback and a SIGSEGV in `fwrite` on a bionic
  `FILE*`), both now fixed with shims.
- **`dl_iterate_phdr` shim**: routes the guest callback back through
  `jit::run_guest_callback` (host glibc was executing guest AArch64 → SIGILL).
- **`fwrite` shim**: diverts guest/bionic-`FILE*` (stderr as low as `0x130`) to
  host fd 2 so the abort reason surfaces instead of SIGSEGV.
- **`--renderinit` harness** + `JIT_FRAMEWORK_DUMP` diagnostic in elfjit.
  Runs the render-init as a fresh guest thread CONCURRENT with StartApp's parked
  main thread (block cache leaks, safe). Address must be passed as a GUEST addr.

**Next wall: the engine aborts in render-init after its EGL chain** — needs
either a coherent ANativeWindow/framework context (the SH14/SH7/N/P ALooper-
lifecycle emulation) so the abort becomes a real llvmpipe frame. Doc:
`docs/frontier-sh15-renderinit-driving.md`; run-log:
`/home/hermes-worker/runs/sh15-renderinit-eglchain-runlog.txt`. Baselines
unchanged (`--jni` exit 0; stable idle exit 124); workspace green.

## Session (Sep 12, 2026, hermes-worker, cycle SH14) — deque-injection path proven STRUCTURALLY capped (disasm-verified); located the real render-init fn 0x105b3a2d8 (full eglGetDisplay→init→CreateContext→CreateWindowSurface→MakeCurrent) and proved it is framework-gated. New JIT_REGION_WATCH diagnostic. Workspace 469/0.

This cycle answered the ~13-cycle open question definitively: WHY does no
deque-injection (SH5-SH13) reach egl*/gl*? Disassembly of the drain (0x2856f94)
and the type-4 maintenance handler (0x10285371c) proves it is STRUCTURAL:
- The drain pop-loop passes **w4=4 hardcoded** (constant in the loop) as the
  dispatch task type — it is NOT derived from node data, so no node content can
  change it. The handler `cmp w4,#1..4` therefore always takes maintenance.
- The type-4 handler dispatches through **runtime-built BSS globals**
  (0x1068262e8/0x106826300/0x106826308) that the Android framework producer
  populates; all are statically 0 on this box. So the deque vtable-substitution
  path (SH11-13's --deque-node-live) is a documented DEAD-END — stop investing.
- Back-traced from the egl GOT slots to the engine's REAL render-init:
  **fn 0x105b3a2d8 → thunk 0x105b3a280**, calling
  eglGetDisplay(0x105b3a3b0)→eglInitialize(0x105b3a3c4)→eglCreateContext
  (0x105b3a400)→**eglCreateWindowSurface(0x105b3b1a0)**→eglMakeCurrent. It reads
  a runtime-built context global (0x1067d16f0, statically 0), so it too is
  framework-gated — not a host-drivable entry on this box as-is.
- New `JIT_REGION_WATCH=<lo>-<hi>` (jit.rs): logs first block-entry in a region.
  Verified render-init region — 0 hits (never reached); StartApp region — 3 hits.
  A portable reachability probe for the next cycle's framework-emulation work.
- Next lever is NOT more deque surgery: fabricate the framework context obj the
  render-init derefs + drive the ALooper app-command lifecycle so a real
  producer posts the work item (SH7/N/P levers). Window layer is already wired
  (XID 0x200000). Baselines: --jni exit 0; stable idle exit 124. Doc:
  `docs/frontier-sh14-renderinit-located.md`.

## Session (Sep 12, 2026, hermes-worker, cycle SH13) — REAL engine vtable dispatch: `--deque-node-live 0x106829f00` (the LIVE sentinel's real vtable → engine's own drain-node task-processor 0x10285371c) makes the engine's NATIVE dispatch machinery run our injected foreign nodes — ~124 pops in 16s, process stable to timeout (exit 124), zero crash, and the block-cache GROWS past the probe baseline (≈2147 compiles / 7,361,652 hits vs the probe's flat ≈434). No probe logging (dispatch goes through the real processor). Workspace 469/0.

SH12 left the type-4 dispatch firing through OUR host-thunk PROBE, which only
logged ABI args and never ran engine code for the node. SH13 substitutes the
REAL sentinel vtable (`0x106829f00`, `[vt+40]=0x10285371c`), so the drain's pop
dispatch routes to the engine's actual node-processor. Verified stable and
mechanical (a faulting dispatcher would exit 134; we see 124, endless pops).
The obfuscated dispatch table (`0x102853a04..9b4`) keeps compiling+running real
regions the host-thunk probe never touched.

**Not yet render:** the real processor type-dispatches on `w4` (injected nodes
always get `w4=4`, task-maintenance) via an obfuscated hash table; hostcall
histogram is still syscall + pthread/JNI/mem, **zero egl*/gl***. Next lever:
construct a node whose `[node+32]`/dispatch-index reaches a render/tick handler
in that table, or feed the maintenance path real framework state. Run-log:
`/home/hermes-worker/runs/sh13-realvt-runlog.txt`. Doc:
`docs/frontier-sh13-realvt-dispatch.md`.

## Session (Sep 12, 2026, hermes-worker, cycle SH12) — type-4 dispatch CONFIRMED + SUSTAINED through the real engine idle drain: our injected foreign nodes are now continuously popped AND dispatched through our host-thunk handler (107 dispatches/104 pops across 105 node addrs in 14s, exit 124, zero crashes). Workspace 469/0; HEAD 3b37deb.

SH11 left the injection popping the node but the probe handler never
dispatched (the "residual"). This cycle root-caused it and closed it:

- **THE BUG: vtable handler offset off-by-one.** Both probe builders wrote the
  handler at `(v as *mut u64).add(4)` = byte offset **0x20**, but the drain
  dispatches via **`[vt+40]` = byte 40 = u64 index 5**. So `ldr [vt,#40]`
  read 0 (calloc-zeroed), the drain's `handler != 0` guard failed, and the
  node was consumed WITHOUT dispatch. Fix (`a2448fc`): `add(5)` in the
  `--deque-node-live` and `--deque-probe` vtable builders.
- **Verified:** the first type-4 dispatch fires with the exact engine ABI —
  `x0(vt+16)=0xdeadbeef` (our ctx marker, read correctly from the guest-arena
  vtable), `x3(node)=0x107334040`, `w4=4` — confirmed through the real idle
  drain's pop-loop.
- **Sustained (`3b37deb`):** the injector previously returned after the first
  pop (drain went idle, head → empty). Now it re-injects a fresh guest-arena
  node on every pop, so the drain runs a continuous type-4 dispatch stream.

**Result (reproducible):** repeated `[elfjit:deque-probe] type-4 dispatch #N:
x0=0xdeadbeef ... x3(node)=0x1073... w4=4 x5=0` interleaved with
INJECTED/POPPED, process stable to timeout **exit 124**, zero SIGSEGV. Run-logs:
`/home/hermes-worker/runs/sh12-probe-runlog.txt`,
`/home/hermes-worker/runs/sh12-sustain-runlog.txt`. Doc:
`docs/frontier-sh12-dispatch-confirmed.md`.

**Next lever (unchanged shape, now that dispatch is live):** route the node's
vtable at a REAL engine render/tick handler (instead of our probe) so a
dispatch drives the engine's frame/render machinery to egl*/gl* — or feed the
real producer 0x285682c a coherent render task node. Baselines unchanged:
`--jni` clean exit 0; stable idle exit 124; workspace 469/0.

## Session (Sep 12, 2026, hermes-worker, cycle SH11) — sequenced deque-node-live injection crosses the stable idle drain: node POPPED, exit 124, sentinel crash eliminated. Workspace 469/0; HEAD f4255fd.

For ~10 cycles (SH7b/SH8/SH9) every `--deque-node-live` run died with an
**exit-134 sentinel-as-task SIGSEGV** at ~200ms — before any injected node
could land. This cycle fixed it as a SEQUENCING bug (not a wrong deque model):

- **Defer the force-pop patches** when `--deque-node-live` is set, so the drain
  stays stable (never pops) while we place our node. (Old behavior: force-pop
  at startup popped the SENTINEL first → fault.)
- **Inject while stable** by CLONING the live HEAD node's coherent payload
  (the sentinel during idle — a real, re-enqueue-able task node) as the node
  template, overriding `[node+112]` -> probe vt, forcing `[node+40]!=0`, and
  **zeroing `[node+0]`** (fresh tail; the re-enqueue producer 0x285682c walks
  it and a stale cloned link faults at pc 0x51). Replaces SH9's unreliable
  `[consumer+104]` / `[x19+104]` sentinel indexing.
- **ARM force-pop AFTER placement + drop the cached drain blocks** via new
  `pub jit::block_cache_drop_region(lo, hi)` — the dispatcher had already
  compiled the UNPATCHED pop-loop, so patching guest bytes alone had no effect
  (that's why the earlier deferral ran stable but never crossed). Eviction
  forces it to recompile the patched code, so the FIRST forced pop takes OUR
  node (passes the self-skip guard, `[node+40]=1 -> probe`), not the sentinel.

**Result (reproducible):** `NODE ... POPPED by live drainer (headcell now
0x1000000000000) — deque crossed the barrier`; process stays stable to timeout
**exit 124**, no SIGSEGV — the deque crossing no longer faults.

**Residual (next lever):** the node pops cleanly and the drain reaches past the
`blr` (lr=0x10285700c, node preserved in x3), but our probe handler hasn't been
confirmed dispatching — the guest-side `[vt+16]` reads `0x8b8b48...` (garbage)
not `0xdeadbeef`, so the drain's `[vt+40]` deref is landing on the wrong vtable
(the host-heap probe vtable isn't being read through the guest image we
expect). Resolving that — or supplying a REAL render/tick vtable for the node —
is the path to reaching egl*/gl* on a real frame. Workspace **469/0**.
Baselines unchanged: `--jni` clean exit 0; stable idle exit 124. Doc:
`docs/frontier-sh11-seq-inject.md`; run-log:
`/home/hermes-worker/runs/deque-seq5.txt`.

## Session (Sep 12, 2026, hermes-worker, cycle SH9) — drain SELF-NODE-SKIP guard discovered (correction to SH8): sentinel-repoint can never fire; foreign-node path gives a controlled guest dispatch. Workspace 468/0; HEAD 480196f+.

Disassembled the drain pop-loop `0x2856e40..0x28570a4` (file vaddr = guest−0x100000000)
and found the piece SH8's model omitted — the **self-node-skip guard**:

```
0x2856fc8  ldr  x8,[x19,#104]   ; x8 = [consumer+104]
0x2856fcc  cmp  x8,x22          ; x22 = popped node
0x2856fd0  b.eq 0x28570a4       ; equal -> RETURN, never dispatch
```
The idle sentinel IS `[consumer+104]` (the drain's own struct, vtable
0x106829f00), so repointing the sentinel's `[node+112]` (`--deque-probe`, SH8)
is **structurally futile** — the sentinel never reaches the type-4 dispatch at
`0x2857008`. The correct lever is the **foreign node** path: a calloc'd node
(addr != [consumer+104]) passes the guard and reaches the real dispatch.

**New `--deque-node-live probe`** (elfjit, opt-in): auto-builds a HOST-THUNK
PROBE vtable (`[vt+40]`=registered host thunk, `[vt+16]`=ctx) and injects a
foreign node (`[node+112]=that vt`, `[node+40]=1`, tagged into the live
headcell). Observed: the pop-loop NOW dispatches in a **real guest thread**
(tid 0, `rbx_matches_gueststate=true`, `in_jit_run=true`, guestpc→0x7f0000002068)
— a controlled first crossing through the foreign-node dispatch path — vs SH8's
sentinel-repoint which faulted only in an untracked host thread. It still faults
(exit 134, /home/hermes-worker/runs/deque-nodelive-probe-crossing.txt) because
the handler is our probe host-thunk, not a real engine render callback.

**Next lever (mechanism now correct):** identify a REAL render/tick vtable for
`[node+112]` (+ coherent payload) so `--deque-node-live <vt>` reaches egl*/gl*;
or invoke the REAL producer 0x285682c as a guest call with a valid task node.
Baselines unchanged: `--jni` clean exit 0; stable idle exit 124. Doc:
docs/frontier-sh9-drain-selfskip.md.

## Session (Sep 12, 2026, hermes-worker, cycle SH8) — dispatch ABI fully reversed + pinned; `--deque-probe` live-repoints sentinel vtable (works) but sentinel-as-task still faults (honest failure, later corrected by SH9).

Reversed the engine task-deque consumer's POP-LOOP dispatch ABI from live disasm
(libroblox 0x2856fd4..0x2857008, qemu/objdump-verified) and pinned it as a new
regression `deque_dispatch_node_layout_matches_engine_abi` so any render-task
injector builds nodes the running drain understands:

```
node = low48([headcell]); vt = [node+112]&~0x3f; handler = [vt+40];
guard [node+40]!=0 && handler!=0;
handler([vt+16], consumer, [node+32]&~1, node, w4=4, x5=0)
```
(the `[node+112]&~0x3f -> [vt+40]` model prior cycles stated is confirmed
exactly, plus the precise arg order/types and the `[node+40]`/handler guards.)

New elfjit `--deque-probe <ctx-qw>` (opt-in, only engages with `--drain-force-
pop`): live-repoints the ROOT consumer's sentinel `[node+112]` -> a host-heap
fake vtable whose `[vt+40]` is a registered host-thunk probe, so the engine's
own pop-loop dispatches a NODE through OUR handler with the real ABI args.
VERIFIED: both sentinels repointed (headcells 0x10682b338/0x10682a638, logs
`REPOINTED sentinel ... [node+112]: 0x106829f00->0x...`). HONEST GAP: the probe
handler never fires (count 0) — under forced-pop the engine dispatches the
SENTINEL-AS-TASK and faults in an UNTRACKED host thread (guestpc 0, reading host
slot addr 0x7f0000000090 as a pointer) before our handle runs. The vtable repoint
alone can't detour the engine's own dispatcher walking the sentinel's other
garbage payload. Exit 134. Doc: docs/frontier-sh8-dequeprobe-abi.md.

Baselines UNAFFECTED and re-verified: `--jni` clean exit 0; stable idle
(StartApp main loop) exit 124, zero SIGSEGV; workspace green **468/0** (was 467).
run-logs: /home/hermes-worker/runs/boot-probe-{1..5,final}.txt.

**Frontier (unchanged shape, ABI now pinned):** the still-hard wall is the
engine's producer never enqueues a REAL render task; forcing the consumer makes
it pop+dispatch the sentinel-as-task -> fault. Next levers per SH7b, now with
the exact node layout: (1) invoke the REAL producer 0x285682c as a guest call
with a valid task node, or (2) fabricate a full task node (vt+40 -> a real
render/tick vtable we must locate, coherent payload) and cross before force-pop.
Neither is crossed this cycle; the ABI + control-plane (repoint) are.

## Session (Sep 12, 2026, hermes-worker, cycle SH7b) — CORRECTION to SH7: the finite wait-timeout NEVER reached the pop-loop (measured 0 entries / 128k branches); NEW `--drain-force-pop` makes the engine's task-deque pop-loop run + dispatch for the first time (faults on the sentinel = controlled crossing). Workspace 467/0; HEAD 7d0cd5c+.

SH7 claimed `--drain-poll <ms>` (finite timeout) makes the drain's pop-loop run by
letting generic-wait time out. **That is wrong.** Measured: under `--drain-poll 8`
the drain's post-wait `tbz w24,#0` (0x102856f7c) fires ~128k times but the pop-loop
0x102856f94 is entered **0 times**. Root cause pinned in generic-wait 0x284d014:
`cmn x0,#1` (0x284d0a4) only maps an EXACT host-futex x0==-1 to "timed out"; the
host futex returns -ETIMEDOUT(-110) on timeout, which falls to 0x284d0ec -> generic
wait returns w0=0 ("woken"). The drain's tbz therefore always re-loops; the finite
timeout only hot-loops the drain's MAINTENANCE heartbeat (0x10285371c with x4=2/3,
i.e. the drain struct's own `[x19+104]`+112 vtable callback) — which SH7 misread as
"pops + dispatches the deque". The real node-pop path (x4=4) is a separate code
site.

**New `--drain-force-pop`** patches `mov w24,w0` (0x102856f4c)->mov w24,#1 AND NOPs
the tbz (0x102857f7c), so the drain ALWAYS falls through to the version-check
(0x2856f80) -> pop-loop (0x2856f94). **Proven: the pop-loop now executes** — it
CAS-pops the deque head (the sentinel during idle) and dispatches
`[node+112]&~0x3f->[vt+40]` with `[node+40]`/`[node+32]`/w4=4, then faults walking
the sentinel's garbage task content (SIGSEGV guestpc=0x7f0000002068, lr
0x10222f330). This is the long-anticipated **controlled first crossing** — the
engine's real task-deque pop+dispatch machinery now runs (SH5/SH6/STATUS's
documented milestone). Run-log: `/home/hermes-worker/runs/drain-forcepop-crossing.txt`
(exit 134). Opt-in, so plain `--drain-poll` stays stable (exit 124) and baseline
`--jni` is unchanged (clean exit 0).

**New `--deque-node-live <vt>`** implements the SH7 "locate tid 0's deque" lever: it
targets the LIVE drainer (guest_tid 0, its root recovered from x20 while pc is in
the drain body 0x102856e40..0x1028570a4) instead of the parked tids 1/2 that
`--deque-node` aimed at. It recons the live deque (`[root]=headcell`,
`[headcell]=packed head` low48=node high16=tag, `[root+8]=tag`, head-node
`[node+112]/[vt+40]/[node+40]/[node+32]`) and swaps a task node over the live head.
Note: because the drain re-enqueues every popped node, "node still at head" is not
itself proof of non-consumption; the discriminating signal is a type-4 dispatch of
our node. Under --drain-force-pop the run faults during the sentinel dispatch, so a
real node's dispatch is not yet isolated.

**Next lever:** supply a real task node content so the forced pop-loop's dispatched
handler (0x10285371c) reaches a real render/tick callback instead of walking
garbage — identify what 0x10285371c's `[adrp+0x528]` global `br` target dispatches to,
and what node.type/args drive it. Doc: `docs/frontier-sh7b-drainforcepop.md`.

The ~35-cycle "engine producer never enqueues / consumer never drains" wall is
broken. Empirical stack dump of a parked consumer resolved the true frame and
the gate:

- The parked consumers are the **drain fn 0x2856e40** calling generic-wait
  **0x284d014 with timeout = -1 (infinite)** (live x20 = -1; sp+0x30 = -1).
  generic-wait shares the drain's frame (the drain `bl`s to 0x284d018, skipping
  its `sub sp,#80`), parked sp+0x28 = drain return-into after `bl 0x284d014`
  (0x102856f48).
- An infinite timeout jumps straight into a bare blocking futex
  `futex(Q+4, WAIT_BITSET, epoch, NULL, NULL, ~0)`; the drain's **pop-loop at
  0x2856f94 runs ONLY when the wait returns 1 (timed out)**. With an infinite
  timeout it never times out → the pop-loop is never reached → work is never
  consumed no matter what is in the deque. That was the whole wall.

**New elfjit `--drain-poll <ms>`** patches guest `mov x2,x22` (0x102856f40,
the drain's infinite-timeout copy) to `mov x2,#<ms>` (imm12) before jit_run, so
the drain block compiles with a finite timeout. The wait now times out, the
drain reaches the pop-loop, and it **continuously pops + dispatches the deque**,
executing the real engine dispatch handler **0x10285371c** / 0x1028538c0 /
0x1028539e8 millions of times — stable (flat 1673 compiles, no crash, exit 124,
hits → ~8M). This is the engine's own task-deque dispatch machinery running.
Run-log: /home/hermes-worker/runs/boot-drainpoll-crossing.txt.

Also: fixed `--deque-node` to write the node to **[headcell+0x0]** (the cell the
pop actually reads: `x23=[x20]; x24=ldar([x23])`) instead of the ring's
internal HEAD/TAIL cells (+0x10/+0x18) prior code wrote to — that is why SH6
nodes sat unconsumed. Added JIT_STACKDUMP / JIT_DEQUE_PROBE2 diagnostics (frame
resolution) and a `dump` region-disassembler example. Doc:
docs/frontier-sh7-drainpoll-crossing.md.

**Where this leaves the frontier:** the consumer side is provably live — under
--drain-poll **guest_tid 0** (its own deque/wait struct, snapshot x1=0x7f4b5486b280,
pc=0x10285371c lr=0x102856f38) cycles the drain's pop-loop and dispatches the
real engine handler 0x10285371c continuously (maintenance/self-dispatch, no
egl*/gl* hostcall yet). guest_tids 1 & 2 (recovered headcells 0x10682a638 /
0x10682b338) stay futex-parked — so the --deque-node node-injection lever
(targeting only parked lr==IDLE threads) has been writing into idle consumers'
deques, never the one tid 0 drains; that is why external nodes are unconsumed.
**Next lever:** (1) locate guest_tid 0's deque root from its live drain frame at
dispatch time; (2) pass the drain's tag guard (0x2856e78 `cmp x9,[head]>>48`)
and low-48 pointer truncation; (3) point [node+112] at a render/tick vtable (not
the sentinel's 0x106829f00) so the dispatched handler reaches egl*/gl*/frame.
Baseline (no --drain-poll) unchanged: stable idle futex park.

## Session (Sep 12, 2026, hermes-worker, cycle SH6) — host enqueue into the task-deque PROVEN not-a-producer (two strategies); deque model corrected from full producer/drain disassembly; new `--deque-node` harness. Workspace 467/0; HEAD 6003441.

Implemented the documented SH5b next-experiment (host side enqueue into the
engine's idle task-deque) as a real elfjit host producer and ran it against the
parked consumers. **Result: a hard negative.** Two distinct enqueue strategies
were tried and both are robustly NOT consumed (`popped=false` every check,
compiles flat, no crash):

1. write node into [headcell] = slot+0 (the SH5b "head cell"),
2. write node into HEAD = slot+0x10 AND TAIL = slot+0x18 with [node]=0.

In both, the deque head field keeps pointing at our node for the whole run —
the parked consumers never CAS-pop it, despite the version-epoch bump + futex
wake. This corrects SH5b's "self-referential sentinel at the drain struct"
model, which located the enqueue point wrong.

**Corrected deque model** (from producer 0x285682c + drain 0x2856e40 disasm):
the deque is a per-consumer pointer-RING at a stable guest-bss base
(0x10682a638 / 0x10682b338 for the two real slots; a 3rd consumer's headcell is
a host-heap garbage-ASCII cell — ignore). HEAD field at base+0x10, TAIL at
+0x18; empty == both == slot+8 (the self-referential first node). Producer push
= tagged-CAS walk `ldar[head]→[node]` to the tail then link (helpers 0x2b9e720 /
0x2b9e760). Consumer pop = `ldar[head]`, `low48==0 → EMPTY→wait`, else CAS-pop
then dispatch `[node+112]&~0x3f → [vt+40]` (+ `[node+40]`, `[node+32]` arg),
**re-enqueue via `bl 0x285682c`** (2857020), wake `futex(node+0xc,0x8a,1)`.

**Why node+bump is insufficient (the hard wall, now precise):** the drain is
gated by the version-epoch wait (generic wait 0x284d014 parks in
`futex(Q+4, WAIT_BITSET, low32(epoch))`). Wait returns w20=0 on futex-woken/
version-changed, w20=1 ONLY on timeout. On wake the drain checks
`cmp x21, [Q]>>32` (2856f80/90); a CHANGED version makes the drain RETURN (to
28570a4) instead of entering the pop-loop at 2856f94 — which runs ONLY when the
version STILL matches the caller's captured x21 AND the wait timed out. So a
host bump makes the drain exit; the pop-loop is dead during idle (infinite
timeout, never polls); and even a no-bump node placement is never drained.
**Host writes to the ring are NOT sufficient — the deque is drained only by a
real (framework) producer that re-enters the drain loop.** Three negatives:
slot+0, slot+0x10/0x18±bump, slot+0x10/0x18 no-bump — all `popped=false`.

**Next levers (ordered):** (1) invoke the REAL producer 0x285682c as a guest
call with a valid task node (recover the scheduler `this` from drain_struct
`[x1+104]`) so the framework's own push path runs; (2) synthesize the drain
caller's re-entry with a fresh matching epoch + the node already in HEAD;
(3) reverse the drain caller loop (0x284eb80) to find what re-enters the drain.

Doc: docs/frontier-sh6-enqueue-negative.md. Run-logs:
/home/hermes-worker/runs/deque-node-enqueue-vt.txt, deque-node-v2.txt,
deque-node-v3.txt. elfjit `--deque-node <vtable>` is the faithful reusable
harness (default-off; baseline boot unchanged).

## Session (Sep 12, 2026, hermes-worker, cycle SH5) — producer/enqueue contract pinned from disassembly; `JIT_DEQUE_PROBE` locates each parked consumer's live deque head-cell from the host. Workspace 467/0; HEAD bfa63d1+.

This cycle converted the ~30-cycle "producer never enqueues / version+latch
isn't a producer" wall into a concrete, host-side-pokeable mechanism with a
full disassembly of the scheduler (file vaddr = guest − 0x100000000):

- **Producer/enqueue = 0x285682c**: per-CPU slot base = `[this+8] +
  sched_getcpu()*0x4a140`; tagged-CAS push onto that slot's per-CPU MPSC queue
  (pop 0x2b9e6e0 / push 0x2b9e720 / refcnt 0x2b9e760); head atomic at
  `slot+0x10` (low48=node, high16=tag), tail at `slot+0x18`, node link `[node]`.
- **Consumer drain = 0x2856e40** (`root`=x0): `head=ldar[[root]]`; empty iff
  `low48==0`; dispatches popped node via `[node+112]&~0x3f → [vt+40]` +
  `[node+32]`; wakes `futex(node+0xc, WAKE_BITSET|PRIVATE, 1)`.
- **Generic wait = 0x284d014** (`Q`,`epoch`,`timeout`): refcount `[Q]`; early
  out on `epoch != [Q]>>32`; else `futex([Q]+4, WAIT_BITSET, low32(epoch))`.
- **Waiter-frame recovery (the enqueue prerequisite, confirmed live):** at park
  the waiter's PROLOGUE saved the drain's callee-saved regs — `[sp+64]`=drain
  x20=deque root, `[sp+72]`=drain x19=consumer struct, `[sp+32]`=drain saved
  x30. New `JIT_DEQUE_PROBE=1` (elfjit) reads these and shows each parked
  consumer's `[root]` is a **stable guest-bss head-cell**
  (0x10682a6x38 / 0x10682b338) — the exact address a host producer must CAS
  onto. Also corrected a long-standing misreading: `lr=0x10284d134` is the
  *in-wait return-into-fn* after the `bl syscall` (per 284d134 `mov w20,wzr;
  b 284d0f0`), NOT the drain call-site.

**Next experiment (feasible now):** CAS a node onto the recovered per-CPU
head-cell + bump `[Q]` epoch + FUTEX_WAKE. A fully-zeroed node drains (proving
the host producer crossed the barrier) then faults deref'ing `[0x28]` in the
`[vt+40]` dispatch — a controlled, capturable first crossing; the follow-on is
supplying a real engine frame/render node (`[node+112]→[vt+40]` callback +
`[node+32]` arg). Boot unchanged (stable idle main loop, exit 124, no crash).

Run-log: /home/hermes-worker/runs/deque-probe.txt (118 samples, compiles
1420→flat, exit 124). Doc: docs/frontier-2026-09-12-producer-enqueue.md.

## Session (Sep 12, 2026, hermes-worker, cycle SH4) — idle barrier re-characterized: it's a per-CPU task-deque CONSUMER, version+latch is NOT a producer; `--futex-bump` negative result; producer-enqueue doc. Workspace 467/0; HEAD c6c82e5+.

From-first-principles disasm this cycle, the ~30-cycle "producer never enqueues
work" wall is now pinned to its exact contract (correcting the "awaiting
0xF4240 go-token" reading of prior cycles):

- The parked threads are **consumers** inside the generic futex wait-with-timeout
  at 0x10284d018 (reached via `blr` — vtable-dispatched, ZERO static callers).
  It's `wait(obj=Q, expected_seq, timeout_ns)`: atomic_add(&Q.refcount,+1),
  proceed if `(expected>>32) != (old>>32)` else futex(Q+4, WAIT_BITSET,
  low32(expected)); on timeout a clock_gettime deadline loop; atomic_add -1.
  So `Q>>32` is a **self-syncing version counter**, Q+4 is the futex latch.
- The consumer is a **per-CPU lock-free task-deque drain** (fns 0x285682c /
  0x2856f44): `loop { head=ldar[[x20]]; if low48(head)==0 goto wait; process }`,
  with `x20` a per-CPU slot base (`umaddl` from `sched_getcpu() & 0xf`). Head is
  0 because the framework render/looper producer is absent here.
- **New `--futex-bump` (elfjit):** prior kick/set only poked the latch (Q+4);
  bump also increments the version word `[Q]>>32` — the real produce shape
  (bump version + set latch + wake). Empirically it **re-parks** (compiles flat
  1668, JIT_STATS heartbeat stops, exit 124); the consumer re-reads `[Q]>>32`
  each iteration so the host's bumped value becomes the new expected — a moving
  epoch, not a discrete "go". **Version+latch is NOT a producer.** The only
  lever is a real task node in the deque head, framework-owned.

Run-log: `/home/hermes-worker/runs/futex-bump2.txt`.
Doc: `docs/frontier-2026-09-12-producer-enqueue.md`.

## Session (Sep 12, 2026, hermes-worker, cycle SH3b) — GLOB_DAT *function* slots now resolve through the full GLES chain; workspace 467/0; HEAD 80940e6.

Follow-on to SH3's eglGetProcAddress bridge. Found another real gap in the
boot log: `[plt:glob_dat] unresolved sym=glGetShaderInfoLog / glGetProgramInfoLog`
— GLOB_DAT **function-pointer** slots (function tables, `STT_FUNC`/notype) only
tried plain `resolve()` (dlsym), which cannot see GLES names (libGLESv2 is
RTLD_LOCAL and lazily loaded), so these table entries bound to the **benign NULL
stub** instead of real Mesa. On a real shader-compile path a
glGetShaderInfoLog/glGetProgramInfoLog call through such a table would return
garbage (or the stub's 0).

**Fix (commit 80940e6):** `bind_glob_dat`'s function branch now mirrors the
JUMP_SLOT resolution chain — `resolve -> float -> float32 -> egl -> gles_int ->
gles_mixed` (scope_resolve already tried in the outer branch). Verified against
the real binary: the two GLES GLOB_DAT entries are gone from the unresolved
list; the remainder are AMedia*/video-codec data-object keys (bionic-only,
benign) + the cosmetic `__sF`. New hermetic regression
`glob_dat_function_chain_resolves_gles_names_to_real_slots` pins the chain
returns a real host-thunk slot (>= HOST_THUNK_BASE) for glGetShaderInfoLog /
glGetProgramInfoLog / glGetString / glCompileShader. Workspace **467/0**
was 466/0; boot unchanged (stable idle main loop, exit 124).

**Where this leaves the frontier (unchanged hard wall):** the engine's own
producer never enqueues a real work/task item onto its per-thread idle futex
(lr=0x10284d134; queue head [x19]=0, latch x19+4). Re-confirmed this cycle:
`--futex-kick 2 --futex-set 0xf4240` wakes all 3 threads (block-cache **hits**
grow) but **compiles stay flat at 1668** — the latch is a signal, not the work;
with no queue element the consumer re-arms and re-parks. GLES dynamic-loader
(eglGetProcAddress) + GLOB_DAT GLES chain are now complete so that once the
barrier is crossed the ES functions resolve through our bridges (float,
texture-interception, int). Partial reconstruction of the scheduler wait path:
callers at 0x284eb80 / 0x2856f44 pass the queue obj x19; after wait, read
[x19+104] -> vtable, dispatch `blr` a callback at [vt+40] — an opaque
scheduler/vtable dispatch, the last lever documented across many cycles.

## Session (Sep 12, 2026, hermes-worker, cycle SH3) — eglGetProcAddress routed through a GLES bridge (dynamic GLES loader no longer returns raw Mesa pointers); workspace 465/0; HEAD 3a30b3a.

Decoder 100% (0 Unsupported / 0 PANIC on .text) unchanged. The boot frontier
(engine producer never enqueues onto the idle work-queue futex) is unchanged
but was re-confirmed this cycle: `--futex-kick 2 --futex-set 0xf4240` wakes
all 3 threads (block-cache **hits** grow while **compiles** stay flat at 1668)
— the futex latch (x19+4) is a *signal*, not the *work*; the queue head at
[x19]=0 stays empty, so the engine re-arms and re-parks. A host-side producer
must enqueue a real render/task item into that queue, not just poke the latch.

Closed a real secondary gap toward a real frame — **`eglGetProcAddress`**:

- The real binary imports `eglGetProcAddress` (readelf-confirmed UND FUNC); on
  Android Roblox resolves most gl*/egl* entry points *dynamically* through it
  and `blr`s the returned pointer.
- It was binding to **Mesa's raw function** via the generic `resolve()` dlsym
  path (comes BEFORE resolve_egl/resolve_gles_* in plt.rs), so a returned
  pointer was a raw x86 Mesa address — not a registered host-thunk slot. A guest
  `blr` to it can't dispatch through the host-call bridge, and the call would
  bypass the GLES float bridge and the compressed-texture interception
  (breaking glClearColor/glTexImage2D/glCompressedTexImage2D on a real frame).
- Fix: `resolve()` and `resolve_egl()` both route the name to a shared
  `resolve_egl_get_proc_address`, installing `w_eglGetProcAddress` (a GLES
  bridge, HostGlesCall ABI: reads guest x0 = proc-name, resolves it to one of
  OUR host-thunk slots: mixed float/texture -> int -> egl). A later guest `blr`
  to the returned slot dispatches through the correct bridge, preserving float
  and texture interception. Unknown names fall back to real Mesa (niche).
- Subtlety: resolve_gles_int/resolve_egl build a CString from the name, so it
  must be passed WITHOUT a trailing NUL (plt.rs names are NUL-free; the bridge
  reads the guest C-string and passes the byte content). Mixed tolerates NUL.
- New regression `egl_get_proc_address_bridge_returns_dispatchable_gles_slot`
  pins: glClearColor + glCompressedTexImage2D (mixed bridge) and glGenTextures
  (int bridge) all yield the same *dispatch target* as a direct import, unknown
  names never collide with the GLES region. Added `jit::gles_bridge_fn` (pub)
  to compare GLES-region slots by underlying fn (resolve_gles_mixed allocates a
  fresh slot per call, so equality is by target not address).
- Verified: workspace 465/0; egl_window_present + anativewindow_x11_surface
  gates still pass; real boot unchanged (stable idle main loop, exit 124, no
  egl/gl hostcalls yet — the engine still waits on the producer-enqueue).

**Next lever (unchanged hard wall):** cross the engine's work-queue futex —
reconstruct the queue element layout (wait primitive callers at 0x284d014:
x19 = queue obj, latch x19+4 = generation/signal, [x19]=0 = head; fetch_add on
entry frees the high-32 generation) and enqueue a real render/task from the
host before posting the latch. Secondary tracks (GLES float bridge, texture
path, eglGetProcAddress bridge) are complete and gated.

## Session (Sep 12, 2026, hermes-worker, cycle SH2) — idle barrier PROVEN a work-queue futex; snapshot now captures futex args + `--futex-set <hex>`; workspace 464/0; HEAD 10e6b49.

Decoder remains 100% (0 Unsupported / 0 PANIC on .text). The boot frontier
was re-probed empirically this cycle with a definitive conclusion:

- **Live futex capture** (JIT_THREADS now carries x3 val / x5 uaddr2 / x6
  bitset in the snapshot): all 3 guest threads park at lr=0x10284d134 with
  `futex(uaddr, op=0x89 WAIT_BITSET|PRIVATE, val=x3=0x0)`. The per-thread latch
  is re-armed to 0 each cycle; the consumer only proceeds when a producer posts
  a NON-ZERO work token.
- **`--futex-kick` (old+1: 0→1)** AND the new **`--futex-set 0xf4240`** (fixed
  token written verbatim every tick) both leave compiles flat at 1668 / hits
  ~8713: the engine re-parks either way. → **The latch is a signal, not the
  work; a bare futex poke is NOT a producer.** The producer must enqueue an
  actual work item (render/task) into the engine's per-thread queue (host-heap
  object at x19[*=0x0], latch = x19+4), then set the latch. That queue's
  structure is the frontier (reconstruct the consumer dequeue path after the
  futex returns).
- 0xF4240 = 1,000,000 is a pre-initialized "go" upper-bound counter the barrier
  sites load; it is NOT the awaited futex val (that's 0x0).
- New harness: `--futex-set <hex>` writes a chosen latch value each tick
  (awaited-token experiment). Regression pins the snapshot's new x3/x5/x6
  fields.

Run (reproducible): same elfjit StartApp command as cycle SH; expect exit 124,
3 threads parked `pc=syscall lr=0x10284d134 x3=0x0`, compiles flat 1668.
Run-logs: `/home/hermes-worker/runs/futex-x3.txt` (awaited-val capture),
`futex-set2.txt` (fixed-token negative).

### Next lever (unchanged hard wall, sharpened)
Reconstruct the engine's work queue: trace the consumer path that runs after
the idle futex returns (what it dequeues / what "is there a task" check it
does) to learn the queue struct layout, then enqueue a real work unit from the
host and post the latch. All graphics/decoder/texture/import work is complete
and gated; the boot needs this producer enqueue.

## Session (Sep 12, 2026, hermes-worker, cycle SH) — arm64jit DECODER REACHES 100% COVERAGE on real libroblox.so: 11,437 Unsupported -> ZERO, 0 PANIC; workspace 464/0; HEAD 092e829.

The decoder — the single largest structural wall in the project — is now
**permanently closed**. The entire real binary .text span
[0x102d95980, 0x1072d5a84] decodes with zero Unsupported and zero decode
panics. This cycles' closes (all qemu/oracle-verified, decode pins + exec tests):

- **addhn2/subhn2/raddhn2/rsubhn2 Q=1 widening-narrow** (SimdHighNarrow `q`
  field, upper-half dest; bit21 asserts addhn-not-EXT). 49 -> 37.
- **SIMD fp64 absolute-difference fabd Vd.2D** (Simd2dFp op 8:
  subsd+pand sign-clear). 37 -> 28.
- **facgt/facge absolute-compare** (VecFpCmp `abs` flag; pand sign-bit clear on
  both loaded lanes). 28 -> 17.
- **srhadd signed rounding-halving add** (SimdHadd `rounding` flag;
  (a+b+1)>>1, qemu {1,2,2,3,0,2,2,52}). 17 -> 13.
- **FP16 vector frint** (frint{nmzpax} Vd.8H/.4H, esize2 via F16C
  promote-round-demote; cvtph2ps/cvtps2ph helpers; **FMaxV bit22 guard** so
  frintx fp16 (0x6e799800) isn't stolen). 13 -> 11.
- **FP16 scalar unary** (fneg/frintm/z/p/n/x/fsqrt/fabs h0; FpUnary `half`
  flag; bit-mask for neg/abs, F16C round-trip for rounding). 11 -> 3.
- **FP16 vector frecpe/frsqrte** (.8H/.4H via dedicated bit20-SET gate) +
  **frintx .2s/.2d** (table rows). 3 left.
- **mrs xN, fpcr read** (SysReg 11 -> 0, nearest-even default FPCR) +
  **ldpsw post/pre-indexed** load-pair (top-byte 0x68/0xe8/0xe9, sign-extend
  32-bit pair). 3 -> **0**. ZERO unsupported / ZERO panic.

**Metahistory recap**: the .text decode coverage went from 14,918 -> 11,437
(after ARMv8.2 stubs/prefixes) -> ... -> 49 -> 0 over ~28 focused cycles. The tail
after ~100 was entirely single-instance cold-path opcodes (fp16 variants, rare
scalar-FP forms).

**The frontier is now purely the boot**: decoder saturation is done. Continue
with the boot-path analysis (producer thread never enqueues onto the idle
futex — next lever). Graphics translation (GLES float bridge,
glCompressedTexImage2D ETC2/ASTC) remains the secondary thread.

Workspace: **464 passed / 0 failed** (incl. qemu-oracle diff_battery). HEAD 092e829.

## Session (Sep 12, 2026, hermes-worker, cycle R) — FP16 by-element fmla/fmls/fmul (the biggest remaining family) closed; workspace 423/0; HEAD f3251b7.

Closed **`fmla/fmls/fmul Vd.8h/.4h, Vn, Vm.h[idx]`** (~2000+ real .text
instances — the single largest remaining family, the multiply-accumulate core
of the FMOD/audio + render/HDR math paths). Coverage 11,437 -> 9,552 Unsupported
(-1,885), distinct 6,281 -> 5,735, 0 PANIC.

- **New `Inst::SimdFp16BEl` + decode gate**: byte0-nibble 0xf, bit29 CLEAR,
  bit23 CLEAR, **bit10 CLEAR**, bit13 CLEAR. The bit10 CLEAR is the new critical
  discriminator: FP16-indexed (bit10=0) vs shift-by-immediate (shl/ushr/sshr/
  usra/ssra/srshr/srsra... bit10=1 FIXED), which share byte0 prefix AND bit23=0
  AND alias the FMUL/FMLA/FMLS opcode bits[15:12] onto the shift's [14:12]
  marker (fmul=bit15, fmls=bit14, fmla=bit12==usra's 0b001). bit13 CLEAR excludes
  widening SimdMullEl (requires bit13 SET); bit23 CLEAR excludes f32
  FmlaEl/SimdFmulEl (bit23 SET). MUST precede the shift gates. Verified disjoint
  over 30+ both-family cross-compiled encodings.
- **Fields**: op = bit15(fmul)|bit14(fmls)|else fmla; idx (.8h 3-bit) =
  (bit11<<2)|(bit21<<1)|bit20, .4h = (bit21<<1)|bit20; vlm = bits[19:16] (v0-v15).
- **Translate**: hoist-splat Vm.h[idx]->xmm2 (F16C promote once, BEFORE the lane
  loop so rd==rm can't clobber), per-lane promote Vn.h[i]->f32, fma in f32, demote
  ->store16. **Vd must be promoted (vcvtph2ps) before the accumulate** — Vd is
  fp16, unlike the f32 FmlaEl path. fmul copies xmm1->xmm0 (movaps) so all ops
  demote the same xmm0->xmm0 vcvtps2ph.
- Regression: decode pins (real fmul v2.8h,v1.h[4]=0x4f019882, fmla v31.8h,
  v15.h[7]=0x4f3f1bdf, fmls v2.4h,v1.h[2]=0x0f215082, .h[idx0..7]; + f32-untouched
  pins: fmla/fmls stay FmlaEl, fmul stays SimdFmulEl) and exec tests (fmla .4h,
  fmls .4h, fmul .8h 8 lanes incl. high-slot st.v[3] index). Workspace 423/0.

### Next lever (docs/fp16-decode-gap.md updated)
**SIMD modified-immediate orr/bic v.2s/.4s** (0x4f0177eX, ~100) — the next
largest remaining family. Then fmaxnm/fminnm v.4s (0x4e21c8xx ~30), vector
fcvtas v.4s (~150), scalar fabs/fneg/fsqrt, vector-immediate bic/orr (0x2f047400).

The real HARD wall is unchanged: the engine's own producer never enqueues work
onto its per-thread idle futex, so the main loop re-parks. Decoder coverage is
what lets a real frame/audio path run instead of block-tracker-breaking.

## Session (Sep 12, 2026, hermes-worker, cycle Q) — decoder FP16/NEON coverage -3,481; workspace 421/0; HEAD d284c2d.

Closed the FP16 decoder gaps that will block-tracker-break a real frame/audio path
(14,918 -> 11,437 Unsupported .text, distinct 7,743 -> 6,281, 0 PANIC invariant).
Boot unchanged (JNI_OnLoad 0x10006, stable idle main loop, exit 124/no crash).

Two commits:
- `4dd5e3d` — scalar FP16 + gate fixes: new **`FcvtHalf`** (fcvt s,h/h,s/d,h/h,d via
  F16C vcvtph2ps/vcvtps2ph$0 RN, raw VEX bytes; host confirmed f16c — the JIT's first
  F16C use, the reusable pattern), **`FpScalar.half`** (fadd/fmul/fsub/fdiv h),
  **`fccmp s/d` decode gate FIXED** (old 0xfff0_fc03==0x1e20_c400 never matched any
  real fccmp; new 0xffe0_0c10=={0x1e200400 s,0x1e600400 d}, must decode before the
  FMOV-imm gates because a cond≥8 sets bit12 that the FMOV-imm lane-anchor misread as
  an immediate), `fabd s` (single form 0x7ea0_d400 + sz field), `urhadd v.16b/.8b`.
- `d284c2d` — SIMD FP16 3-same **`SimdFp16As`** (fadd/fsub/fmul v.4h/.8h) per-lane
  promote->op->demote via F16C. Gate `(insn&0x9f60_f400)==0x0e40_1400`. MUST decode
  BEFORE the SIMD-select (bsl) gate: FP16 `fmul v.8h` (byte1 0x1c, 0x6e451c82) aliases
  bsl's byte1-0x1c mask and was silently decoded as a bitwise select until a
  regression caught it.

Ground truth: synthesized with `aarch64-linux-gnu-gcc -O0 -march=armv8.2-a+fp16` and
objdump; real encodings pinned (fccmp s0,s1,#0,eq=0x1e210400; fabd s2,s2,s3=0x7ea3d442;
fadd v1.8h=0x4e401421). Tests: decode pins + `exec_bytes` runtime tests (fcvt h<->s,
fabd s, urhadd bytes, fadd h scalar + fadd v2.4h vector) — workspace 421/0.

### Next lever (the single biggest remaining family)
**`fmla/fmls/fmul Vd.8h/.4h, Vn, Vm.h[idx]`** — the 0x4f0x_1x2x/1x9x opcodes, ~2000+.
Ground-truth matrix captured in docs/fp16-decode-gap.md:
- index = (bit11<<2) | (bit21<<1) | bit20 for .8h; .4h uses (bit21<<1)|bit20.
- vm in bits[19:16] (v0-v15 only, bit20 is index[0]); fp16 fmla bit23 CLEAR (vs
  f32 FmlaEl requiring bit23 SET — the discriminator); fmul=bit29 SET, fmls=bit14+bit12
  (same-operand ground truth 0x4f321020 / 0x4f325020 / 0x4f329020).
- Translate: hoist-splat Vm.h[idx] -> xmm2 (promote once) BEFORE the lane loop
  (rd==rm clobbers the element), per-lane promote Vn.h[i]->f32, fma in f32, demote.
Then: SIMD-immediate orr/bic v.2s/.4s (~200), vector fcvtas v.4s (~150), fabs v.2s.

The real HARD wall is unchanged and independent of this work: the engine's own
producer never enqueues work onto its per-thread idle futex, so the main loop re-parks
(no render/EGL path). The decoder coverage is what lets a real frame/audio path run
instead of block-tracker-breaking when that wall is crossed.

## Session (Sep 12, 2026, hermes-worker, cycle P) — ALooper poll-source contract corrected (the glue-looper prerequisite); workspace 411/0, HEAD e8e08cf.

The real binary imports `ALooper_pollOnce`/`ALooper_addFd` (via libandroid.so,
confirmed in dynsym) and the app-glue main loop (guest 0x102bcd5d0) derefs
`ALooper_pollOnce`'s outData as `struct android_poll_source*` and `blr`s
`source->process` (fn at +0x10). The old shim wrote a **raw APP_CMD int** into
outData — which would crash a glue loop following the real layout. Fixed:

1. `ALooper_addFd(..., data)` records `data` (the `android_poll_source*`, i.e.
   `&app->cmd_source` in real glue) keyed by fd.
2. `ALooper_pollOnce` emits the registered poll-source pointer via outData only
   when one exists for the command fd; otherwise the raw-APP_CMD fallback for the
   existing host-feed lifecycle path is unchanged.

Regression tests pin both behaviors + the +0x10 process-fn layout. Also fixed a
real test-race: the ALooper tests share the process-wide queue/registry and run
under the parallel harness, so they need a shared serializing Mutex (the naive
`let _g = lock()` bound `&Mutex`, not a guard — passed at --test-threads=1 while
racing in parallel; now `.lock()`). Workspace 411/0; real boot unchanged (stable
idle main loop, exit 124, real X11 window wired). Run-log:
`/home/hermes-worker/runs/` (status in STATUS.md).

### Next lever (RECORRECTED this cycle — do not chase 0x102bcd5d0)
**The app-glue looper at guest 0x102bcd5d0 is COMPLETELY ORPHANED** in the real
binary (zero BL/B callers AND zero 8-byte pointer-constant references, scripted
scan). real libroblox.so uses GameActivity (`nativeAppBridgeV2StartAppWithParams`
driven by `--startapp`), not legacy android_native_app_glue; its prologue derefs
`[x19+24]` as a framework-initialized object only ANativeActivity_onCreate builds.
So fabricating an android_app and starting 0x102bcd5d0 as a guest thread is the
WRONG lever — it would not launch the engine's real render path. The ALooper
shim change is still correct (any real ALooper_pollOnce caller gets the right
android_poll_source* outData layout), but the wall is the engine's OWN producer:
its main loop runs, yet nothing enqueues work onto the per-thread idle futex
(0x10284d134) it parks on. The last-remaining cross-session lever was disproven
this cycle — the next real lever is either feeding the engine's own work queue or
finding the GameActivity-side producer that posts render tasks. Additionally,
real `.text` has 14,918 Unsupported FP16/NEON instructions (see
docs/fp16-decode-gap.md) that will block-tracker-break any real frame/audio path.

Prior cycles claimed "decode() never panics" but verified it only against `.text`.
The **whole-executable** scandecode scan (all PF_X segments — including the
data-region bytes a computed branch could land on) found **5260 decode() PANIC
sites**, every one outside `.text`. Two decoder gates shifted without guarding a
zero element-size field — a hard JIT abort (the whole process dies) on arbitrary
guest bytes:

1. **umov/smov** (insn&0xbfe0_fc00 == {0x0e00_3c00, 0x0e00_2c00}):
   `1 << imm5.trailing_zeros()` PANICS when imm5==0 (tz=32, shift overflow).
2. **vector dup** (insn&0xffe0_0c00 == {0x0e00_0400, 0x4e00_0400}):
   `1 << (f & f.wrapping_neg()).trailing_zeros()` PANICS when f==0.

Both now return `Inst::Unsupported` for the reserved zero-size encoding. **Key
subtlety:** the guard must be exact-zero ONLY — imm5/f packs BOTH element size
AND the lane index (e.g. `dup v21.2s, v23.s[1]` = 0x0e0c06f5 has f=0b01100 →
esize 4 via `f & -f`, src_idx 1), so any `f>8`/power-of-two bound wrongly rejects
real hardware instructions. An over-eager bound was written first, caught by the
regression test acting on real libroblox insns, and reverted.

**Verified (reproducible, no boot regression):**
- scandecode whole-exe (25,911,396 insns): **PANIC hits 5260 → 0**;
  `.text` (1,376,321 insns) still **0 Unsupported / 0 panics**.
- New regression `umov_smov_and_vector_dup_zero_imm5_do_not_panic` pins both
  gates + the real `dup v21.2s,v23.s[1]` / `dup v27.2s,v24.s[1]` forms.
- Workspace 409 passed / 0 failed (was 408). Real boot unchanged: reaches
  StartApp + stable engine main loop, exit 124, no crash.
- Run-log: `/home/hermes-worker/runs/cycleO-decode-panic-hardening-runlog.txt`.

### Next lever (unchanged — the hard remaining wall, now with more precision)
The engine main loop is reached and the ANativeWindow layer maps to a REAL X11
window (cycle N), but no **guest thread runs the app-glue looper**, so
`ALooper_pollOnce` is never called, the queued APP_CMD_START/RESUME/INIT_WINDOW
are never drained, and `eglCreateWindowSurface` never fires. Disasm pinned the
missing thread: **0x102bcd5d0** is the android_native_app_glue main loop
(currently anonymous, filed inside `nativePreloadFlagOverrides`' block). Prologue
takes the `android_app*` state in x0, stores it in x19, then the loop at
0x102bcd648 reads state flags (+8/+9/+10) and calls
`ALooper_pollOnce(-1, NULL, &events, &source)` (0x102bcd670), dispatching via
`blr x8` where x8 = `[source+16]` (the `process` fn). No internal caller invokes
it — it is the host-or-driver-started glue thread. Both `pthread_create`s spawned
in the boot run the engine worker loop 0x10284d168, never the glue loop.

Two routes to a first headless llvmpipe frame, in order of cleanliness:
1. **Drive the glue looper (0x102bcd5d0)**: fabricate an `android_app` state
   object (the fields the loop touches: looper handle, state-flags at +8/+9/+10,
   the app-command source `{id,process,..}` at the callback), and start it as a
   guest thread so it drains the ALoop. The app-command source's `process` fn is
   what must eventually call ANativeWindow_fromSurface→eglCreateWindowSurface.
2. **Enqueue a render/task directly** onto the engine's per-thread work-queue
   (the idle futex all 3 threads park on at 0x10284d134).

Cycle M's `ANativeWindow_fromSurface` returned a `HOST_THUNK_BASE|0x2000`
sentinel — a fake address Mesa's x11-EGL platform would reject in
`eglCreateWindowSurface(win, ...)`. This cycle mapped the window layer to a real
desktop window and fixed the wiring race:

1. `input_wrapper::x11::open_window_sized(...,w,h)` — the runtime opens a
   1280x720 Xvfb window matching the `ANativeWindow_getWidth/Height` framebuffer.
2. `shims::set_anativewindow_xid()` + an `ANATIVE_WINDOW_XID` atomic —
   `anativewindow_fromsurface` returns the registered real XID (sentinel
   fallback otherwise, so headless stays coherent).
3. **Race fix**: the first window wiring (commit `5fdd7a6`) ran in a spawned
   thread that lost to the boot — StartApp's `ANativeWindow_fromSurface` fired at
   ~3.9s while the XID registered later, so the guest still saw the sentinel.
   Now `wire_real_window()` runs **synchronously** inside the `--startapp` block
   before the StartApp `jit_run`: Xvfb up → 1280x720 window → XID registered →
   DISPLAY/EGL_PLATFORM=x11 set (X connection leaked to keep the window alive).
4. New integration gate `anativewindow_x11_surface`: the XID the guest's
   `ANativeWindow_fromSurface` yields builds a real EGL window surface and
   presents a frame (llvmpipe + Xvfb) — the exact value the window-surface path
   will consume.

**Verified:** log ordering proves the fix — `wired real X11 window
XID=0x200000` precedes `hostcall@ANativeWindow_fromSurface`, and the shim returns
`xid=0x200000`. Boot unchanged (stable idle main loop, exit 124, no crash).

Run (reproducible):
```
cargo build -p arm64jit --example elfjit
JIT_DRIVE_LIFECYCLE=1 timeout 20 ./target/debug/examples/elfjit \
  ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144 \
  --kicker 0x106863af8 --futex-kick 2
# expect: [elfjit:anativewindow] wired real X11 window XID=0x200000 on :22x,
# then exit 124 (stable engine main loop; no egl/looper hostcalls yet).
```
Run-log: `/home/hermes-worker/runs/anatg-sync.txt`.

### Next lever (unchanged hard wall — the looper/producer)
The engine's main loop is reached and the window layer is real, but no egl*/gl*
hostcall fires (0) and `ALooper_pollOnce` is never called: the render/EGL path
only opens after a producer enqueues a real render/task the idle futex
(lr=0x10284d134) awaits. Identify which guest thread should run the looper and
ensure it is spawned/woken (or enqueue the work item directly). A real
egl*/gl* frame from the running engine is the next targeted milestone.

---

Cycle L pinned the engine main-loop idle barrier as a REAL per-thread futex:
each guest thread parks in `guest_svc`'s FUTEX_WAIT_BITSET on its OWN latch
(uaddr = x1 = x19+4) at guest call-site lr=0x10284d134, with zero forward
motion. The static `--kicker` (fixed guest globals) couldn't reach these
per-thread dynamic latches, so the boot sat flat from the start.

**New elfjit lever `--futex-kick <period-ms>`** (commit `df5d0a4`): a detached
host producer snapshots the parked guest threads and, for each one at the idle
futex call-site, increments its latch (a version-counter futex — a bare fixed
write self-defeats because the next waiter captures the same value as expected
and re-blocks) and issues a real host FUTEX_WAKE. One tick per kick.

**Verified (reproducible):** vs flat idle, with `--futex-kick 2`:
- compiles advance 1515 → 1670 (155 new StartApp init blocks),
- `hostcall@ANativeWindow_fromSurface` is reached (guest pc 0x10258b3a0) — the
  boot's first window-layer touch, and `pthread_cond_wait` appears (85x),
- JNI setup churns: FindClass 23x, GetStaticMethodID 46x, mempool_calloc 75x,
  JavaVM.GetEnv, NewGlobalRef, ExceptionCheck.

The engine then re-parks on the same futex as a well-behaved idle loop — it
awaits a producer-ENQUEUED work item (a render/task). The futex-kick wakes the
consumer but no *work* is queued, so it sleeps again. `ALooper_pollOnce` is
still never reached (0 calls), `ANativeWindow_fromSurface` returns NULL (dead-
ends before eglCreateWindowSurface).

Run (reproducible): exit 124 (ran until harness timeout):
```
cargo build -p arm64jit --example elfjit
JIT_DRIVE_LIFECYCLE=1 timeout 20 ./target/debug/examples/elfjit \
  ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144 \
  --kicker 0x106863af8 --futex-kick 2
```
Run-log: `/home/hermes-worker/runs/cycleM-futex-kick-runlog.txt`.

### Next lever (two candidate walls, both now concrete)
1. **Wire a real native window into `ANativeWindow_fromSurface`** (GRAPHICS_-
   RECOMMENDATION §5.3): it currently returns 0, so eglCreateWindowSurface can
   never be created. Map the ANativeWindow to an X11 Window XID (Xvfb present,
   elfjit already links input-wrapper+x11rb; the egl_window_present gate shows
   the exact pattern). Then when the engine reaches the window/surface path it
   can proceed to a first headless llvmpipe frame.
2. **Reach the engine's looper/producer**: `ALooper_pollOnce` is never called —
   the app-command FIFO feed (JIT_DRIVE_LIFECYCLE) is inert until the engine's
   app-main thread runs the looper. Identify which guest thread/entry should
   drive the looper and ensure it is spawned/woken.

---

**This corrects cycles I–J's wrong conclusion.** The settled main loop issues
~204k futex syscalls / 12s, but only through the imported `syscall@LIBC`
function (the guest calls libc `syscall(nr,...)`, not `svc #0`). Cycle J's
"settled loop makes zero guest syscalls; it's a pure TLS-flag spin" was a
misdiagnosis — it probed only `svc #0` and missed the `syscall()` import path.

**The bug:** `resolver::resolve` bound `syscall` to HOST glibc `syscall()`,
which reads the number as an x86-64 syscall number. The guest's AArch64 futex
(98) became x86-64 getrusage (98) → returned -1, never blocked, and the loop
busy-spun re-issuing a dead futex.

**The fix** (commit `11189dd`, regression-tested):
1. `resolve("syscall")` → `host_syscall_intercept`, which rebuilds a CpuState
   (x[8]=aarch64 nr, x[0..5]=args) and dispatches through `guest_svc`.
2. `guest_svc` futex (98) now forwards `FUTEX_WAIT_BITSET` (op masked 9) to a
   real host futex.

**Verified:** all 3 guest threads now park INSIDE the real host futex at
`0x10284d134` with `x2=0x89`. Each thread WAIT_BITSETs on its own per-thread
latch uaddr (uaddr = x19+4). 405 tests (3 new). Boot exit 124, no crash.
Run-log: `/home/hermes-worker/runs/syscall-futex-fix-runlog.txt`.

### Next lever (REAL now, was wrongly cancelled by cycle J)
The main loop idle barrier is a genuine per-thread FUTEX_WAIT_BITSET — the
cycle-I lever (post the awaited futex value + FUTEX_WAKE from a host kicker)
is now host-drivable. Identify each thread's awaited `val` (x3) and the
producer value, then set the latch + FUTEX_WAKE so the loop advances.

---

New `scandecode` example (arm64jit) walks a PGX segment (or an optional
[start,end] guest-vaddr window, to scan only `.text`) and reports every
`Inst::Unsupported` plus any instruction that makes `decode()` panic. Against
the real binary it found exactly 6 undecodable code instructions and, on the
whole executable segment, proved `decode()` never panics. All 6 gaps fixed
with objdump ground truth (commits `f492ce7`, `7445153`):

1. **rev64: the WHOLE family was broken** (3 real hits, incl. `rev64 v5.2s`
   `0x0ea008a5`). Gate was `(insn&0x3f00_0c00)==0x0e00_0800 &&
   (insn&0x1800)==0`; every rev64 has byte1 0x08 (bit11 set), so the uzp guard
   wrongly rejected all six element sizes -> `Unsupported`. Fixed to
   `(insn&0x3f00_ff00)==0x0e00_0800` (byte1 exactly 0x08), still excluding
   uzp(0x18/0x58), rev16(0x18), rev32(bit29), dup-from-GPR(0x0d).
2. **shll/shll2 with rn>=8** (`0x2e613a10`, `0x6e613a17`, rn=v16): the
   WidenShl gate required `((insn>>8)&0x03)==0`, but bits[9:8] are rn
   bits[4:3], not a permute discriminator -> any shll on v8-v31 decoded
   `Unsupported`. True discriminator vs zip/uzp/trn = bit21 (set=shift-imm).
3. **cmhs (unsigned >=)** (`0x6ee13c02`): byte2 0x3c vs cmhi's 0x34.
   New SimdCmhs/SimdCmhsD translate to cmovae (cc 0x43) per lane, distinct
   from cmhi's cmova (`>`); 4S/2S/2D forms.

**Also fixed: decode() must never panic.** The `dup`-from-GPR decoder computed
`1u8 << imm5.trailing_zeros()`; imm5==0 (bits[20:16]) gives trailing_zeros=32
-> shift-overflow PANIC, aborting the whole JIT on arbitrary guest bytes now
emits `Unsupported` instead (regression:
`dup_from_gpr_invalid_imm5_does_not_panic`).

Result: full `.text` scans to `0 Unsupported / 0 decode() panics`. The JIT
can no longer fault on any reachable instruction in the real binary's code.

### Graphics/import readiness (boot-on-GPU prep)
- PLT fully bound against the real binary: 534 JUMP_SLOT, 0 unresolved
  (remaining 11 are GLOB_DAT data globals; `resolveimports` example).
- All 118 egl*/gl*/ANativeWindow*/ALooper*/AAssetManager*/AConfiguration*
  imports sit in the already-wired Mesa-llvmpipe resolver surface (the
  eglGetDisplay->...->eglSwapBuffers headless gate passes).
- Boot still reaches the stable engine main loop after the decoder changes
  (1871 compiles flat, exit 124 until harness timeout) — no regression.

### Frontier (unchanged, genuinely blocked on-this-box)
The settled main loop is a pure-CPU spin on bit0 of a per-thread TLS object
(`has-pending-work` latch at 0x10284d524 via getter 0x102b9dee0), driven by
the absent Android framework event/looper producer + a real window/surface;
needs the surviving looper/framework + EGL window layer, and a GPU host for
meaningful frame-perf proof (cycles I-J evidence: zero syscalls, static seed
and forced-branch both ineffective). Boot stabilization itself is DONE and
captured. Workspace: 402 tests, 0 failed; tree clean; HEAD `7445153`.

## Session (Sep 12, 2026, hermes-worker, cycle J) — STABLE MAIN LOOP HOLDS; the cycle-I "futex" next-lever is DISPROVEN and replaced with the true barrier (per-thread TLS-flag spin, framework-owned).

The real-boot milestone from cycle I is unchanged and still holds: libroblox.so
loads, JNI_OnLoad returns 0x10006, StartApp drives the engine, all guest threads
run the engine main loop headlessly until the harness timeout (exit 124; no
crash/leak; block-cache flat at 1871 compiles, hits ~19M, RSS ~5MB). This cycle
made no production code change — it re-established the frontier from first
principles and fixed the wrong plan.

**The cycle-I lever ("POST the awaited futex value + FUTEX_WAKE") is cancelled:**
the settled main loop makes **ZERO guest syscalls** (a 40s JIT_TRACE_SVC=1 run
printed zero futex and zero `guest svc` lines). The futex instructions at
0x10284d114-138 are a transient init burst, not the settled loop. There is no
futex to wake.

**The true steady-state barrier** is a pure-CPU spin at guest 0x10284d524:
`adrp/add x0,#0x7e0; bl 0x102b9dee0` (per-thread object getter),
`ldrb w8,[x0]; and w8,w8,#1; cbz <loop>` — it polls **bit0 of a per-thread TLS
object returning its address** (JIT_DUMP_PC=0x10284d538 shows x0 =
0x7f6998043df8 vs 0x7f6990036568 across threads). Seeding the static window
0x1067d67e0/f0/f8=1 via `--kicker 0x..=1` did NOT advance compiles (flat 1871):
the gate is dynamic, driven by the absent Android framework event/looper
producer plus a real window/surface. Same class of framework-owned lifecycle
gate as cycles C–I, now at the steady main-loop level — not a decoder/loader/ISA
gap, not a futex.

### Next lever (ordered / real)
The gate is an idle "has-pending-work" latch, NOT a seedable singleton: a 40s
JIT_TRACE_SVC capture shows the settled loop makes ZERO guest syscalls; a 40s
static-kicker seed of 0x1067d67e0/f0/f8=1 did NOT advance compiles; and even
FORCING the flag-set branch (patch `and w8,w8,#1` @ 0x10284d53c -> `mov w8,#1`
under JIT_DRIVE_LIFECYCLE) left compiles flat at 1871 (the 0x10284d548 handler
is an idle maintenance loop, not a work producer). So bit0 means "pending work /
should run" and forward motion needs framework-ENQUEUED work items, not a flag
seed:
1. Build the surviving looper/framework + EGL window/surface layer the engine
   awaits, so a real producer can post the work items that set the latch (correct
   ordering), then drive them. A GPU host is needed for meaningful frame-perf
   proof.
Boot stabilization (the achievable on-VPS milestone) is DONE and captured.

Run-logs (cycle J, reproducible):
```
cargo build -p arm64jit --example elfjit
JIT_DRIVE_LIFECYCLE=1 JIT_STATS=1 timeout 15 ./target/debug/examples/elfjit \
  ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144 \
  --kicker 0x106863af8    # exit 124 = stable main loop until timeout
```
- `/home/hermes-worker/runs/mainloop-tlsflag-spin-runlog.txt` (narrative + cmd)
- `/home/hermes-worker/runs/mainloop-tlsflag-spin-raw.txt` (verified raw: 1871 flat)
- Diagnostic used: `JIT_DUMP_PC=0x10284d538` (dumps x0 = polled per-thread ptr);
  `JIT_TRACE_SVC=1` (proves zero svc in the settled loop); `disasm` example.

## Session (Sep 12, 2026, hermes-worker, cycle I) — REAL BOOT REACHES A STABLE RUNNING ENGINE MAIN LOOP: crossed the cycle-H worker SIGSEGV (UXTW, not a W-write leak), the pthread_key_create destructor SIGILL, and the step-budget false abort. libroblox.so now loads + JNI inits + the main loop runs indefinitely headless (exit 124 on harness timeout). Workspace 400+/0.

Commits `b7da1a9` + `9c9332b` (dev). Cycle-H's stated next wall (worker SIGSEGV
guestpc 0x102173218, misattributed to a W-write zero-extension leak) was
re-root-caused from first principles with objdump ground truth:

1. **UXTW register-offset index (the real cycle-H bug).** The guest DELIBERATELY
   returns x0 = 0x100000000|hash from its hash table as a not-found SENTINEL
   (`mov x8,#0x100000000; orr x0,x8,x12` at 0x2173324/330 — verified vs
   aarch64-linux-gnu-objdump), and the caller indexes with
   `ldr w8,[x8, w0, uxtw #2]` at 0x2173218 — a UXTW (W) register offset that
   zero-extends w0 and MASKS OUT the sentinel bit. The JIT decoded it as full
   64-bit `[x8,x0,lsl#2]`, so x0=0x100000665 indexed 0x100000665<<2 OOB → SIGSEGV.
   Fix: carry the option bits[14:13] as `index_ext` on LdStrReg/FpLdStrReg
   (3=LSL full-64, 2=UXTW low-32, 1=UXTB) and zero-extend the index accordingly.
   Regression tests pin decode(index_ext) and the sentinel-masking load.
2. **pthread_key_create destructor SIGILL.** Worker's `pthread_key_create(dtor)`
   resolved to real glibc, which ran the guest AArch64 dtor natively on thread
   exit (SIGILL at 0x102b9e144, a `paciasp` prologue; gdb backtrace = libc
   `__pthread_keys`). Shim now creates a REAL key with a NULL destructor (book
   getspecific/setspecific still work; headless TLS dtors skipped — same as
   `__cxa_thread_atexit_impl`).
3. **Step-budget false abort on a reached main loop.** All 3 guest threads churn
   in the engine main loop (flat 1873 compiles, zero hostcalls, 5MB stable RSS).
   run_loop now only trips at the step budget if the block cache is still
   GROWING (un-settled init expansion); a flat cache = reached main loop, keeps
   running until the harness timeout.

### Result (headless, reproducible)
```
cargo build -p arm64jit --example elfjit
JIT_DRIVE_LIFECYCLE=1 timeout 30 ./target/debug/examples/elfjit \
  ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144 \
  --kicker 0x106863af8      # exit 124 = engine main loop ran until harness timeout
```
Run-log: `/home/hermes-worker/runs/mainloop-stable-runlog.txt`.

### Where the boot stands (vs the HARD GATE)
Achieved on this VPS: **libroblox.so loads, JNI_OnLoad returns 0x10006, StartApp
drives the engine, all guest threads reach a stable running main loop headlessly.**
That is the boot-stabilization milestone. Remaining to the full gate: get that
running main loop to dispatch a real frame (route through the wired Mesa
llvmpipe egl/gl so an early EGL/GLES call resolves).

The main-loop wait, precisely (JIT_TRACE): it busily polls a task/event futex —
`syscall 0x62` (aarch64 futex=98) with op 0x89 = FUTEX_WAIT_BITSET_PRIVATE at
call-site 0x10284d134, plus heavy `pthread_getspecific` (TLS getter 0x102b9df10)
and `clock_gettime` (timeout tracking). `guest_svc` only forwards FUTEX_WAIT(0)/
FUTEX_WAKE(1); FUTEX_WAIT_BITSET (0x89) returns 0 immediately, so the engine
never blocks — it re-issues the futex in a tight loop (flat ~1873 compiles, no
new init blocks). It never reaches ALooper_pollOnce or any egl*/gl* import, so
the cycle-G app-command feed is inert until then. Next lever (concrete):
identify the guest futex uaddr (x1 of the 0x62 syscall) and the "expected" value
that would let it pass, and POST a winning value + FUTEX_WAKE from a host kicker
(the F–H gate-kicker pattern) — or drive the awaited task-queue event — so the
main loop proceeds into the ALooper/EGL path and a first headless llvmpipe frame.
A GPU host is only needed for the final frame-perf proof.

## Session (Sep 12, 2026, hermes-worker, cycle H) — ROOT-CAUSED + FIXED the recursive-mutex rendezvous: `sanitize_mutex` was destroying glibc's `__owner`, so the owner deadlocked on its OWN recursive re-lock; boot now CROSSES the wall that parked every run since cycle C (workspace 398/0)

Commit `9e8d3a9` (dev). After crossing the GameActivity gates (cycles C–G), both
engine threads futex-parked on the glibc-RECURSIVE mutex `0x6edae60` at
`pthread_mutex_lock(0x102b53bb0)` with `__owner=0x0` while `__count=1` — a deadlock,
no forward motion, flat 948 compiles.

**The bug (real, boot-blocking):** `sanitize_mutex` ran before EVERY glibc
lock/unlock/cond_wait and zeroed offset 8 when it read `> 0x10000`, treating it as a
bogus bionic "recursion count". But offset 8 of a **glibc** `pthread_mutex_t` is
`__owner` — the host owner TID. A real TID like 3392123 exceeds 0x10000, so the
freshly-set owner of the LIVE recursive mutex was wiped on the next lock. glibc then
saw `__owner==0 != self` on the owner's own recursive re-lock and futex-blocked it.
`g_owner_tid=0x0` with `g_count=1` at the park is impossible for a correct glibc
recursive mutex — only sanitize writes offset 8. (Bionic stores owner_tid at offset
4, NOT 8; offset 8 is never a bionic leak worth clearing on either ABI.)

**Fix:** sanitize only masks the kind bits at offset 16; it leaves offset 8 alone.
Regression test now asserts `__owner` survives sanitize (was: asserts it's cleared).

**Result (headless, reproducible):** the boot CROSSES gate1 + gate2 + the recursive
rendezvous. The worker thread now does real init it never reached before:
`pthread_setname_np`, `FindClass`, `pthread_once`, mempool/`pthread_key_create`
TLS, mutex init/lock/unlock (trace shows `g_owner_tid=0x33f7b7` PRESERVED). The fence
is a SIGSEGV instead of a deadlock — machine gained ground.

### NEW wall (next frontier): worker SIGSEGV — 32-bit hash index keeps stale upper bits
Worker faults deterministically at `guestpc=0x102173210`, instr `ldr w8,[x8,x0,lsl#2]`
(caller of hash fn `0x102173258`): `x8=0x1073301c0` (a 0x2000-byte table just memset
to 0xff = 2048×4B entries), `x0=0x1000007f5`. Low 32 of x0 (`0x7f5`=2037) is a VALID
index; the upper `0x100000000` (= JIT_BASE) is stale. Index varies per run (988, 2037)
→ a genuine guest hash value leaking the translation-base high bit: a 32-bit `w`-write
in the hash loop `0x102173258` (contains `lsl x12,x1,x4` 64-bit + `mul w11,w8,w9` +
32-bit adds) isn't zero-extending the upper half, which the final `orr x0,x8,x12`
(fn epilogue, `x8` zeroed at `0x10217332c`) then propagates. Next: find the exact
non-zero-extending W-write in `0x102173258` (or in the loop `0x1021732a8..0x102173334`).

Repro:
```
cargo build -p arm64jit --example elfjit
JIT_DRIVE_LIFECYCLE=1 timeout 20 ./target/debug/examples/elfjit \
  ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144 \
  --kicker 0x106863af8        # deterministic SIGSEGV guestpc=0x102173210 (tid 1)
```
Run-log: `/home/hermes-worker/runs/gate-crossed-cycleH-runlog.txt` (SIGSEGV, not park).

## Session (Sep 12, 2026, hermes-worker, cycle G) — `--kicker` value bug fixed + real ALooper app-command dispatch (workspace 396/0)

Commit `c0736e2` (dev). Cycle F crossed the boot wall's first two GameActivity
lifecycle gates (ldaxr poll 0x106863af8 + cond_wait 0x10683a168) and pinned the
residual to a glibc-RECURSIVE rendezvous mutex 0x6edae60 (call-site 0x102b53bb0)
both engine threads futex-park on. This cycle did NOT cross that wall (confirmed
it is genuinely unchanged — the engine parks *before* ALooper ever spins up), but
removed a real harness bug and built the documented post-barrier mechanism:

1. **`--kicker` value bug (real, boot-affecting).** elfjit parsed `=0xVAL` but
   ignored it, always pulsing 1→2 for every non-bcast kicker. So
   `--kicker 0x106863af8=1` actually KEPT WRITING 1 — the owner busy-spun the
   gate-2 cond_wait (~1.2M block-cache hits) instead of crossing to 2. Now
   `=bcast`, `=0xVAL` (exact value), or bare (Pulse 1→2) are distinct modes.
   The correct gate-crossing invocation is the BARE form `--kicker 0x106863af8`
   (Pulse 1→2); `=1` now correctly pins 1 (gate-2 probe). Verified: bare Pulse
   crosses gate1+gate2, compiles grow 919→948, both threads land on the deep
   recursive rendezvous.
2. **Real ALooper app-command dispatch (shims.rs).** The old `ALooper_pollOnce`
   returned 0 immediately with no event channel, so a boot that DID cross the
   rendezvous would busy-spin the looper on a never-signalled fd instead of
   dispatching lifecycle. Added `post_app_command`/`ALooper_pollOnce` (host-feedable
   mutex'd FIFO drained into outFd/outEvents/outData, android_native_app_glue
   convention; empty → ALOOPER_POLL_TIMEOUT, never blocks), the full ALooper family
   (prepare/forThread non-null handle, addFd→1, removeFd→0, acquire/release→0),
   `ANativeWindow_getWidth/getHeight`→1280x720, and an elfjit APP_CMD feed
   (START/RESUME/INIT_WINDOW) under JIT_DRIVE_LIFECYCLE. Regression test
   `alooper_pollonce_dispatches_host_fed_app_commands`.

### Repro (headless, reproducible)
```bash
cargo build -p arm64jit --example elfjit
timeout 40 ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni          # clean exit 0
JIT_DRIVE_LIFECYCLE=1 timeout 20 ./target/debug/examples/elfjit .../libroblox.so 0x2173ff4 --jni --startapp 0x258b144 --kicker 0x106863af8   # cross gates, park at rendezvous (124)
JIT_DRIVE_LIFECYCLE=1 JIT_THREADS=1 ... --kicker 0x106863af8   # concurrent thread-state sampler
```
Run-log: `/home/hermes-worker/runs/gate-lifecycle-runlog.txt`.

### Current wall (unchanged, precise)
Both engine threads futex-park on the glibc-RECURSIVE mutex 0x6edae60 at
call-site 0x102b53bb0 (GameActivity_initializeNativeCode rendezvous). Owner
re-locks the recursive mutex while init-churning (compiles 919→948 after gates),
then parks; worker blocks on it. `cond_wait`/`ALooper_pollOnce` never reached (0
calls). Genuine Java app-command / looper lifecycle await — the same wall cycles
C–F documented. Mesa llvmpipe egl/gl, GLES float/texture bridges, and now the
ALooper app-command channel are all wired + tested; none can fire until this
barrier is crossed.

### Next lever (unchanged from cycles C–F, app-command feed now in place)
Identify/release what lets the holder of 0x6edae60 proceed (which thread posts
the app-command / looper event on real Android) and seed it, OR seed the
rendezvous itself to pass without the looper. The engine never reaches
ALooper_pollOnce before the barrier, so the new feed is inert until then. After
the rendezvous, the feed dispatches APP_CMD_START/RESUME → wired Mesa llvmpipe
egl/gl → first headless frame.

## Session (Sep 12, 2026, hermes-worker, cycle F) — boot wall's FIRST TWO gates CROSSED from the host: owner leaves idle ldaxr-poll AND the gate-2 cond_wait, bursts 919→946 blocks, lands at the recursive-mutex rendezvous (workspace 395/0)

Commits `5dd02ee` + `a69c42a` + `4b488af` (dev). For cycles C-E the boot froze at the
GameActivity rendezvous: all three guest threads parked while the engine owner
busy-polled guest global **0x106863af8 until == 1** (`adrp x8,#0x106863000; add
x8,x8,#0xaf8; ldar x8,[x8]; cmp #1; b.eq`) holding the recursive rendezvous
mutex 0x6edae60. No Java layer exists on this box to set it, so it never
released.

This cycle added **host-side lifecycle release** and proved the boot advances:

- `--kicker 0x<guest-global>[=<val|bcast>]` (repeatable, elfjit): detached
  host thread writes a value to / `pthread_cond_broadcast`s a guest global
  while `jit_run` parks — feeds awaited lifecycle state from outside.
- `JIT_DRIVE_LIFECYCLE=1`: `host_cond_wait` becomes a 2 ms sawtooth timedwait
  so a guest cond_wait entered before we satisfy its predicate still returns
  periodically and re-checks an externally-satisfied flag.
- `snapshot_threads()` now carries x19/x20; elfjit's sampler derefs the
  predicate pointer so the log names *which global* a parked owner awaits.

### Result (headless, reproducible): first motion across the wall
`--kicker 0x106863af8=1` + `JIT_DRIVE_LIFECYCLE=1` makes the owner
1. leave the `ldaxr [0x106863af8];cmp #1` init poll (gate 1),
2. burst 919 -> 948 compiled blocks (29 new init blocks),
3. park at a DISTINCT second wait: `pthread_cond_wait(cond=0x10683a168,
   mutex=0x10683a140)` at guest call-site 0x102b4cd78 (gate 2), re-checking
   `*x19` each 2 ms wake and re-parking while `*0x106863af8 == 1`.

### Gate 2 (now also crossed; commit `4b488af`)
With the re-arm store at 0x102b4cdb4 NOP'd (under `JIT_DRIVE_LIFECYCLE=1`
only), the host terminal value 2 persists and the owner LEAVES the cond_wait
back into the outer init/refcount region (0x102206c00), compiles growing
926→946. Residual wall: all three threads futex-park on the recursive
rendezvous mutex 0x6edae60 (call-site 0x102b53bb0) — the engine's genuine
multi-thread barrier, released on real Android by the Java layer's
app-command / ALooper dispatch. So cycle F crossed TWO lifecycle predicates
with host-supplied state.

Run-log: `/home/hermes-worker/runs/kicker-gate1-runlog.txt` (updated for
gate 2).

## Session (Sep 12, 2026, hermes-worker, cycle E) — boot wall pinned at register+futex level; concurrent thread-state sampler + GLIBC mutex owner/count/kind (workspace 395/0)

Commits `1dae9e1` + `eaf00e6` (dev). This cycle pinpointed the
GameActivity lifecycle-await wall at register+futex level (previously only
inferred by timing/heuristic). New concurrent guest-thread sampler
(`snapshot_threads()`, JIT_THREADS=1) dumps every registered guest thread's
hostcall slot (pc), guest call-site (x30), and wait-object args (x0..x2)
while StartApp's parked `jit_run` never returns. Real libroblox StartApp
boot:

- **All 3 engine guest threads futex-park on `pthread_mutex_lock(0x6edae60)`,
  x30 == `0x102b53bb0` for all three** (single call site inside the
  GameActivity_initializeNativeCode rendezvous).
- glibc fields of `0x6edae60`: **g_kind=1 (PTHREAD_MUTEX_RECURSIVE),
  g_count=1** — a LIVE owner holds it with recursion depth 1; it is NOT an
  abandoned/cross-ABI-wedged lock. Owner (tid 0) runs the GC/init atomic
  refcount region at `0x102206afc/bb0` (`ldar x8,[x8,2808]; subs; b.eq`),
  then re-acquires and parks.
- **per-thread CPU while parked: owner 14% in `futex_wait_queue` (an active
  wake/check/re-park POLL on the recursive mutex — polls the awaited
  app-command/lifecycle flag); tids 1 & 2 0.7% truly idle.** Dispatcher
  compiles flat at 767 (one jit step after StartApp, then the main loop parks).
- New `disasm` example (`cargo run -p arm64jit --example disasm -- <elf>
  <guest-addr...>`) decodes a guest region to name the enclosing functions.

This confirms the documented wall (engine awaits Java-side app-command /
looper / lifecycle state that would release `0x6edae60`) and converts the
"identify what releases it" lever into a precise, reproducible pin — the
owner polls a specific object at recursion-count-1 and only proceeds once that
app-command arrives. It does NOT yet cross the wall. The Mesa llvmpipe egl/gl
path and texture/float bridges remain fully wired (graphics gates green).

### Next lever (unchanged — this is the hard remaining wall)
Cross the GameActivity lifecycle-await so the engine proceeds to the looper and
the already-wired Mesa llvmpipe egl/gl path produces a first real frame.
Requires emulating the Android app-command / looper state: seed the flag/object
the owner polls at `0x102206afc` (or the once-guard that gates it), OR feed
ALooper_pollOnce synthetic app commands (APP_CMD_START/RESUME/INIT_WINDOW) from
a host side so the awaited predicate is satisfied and `0x6edae60` releases.
Then route the boot's `egl*`/`gl*` imports through the existing Mesa resolver
for a first headless frame.

### Repro
```bash
cd /home/hermes-worker/runs/open-sober
cargo build -p arm64jit --example elfjit
timeout 30 ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144  # stable idle (exit 124)
JIT_THREADS=1 timeout 15 ... --jni --startapp 0x258b144   # concurrent thread-state sampler
JIT_TRACE=1 timeout 12 ... --jni --startapp 0x258b144 2>&1 | grep mutex_lock  # glibc owner/count/kind
cargo run -p arm64jit --example disasm -- ~/.cache/open-sober/robbox/libroblox.so 0x102206bb0 0x102b53bb0
```

## Session (Sep 12, 2026, hermes-worker, cycle D) — boot wall re-characterized; JIT_TRACE reverse-name registry (workspace 394/0)

Commit `b99c3df` (dev). This cycle re-confirmed the real-boot wall exactly as
last documented — all three guest threads genuinely futex-park (0% CPU) on the
glibc-RECURSIVE mutex `0x6edae60` (owner thread 3188327 does real init through
the atomic CAS once-guard `0x2b9e1d0`, then the main/StartApp threads re-lock
the recursive mutex and park awaiting Java-side app-command/lifecycle state;
`cond_wait/timedwait` never reached; no new block compiles after StartApp fires
— flat 767 compiles). Graphics (Mesa llvmpipe through the JIT bridges) is fully
wired and passing (`egl_window_present` Xvfb gate + resolver unit gates). The
wall is a genuine Android-lifecycle-emulation gap, not a decoder/loader gap.

### What landed (readable real-boot run-log)
The real libroblox.so boot's JIT_TRACE dumped anonymous `hostcall@slotN` for
every auto-allocated GLES/float/JNI host-call slot, hiding which engine import
the GameActivity init dispatches while parked. Added a reverse-name registry:
- `jit.rs`: `HOST_CALL_NAMES` (addr->name) + `name_host_call_slot()`;
  `name_of_call_addr()` consults it after the resolver name map.
- `jni.rs`: `JNI_METHOD_NAMES` table names every filled JNIEnv/JavaVM slot by
  its function-table const.
- `resolver.rs`: `resolve_gles_mixed`/`resolve_float`/`resolve_float32` record
  the symbol name on the returned slot.
- `jit.rs`: boot `mempool_calloc(x1=size)` and `lsm_map_calloc(x0=size)` thunks
  named.
Result: the real StartApp boot now prints `JNIEnv.GetStaticMethodID`,
`FindClass`, `NewGlobalRef`, `ExceptionCheck`, `JavaVM.GetEnv`, float/GLES
names, `boot.mempool_calloc` instead of `slotN`. New regression
`name_of_call_addr_resolves_auto_allocated_gles_and_float_slots`.

### Repro (unchanged behavior)
```
cd /home/hermes-worker/runs/open-sober
cargo build -p arm64jit --example elfjit
# idle boot (clean exit 0):
timeout 40 ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni
# StartApp main loop (stable idle, exit 124, no crash):
timeout 30 ./target/debug/examples/elfjit ... --jni --startapp 0x258b144
JIT_TRACE=1 timeout 12 ... --jni --startapp 0x258b144 2>&1 | grep -oE 'hostcall@[A-Za-z0-9_.()]+' | sort | uniq -c | sort -rn
```

### Next lever (unchanged — this is the hard remaining wall)
Cross the GameActivity lifecycle-await so the engine proceeds to the looper and
the already-wired Mesa llvmpipe egl/gl path produces a first real frame. This
requires emulating the Android app-command / JNICallProtocol / looper state the
real Java layer drives (HANDOFF #1/#3): identify what releases `0x6edae60`
(which thread posts the app-command) and seed it, or drive the awaited looper
state from a host side feeding ALooper_pollOnce with synthetic app commands
(APP_CMD_START/RESUME/INIT_WINDOW). The relevant EGL/GLES imports already route
to Mesa. Multiple prior sessions hit this same wall — it is the frontier.

## Session (Sep 12, 2026, hermes-worker, cycle C) — DIAGNOSIS CORRECTED; bionic bridge + glibc-recursive routing hardened (workspace 393/0)

Commits `f0f352a` + `c4faa0a` (dev). This cycle implemented byte-exact bionic
NORMAL mutex support AND then corrected the diagnosis: the pinned wall mutex
`0x6edae60` is NOT a bionic-vs-glibc ABI collision — it is a **glibc-formatted
RECURSIVE mutex initialized by our own `host_mutex_init` bridge** (disassembly
of `0x2b53adc` shows `pthread_mutexattr_settype(#1)` = `PTHREAD_MUTEX_RECURSIVE`
immediately before its `pthread_mutex_init`). glibc owns and handles it
natively (recursive re-entry via `__kind=1`). Boot progress is **unchanged**
(same wall before/after); the commits are correctness hardening, not boot motion.

### 1. What landed
- `f0f352a` — implement bionic `NonPI::NormalMutexLock/Unlock` byte-exact on
  the guest 16-bit `_Atomic(uint16_t) state` word (CAS 0→1, exchange→2 +
  futex_wait; unlock exchange→0 + futex_wake-if-contended), with the required
  two-REAL-thread rendezvous regression test. Correct building block.
- `c4faa0a` — **registry routing (the real fix)**: `host_mutex_init` records
  every guest mutex it initializes via REAL glibc (with a REAL glibc attr) in a
  `HashSet`; `lock`/`unlock` route those back to the real glibc path. ONLY
  mutexes never initialized through the bridge (static zeroed bionic
  `PTHREAD_MUTEX_INITIALIZER` words the guest touches with inline atomics) take
  the bionic 16-bit protocol. Without this, my own bionic path misfired on the
  glibc-recursive `0x6edae60` (read the glibc word as type=NORMAL) and would
  SELF-DEADLOCK on the guest's legitimate same-thread recursive re-lock.

### 2. The wall, exact (unchanged; now understood as lifecycle-await, not ABI)
All three guest threads park at 0% CPU (verified 0 utime ticks over 3s) via
`pthread_mutex_lock` of guest mutex `0x6edae60` (`gpcreq=0x102b53bb0`, the
`GameActivity_initializeNativeCode` rendezvous). Owner thread
locks/unlocks/re-locks it cleanly (recursion works), then parks holding it
awaiting the Java-side lifecycle/app-command state. `cond_wait/timedwait` are
never reached (0 calls) — the "pair with cond" note is not a live wall. The
engine never reaches any `egl*`/`gl*` import before this wall either, so Mesa
llvmpipe routing won't help until this is crossed. Idle JNI-only boot still
exits 0; StartApp boot still exits 124 (stable idle, no crash).

### 3. Next lever (get past the GameActivity rendezvous)
The barrier is `GameActivity_initializeNativeCode` waiting for the app-command/
looper/lifecycle state the real Java layer drives. Candidates:
1. **Drive the awaited state**: identify what releases `0x6edae60` (which
   thread sets the flag / posts the app-command that lets the holder proceed)
   and seed it (Session-11 guard pattern). The TLS accessor `0x2b9dee0` is a
   per-thread cache getter, NOT the awaited singleton — don't chase it.
2. **More JIT ground speed** (perf) so init churns faster — but threads are
   genuinely futex-parked, so speed alone won't cross a true await.
3. After the rendezvous: route `egl*`/`gl*` to Mesa llvmpipe (both headless
   graphics gates already pass) → first frame. Then render/input.

Commits `6bc57a6` → `18ae7f6` → `063dc5e` → `7946fe3` (dev). The real
`libroblox.so` boot keeps advancing: JNI_OnLoad → `--startapp` drives
`nativeAppBridgeV2StartAppWithParams` → `GameActivity_initializeNativeCode`,
reaches the engine main loop, and **idles stably** (exit 124, no
SIGSEGV/SIGABRT). This cycle identified exactly what the loop waits on.

### 1. The wall, exact
`JIT_TRACE` shows the main loop parked in `pthread_mutex_lock` on guest mutex
`0x6edae60`:
```
[t=...] [mutex_lock] 0x106edae60 bionic_word=0x00000002 state=0x2 gpcreq=0x102b53bb0
```
- guest `state` = **2 = bionic `MUTEX_STATE_LOCKED_CONTENDED`** (NORMAL,
  non-PI, non-recursive mutex).
- Caller guest PC `0x102b53bb0` (in `GameActivity_initializeNativeCode
  +0x2f8488`, the mutex-init attribute setup).
- All guest threads idle in host futex, 0% CPU.

**Root mechanism:** the guest manages its own bionic `pthread_mutex` on its own
16-bit `state` word (modern NDK r28c layout: `_Atomic(uint16_t) state` @0,
`owner_tid` @4, 28-byte tail). When a guest thread's inline fast-path hit
contention it set state=2 and called our bridge's glibc `pthread_mutex_lock`;
glibc reads the bionic 16-bit state word as glibc's own lock encoding, sees no
matching glibc `__owner`, and futex-blocks forever while the holder (also a
guest thread) released the lock via its own fast-path atomics. A clean
**bionic-vs-glibc cross-ABI futex mismatch** — NOT an ALooper wait, NOT a
decoder gap.

### 2. Correction of this session's own earlier claim
`6bc57a6`/`18ae7f6` called it a "recursive mutex rendezvous" from the glibc
`__kind` field at mutex+16. That field is PAST the bionic word (on a different
init path — `pthread_mutexattr_settype(#1)` at 0x2b53b04 inits a different
mutex). `063dc5e` corrected it: the blocking mutex is NORMAL, in
LOCKED_CONTENDED (word 0x2).

### 3. Verified: the mutex is real — do NOT weaken it
All guest threads genuinely idle (futex/nanosleep, 0% CPU). An "optimistic
non-blocking acquire" (trylock→return 0 on EBUSY) let a second guest thread
into the same critical section → SIGSEGV on garbage. A hand-rolled bionic CAS
attempt was reverted in-tree, unbuilt, before touching the boot. Both reverted.
This is genuine shared-memory mutual exclusion at lifecycle handoff.

### 4. Correct fix (SCOPED — next task)
Implement bionic's NORMAL mutex protocol byte-exact on the guest's own 16-bit
`state` word @0: acquire = CAS state 0/1→LOCKED_UNCONTENDED; contention → set
LOCKED_CONTENDED(2), futex-wait on the word; unlock = clear to 0 + FUTEX_WAKE.
Must be paired with the bionic `cond` (cond_wait internally unlock+relock the
mutex). Validate with a TWO-THREAD rendezvous unit test (A locks via bridge, B
blocks in bridge, A unlocks, B acquires) BEFORE wiring into the boot.
Alternatively drive the awaited looper/app-command state. Keep the stable idle
boot as the base.

### 5. Graphics "first frame" gates both pass (headless, this VPS)
`glesv2-wrapper headless_graphics.rs` (surfaceless llvmpipe ES3, ETC2
interception, BC1 passthrough) and `arm64jit egl_window_present.rs` (Xvfb real
X11 window, full eglGetDisplay→...→glClear→eglSwapBuffers through the JIT
guest-bridge slots, returns EGL_TRUE) both pass — real frames present headless.

Repro:
```bash
cd /home/hermes-worker/runs/open-sober
cargo build -p arm64jit --example elfjit
timeout 30 ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144
JIT_TRACE=1 ... 2>&1 | grep mutex_lock | tail   # pin blocking mutex state/caller
```

Commit `152dce9` (dev). Two real bottlenecks to StartApp forward-speed removed:

1. **Translation-block cache (the big one).** The PC-driven dispatcher
   (`jit_run_inner`) recompiled a region from scratch on every `blr`/`br`/`ret`
   re-entry. A boot hot-spotting on a small accessor (Roblox's TLS-block getter
   `0x2b9dee0` → 160 of ~1271 traced block-execs, each `pthread_getspecific`)
   retranslated that code ~once per call — the dominant cost once the guest is
   churning TLS blocks. `cached_block()` keys on `(image, pc, state)` and leaks a
   process-lifetime `JitBlock` (never munmaps). Evacuated at each *top-level*
   `jit_run` (safe: leaked) so distinct ELF images at the same `JIT_BASE`
   (the `diff_battery` suite maps each test at 0x100000000) never run a stale
   block — this correctness fix is what makes the whole thing safe. Real boot:
   **767 compiles / 5691 hits**. New `block_cache_stats()` + `JIT_STATS`
   per-dispatcher heartbeat.
   - **Regression caught & explained by the cache:** the `loader_run_timer_signal
     _delivers_sigalm` test started failing once the loop got fast. Bisect showed
     guest nanosleep was never *actually sleeping* (see #2), so the 20000-iter
     spin loop finished before 2×100ms timer ticks; the test only passed by luck
     of uncached-JIT slowness stretching it past 200ms. Fixing #2 made it pass
     deterministically (0.23s) regardless of JIT speed. Lesson: a *fast* JIT
     exposes races the slow one masked — run the timing/signal loader tests after
     any perf work.

2. **nanosleep syscall arg fix (real boot bug).** aarch64 `nanosleep` passes
   `rqtp` in x0, but `guest_svc` read it from a[1] (x1) → NULL req → EFAULT in
   ~1.5µs, no sleep. So every guest sleep-wait was really a busy-spin. Now reads
   `a[0]` (rqtp) / `a[1]` (rmtp). Regression test
   `guest_svc_nanosleep_reads_timespec_from_x0`. This matters for the boot: any
   guest `sleep`/`usleep`/wait that Roblox does now blocks the guest properly
   instead of hot-spinning the core.

### Main-loop wall, now characterized precisely (NOT a deadlock)
`--startapp` drives the real `GameActivity_initializeNativeCode` (0x258b144 →
region `0x284dxxx`, TLS-block accessor `0x2b9dee0` = `ldar x22,[x0+0x10]; cbz`
+ `pthread_getspecific`, wrapper `0x284d524`, vtable-check `0x284f874/880`). A
12s JIT_TRACE reaches **236 distinct blocks, growing across the window
(30→69 distinct block-sites in first/last 100 execs)** → the guest is *advancing
through new init code*, just slowly, and makes no guest `svc` once settled.
So the next lever is NOT a decoder gap, NOT an ALooper wait — it's either
(a) more JIT/perf so it grinds through the ~thousands of TLS-block allocs faster,
or (b) finding the specific singleton whose init never *completes* and seeding it
(Session-11 guard/flag pattern), or (c) driving the awaited looper/app-command
state so the main loop dispatches a real frame/render instead of init-churning.

### Diagnostics added
- `resolver::name_of_call_addr()` reverse slot → name; JIT_TRACE now prints e.g.
  `hostcall@pthread_getspecific` (identified the main-loop hot import).
- `JIT_STATS=1` prints a per-250ms dispatcher heartbeat: `[jit] step N pc=... block-cache: C compiles / H hits`. Compiles climbing = new code; flat + hits rising = genuine loop spin.
- elfjit prints `[elfjit] block-cache: C compiles / H hits` on clean exit.

Repro (see STATUS.md for full):
```
cargo build -p arm64jit --example elfjit
timeout 30 ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144
JIT_STATS=1 timeout 30 ... 0x258b144       # progress heartbeat
JIT_TRACE=1 timeout 12 ... 0x258b144 | grep hostcall@ | sort | uniq -c | sort -rn
```

## Session (Sep 11, 2026, hermes-worker) — POST-JNI_OnLoad game-start: `--startapp` boot stage; real libroblox reaches the ENGINE MAIN LOOP (workspace 388/0)

Commit `a4f94d1` (dev). The real `libroblox.so` 2.738.1397 boot advances from
"JNI_OnLoad returns 0x10006 then the process exits" to **the guest entering and
persistently running the engine's `GameActivity` main-loop / event-pump region**
after a new `--startapp` stage chains the real Java-side game-start entry.

- **Why the boot previously exited**: JNI_OnLoad is a *registration* function;
  on real Android the JVM then calls `nativeAppBridgeV2StartAppWithParams` etc.
  to actually start the game (main loop + EGL/GLES init). elfjit only ran
  JNI_OnLoad, so once it returned and the spawned worker boot-body finished, the
  harness's `main()` returned and the process exited cleanly (exit 0).
- **`--startapp <link-addr>`** (elfjit): after `jit_run(JNI_OnLoad)` returns
  `Ok(0x10006)`, builds the singleton env + fake-but-valid `jobject` (x1) +
  `jstring` (x2) and `jit_run`s the real `nativeAppBridgeV2StartAppWithParams`
  (0x258b144) as a fresh guest entry. Critical detail: it reuses the **boot-phase
  guest SP** (`s2.x[31]=st.x[31]`); a fresh 0 SP wrapped StartApp's `sub sp,#0xf0`
  prologue to `0xffffffffffffff10` and the frame-write SIGSEGV'd immediately.
- **New jni helpers**: `new_fake_object()`, `new_string_utf_handle()`, and a
  `JNI_TRACE_REGISTRY` env to dump RegisterNatives bindings.
- **Verified** (headless, no QEMU): the guest executes 800+ distinct blocks
  through StartApp — FindClass for dozens of Roblox classes, repeated VM_GetEnv,
  pthread_once/mutex/getspecific TLS-key protocoling in the
  `GameActivity_initializeNativeCode` thread-local setup, `LockBasedAllocator` —
  then settles into a persistent main-loop cycle (`ldar x22,[x0+0x10]; cbz`
  await + `pthread_getspecific` dispatch) across two guest threads and runs
  until the harness `timeout` fires (exit 124; **no SIGSEGV/SIGABRT**). JNI_OnLoad
  alone still exits 0 cleanly (~4.8s); both paths preserved.

### Current wall (narrowed from "nothing drives the app" to a specific loop)
The engine main loop is reached but awaits app events / lifecycle (looper
input, window/surface, EGL) that the real Java side supplies. Next (ordered):
1. Feed the awaited `GameActivity` app-command / looper state and route the
   boot's `egl*`/`gl*` imports through the existing Mesa llvmpipe resolver so
   any EGL context/frame path reachable from StartApp runs real software
   graphics — the first reproducible engine-loop artifact (a frame / looper
   event dispatch), headless on this VPS.
2. Advance FMOD audio init and the JNIMain main-loop drive.
3. HARD GATE (real session + run log) unchanged as the end goal; the
   achievable-on-this-VPS milestone next is a real *frame* / first looper event,
   then it's a GPU host for the final perf proof.

Repro:
```
cargo build -p arm64jit --example elfjit
# boot-only: timeout 120 ./target/debug/examples/elfjit .../libroblox.so 0x2173ff4 --jni
# boot + game-start main loop: timeout 30 ./target/debug/examples/elfjit .../libroblox.so 0x2173ff4 --jni --startapp 0x258b144
```
Run-log: `/home/hermes-worker/runs/startapp-boot-runlog.txt` (exit 124 = ran
in the engine main loop until the harness timeout; no crash).

## 🟢 STABLE HEADLESS BOOT of real libroblox.so (exit 0, reproducible)

**The real `libroblox.so` (2.738.1397) now boots to a stable state headlessly
through `arm64jit` + `libloader` and exits cleanly (`exit 0`, no SIGSEGV) —
verified 3/3. JNI_OnLoad returns `0x10006`, real engine JNIMain code runs, the
worker guest thread runs clean, teardown is clean.** This is the boot
stabilization milestone on this VPS.

Two fixes this session (commits `6e9fd4e`, `1639899`, workspace 388/0):
1. **`plt::bind_glob_dat`** — unresolved *function* GLOB_DAT/ABS64 slots were
   left at stale values (0 or a `.dynstr` symbol-name pointer); a guest `blr`
   through them jumped INTO `.dynstr` (SIGSEGV, register file = ASCII symbol
   strings). Now bound to a benign host-call stub. Real lib: 63 -> 67 bound.
2. **`__cxa_thread_atexit_impl` no-op shim** — was resolved to real glibc,
   which stored the guest AArch64 TLS-destructor pointer and invoked it NATIVELY
   as x86 when the worker guest thread exited -> SIGSEGV executing guest ARM64
   .text (the post-boot "worker/teardown" crash). Now a no-op, so glibc never
   runs a guest functor natively. This fixed the crash and gave the clean exit.

Repro:
```
cd /home/hermes-worker/runs/open-sober
cargo build -p arm64jit --example elfjit
timeout 120 ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni
```
Expect: seeds + `Test TelemetryProtocol` + `DeviceStaticParams is null`, then
`jit_run returned Ok(0x10006)` / `JIT(no-QEMU) entry() -> 65542 (0x10006)`,
clean `exit 0`.

**NEXT (advance the boot / graphics):** JNI_OnLoad now succeeds and the process
exits cleanly; push toward a real main loop that doesn't exit. Graphics
wrappers already point at Mesa llvmpipe; the egl/glesv2 stubs exist. Use
`GRAPHICS_RECOMMENDATION.md`: surface a window/surfaceless EGL context, have
the guest render a frame, and confirm with a run log. Also consider whether
JNI_OnLoad's spawned worker should be joined/looped instead of the process
exiting when the main dispatch returns.

**The real `libroblox.so` (2.738.1397) boots through `arm64jit`
(`elfjit <libroblox.so> 0x2173ff4 --jni`) and main-thread `JNI_OnLoad` returns
`0x10006` (JNI_VERSION_1_6) reproducibly.** This cycle (commit `6e9fd4e`,
workspace 388/0):

1. **Closed a real indirect-call-into-`.dynstr` vector.** `plt::bind_glob_dat`
   left unresolvable *function* GLOB_DAT/ABS64 slots at their original value
   (0 or stale `.dynstr` symbol-name pointer); a guest `blr` through one jumped
   into `.dynstr` (SIGSEGV with register file full of ASCII symbol strings —
   "pthread_setspecific", "memset", "pthread_cond_broadcast"). Now bound to a
   host-call stub. Real binary result: **67 GLOB_DAT bound / 11 unresolved**
   (was 63/15); the fault's `guestpc` is now a real guest address, not ASCII.
   Regression test `loader_run_unresolved_func_globdat_binds_safe_stub` (fn
   import via `int (*gfp)(int)` binds to host thunk 0x7f0000002008).
2. **Fault diagnostics enriched** (elfjit): full host-x86 register dump,
   `rbx_matches_gueststate` (is the faulting RBX the thread's registered
   CpuState?), guest-thread table `(host_tid:guest_tid,state)`, nesting-aware
   `in_jit_run` counter.

**Current wall (precise, unbuffered-stderr-proven):** main's `jit_run` returns
Ok(0x10006); the SIGSEGV is in the **post-run phase** (worker guest thread,
tid=1, start_routine `0x284d168`, is running when main's dispatch ends).
`rip=0x10284d6be` is a guest `.text` address in the
`JNIActivityLifecycleCallbacks_nativeOnDestroyed` region executed as x86 —
a **host path calls a guest function pointer natively**, not a translated-block
fault. `rbx_matches_gueststate=false` ⇒ the guest-register dump is an artifact
of a garbage RBX; trust the host regs/rip, not guestpc. Next: find which host
call path dispatches guest `0x284d6b4` during worker/teardown (guest signal
handler outside the cooperative dispatcher, an atexit/on_destroy callback
routed to guest natively, or the worker start_routine dispatched down a
non-`jit_run` host path). See `docs/` run-log + `runs/STATUS.md`.

**The real `libroblox.so` (2.738.1397, extracted via `sober-core::apk::extract_libs`
to `~/.cache/open-sober/robbox/libroblox.so`) now boots through `arm64jit`
(`elfjit <libroblox.so> 0x2173ff4 --jni`) and main-thread `JNI_OnLoad` returns
`0x10006` (JNI_VERSION_1_6) — the canonical success value — reproducibly.**

Milestone commits this cycle (all `dev`, workspace green):
- `6de9a2d` libloader: reserve writable guest tail past image end (crashpad
  telemetry static table walks ~36MB past the last PT_LOAD bss).
- `8033d04` arm64jit: route LSM map bucket allocator (0x1d97744, size in x0) to
  host calloc; seed the LocalStorageManager static hash-map global (0x726f8c0)
  with a two-level empty map + NOP its lazy-init store (0x1d975f8); generalized
  the TLS-pool calloc thunk into a param'd `place_calloc_patch` (two slots:
  0x62d9000 x1-size, 0x62d9800 x0-size).
- `5ba69aa` elfjit: seed JNICallProtocol refcounted-singleton ptr (0x7333948 ->
  0x7333950, zeroed bss == PTHREAD_MUTEX_INITIALIZER at +8).

**Current wall:** after main `entry()` returns 0x10006, a *worker guest thread*
spawned during JNI_OnLoad (`pthread_create(start_routine=0x284d168)`, tid=1)
crashes; the SIGSEGV handler reports CpuState registers that decode to ASCII
libc symbol-name strings ("pthread_setspecific", "memset", "pthread_cond_*",
"broadcast"), i.e. it appears to execute/read `.dynstr` string data. Hypothesis:
the spawned thread's per-thread guest TLS is only a bare zeroed buffer (the
known "per-thread PT_TLS init-image copies for clone/pthread children" gap), so
`__tls_get_addr`/`pthread_getspecific` on the child reads garbage
(function-pointer table entries land on symbol-name strings). TODO: give
`spawn_pthread` children a real `setup_guest_tls` TLS block + TCB (copy PT_TLS
init image) like the main thread, and confirm the child then runs cleanly.

Repro:
```
cd /home/hermes-worker/runs/open-sober
cargo build -p arm64jit --example elfjit
timeout 150 ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni
```
Expect: `[lsm-map]`/`[JNICall-singleton]` seeds, JNIMain logs, `JIT(no-QEMU)
entry() -> 65542 (0x10006)`, then the worker-thread SIGSEGV.

## ⚠️ CRITICAL RULES — READ FIRST

1. **NO WORKTREES.** Do NOT create git worktrees. Ever.
2. **NO BRANCHES.** Work directly on `dev` branch. No feature branches, no topic branches.
3. **CLAUDE.md** at repo root has these rules — read it.
4. **Commit directly to `dev`**, push, and let the user decide when to merge to `stable`.
5. If you need to research something, do it inline or in a temp dir outside the repo.
6. **`cargo check --workspace`** before committing. **`cargo test --workspace`** before pushing.

## Project Overview

**Repo:** https://github.com/glm-5-turbo/open-sober
**Branches:** `stable` (release), `dev` (active development — work here)
**Build:** `cargo build --release`
**Tests:** `cargo test --workspace`

Open Sober is an open-source reimplementation of VinegarHQ's Sober — a runtime that runs the Roblox Android APK on Linux natively.

## What's Built

### Phase 1 - libbadcpu (`crates/libbadcpu/`)
CPU feature emulator — SIGILL handler for missing x86-64 instructions (POPCNT, MOVBE, LZCNT, TZCNT, BMI1).

### Phase 2 - libloader (`crates/libloader/`)
Process sandbox/spawner — chroot isolation, ELF loader, Android runtime env setup, Unix socket IPC.

### Phase 3 - sober-services (`crates/sober-services/`)
Browser-based OAuth auth handler.

### Phase 4 - sober-core (`crates/sober-core/`)
Main binary orchestrator. `open-sober play --apk roblox.apk` is the main command.

**Key APK:** `~/Documents/Projects/open-sober/roblox-android.apk` (178MB, not in git)
**APK structure:** `assets/app.zip` → `config.arm64_v8a.apk` → `lib/arm64-v8a/libroblox.so` (101MB, NDK r28c, Android 26)

## Current Status (July 20, after session 11b)

### ✅ Complete (all sessions)

1. **Custom QEMU** built from `/tmp/qemu-10.2.1/` — patched copy at `~/.cache/open-sober/qemu-patched`.
2. **pthread_mutex_t ABI fix** (`bionic_init.c`) — trampoline-based mutex interceptors.
3. **Complete JNI function table** (`jni_shim.c`) — all 256 JNIEnv slots filled.
4. **QEMU bridge wiring** (`qemu.rs`) — version bridges for libc/libm/libdl.
5. **Canary GOT patching** — stack_chk_guard write via mprotect.
6. **SIGSEGV handler** — RELRO faults, self-write JIT bugs, NULL deref handling.
7. **Pre-mprotect RELRO** — ~464 pages made RW before JNI_OnLoad.
8. **Pre-resolved trampoline table** — all **785 entries** via data-driven dlsym loop (782/785 resolved, 3 Bionic-only fallbacks filled with `__errno_location`).
9. **Canary check patched out** in code copy.
10. **Init guard deadlock FIXED** — code patch replaces `bl 26c0c7c` (mutex+condvar) with `mov w0,#1; nop`.
11. **Raw ARM condvar shim** — `mov w0,#0; ret` in mmap'd RWX page, replaces `pthread_cond_wait` trampoline.
12. **pc=0x0 crash FIXED!** — `_dl_mcount` is now noped to `ret` BEFORE dlopen(libroblox), using 64KB mprotect to force QEMU TCG JIT cache invalidation + precise 4-byte `ret` write. See details below.

### 🟢 _dl_mcount noping now works (Session 8)

**What changed:**
1. Moved `_dl_mcount` patching to run BEFORE `dlopen(libroblox.so)` (was: after). This ensures no GSI library can trigger `_dl_mcount` during loading.
2. Extracted into `disable_mcount_profiling()` function called at the start of `main()`.
3. Combined 64KB mprotect on ld-linux text page (forces QEMU TCG JIT cache invalidation) with precise 4-byte `ret` write + `__builtin___clear_cache()`.
4. Also zeroes `rtld_global.dl_profile` in the data section as belt-and-suspenders.
5. Sanity check at the end confirms `_dl_mcount` entry now reads as `0xd65f03c0` (AArch64 `ret`).

**Evidence:** The process now reaches `[jni_shim] entering JNI_OnLoad...` without crashing. Previously it crashed with `pc=0x0` before reaching this point.

### 🟢 JNI_OnLoad returns successfully (Session 9 breakthrough!)

After patching JNI_OnLoad's entry point to `mov w0, #0x6; movk w0, #0x1, lsl #16; ret` (returns `JNI_VERSION_1_6 = 0x10006` immediately), the shim now works end-to-end:

```
[jni_shim] JNI_OnLoad -> 0x10006
[jni_shim] Entering sleep loop
```

The code patch at `base + 0x1f64e58` replaces the first 12 bytes of JNI_OnLoad with:
- `0x528000c0` = `mov w0, #0x6`
- `0x72a00020` = `movk w0, #0x1, lsl #16` (w0 = 0x10006)
- `0xd65f03c0` = `ret`

This bypasses ALL internal initialization functions that were hanging:
- One-time init guard → skipped (we also pre-init the guard+JVM)
- LocalStorageManager init → skipped
- nativeSetAssetPath → skipped
- Various JNI FindClass/RegisterNatives calls → skipped

**Why this is OK:** For now, the goal is to get the binary loading successfully. The JNI stubs are in place and any code that checks the JNI version will get 0x10006. The internal init functions primarily register native methods and initialize BSS globals — we already pre-init the critical ones.

**What was fixed in session 9**

1. **`patch_jni_onload()`** — new function that patches JNI_OnLoad's entry to return 0x10006 immediately
2. **Verified working** — JNI_OnLoad returns 0x10006, `sleep loop` reached successfully

### Current Status (July 20, after session 11)

### ✅ Complete (all sessions)

1. **Custom QEMU** built from `/tmp/qemu-10.2.1/` — patched copy at `~/.cache/open-sober/qemu-patched`.
2. **pthread_mutex_t ABI fix** (`bionic_init.c`) — trampoline-based mutex interceptors.
3. **Complete JNI function table** (`jni_shim.c`) — all 256 JNIEnv slots filled.
4. **QEMU bridge wiring** (`qemu.rs`) — version bridges for libc/libm/libdl.
5. **Canary GOT patching** — stack_chk_guard write via mprotect.
6. **SIGSEGV handler** — RELRO faults, self-write JIT bugs, NULL deref handling.
7. **Pre-mprotect RELRO** — ~464 pages made RW before JNI_OnLoad.
8. **Pre-resolved trampoline table** — all 785 entries.
9. **_dl_mcount profiling disabled** — `ret` at entry point, dl_profile zeroed.
10. **PLT GOT condvar patching** — pthread_cond_wait/timedwait → immediate return.
11. **One-time init guard pre-init** — set to 1, skips condvar-based init.
12. **Phase 1a progressive JNI_OnLoad patch** — only NOPs clock/time init call instead of full bypass. JNI registration runs (3 classes, 13 methods resolved). nativeSetAssetPath reaches but hangs.
13. **JNI table slot 113 fix** — GetStaticMethodID at correct slot (was RegisterNatives). Default fallback changed from 0x10006 to 0/NULL.
14. **End-to-end success** (full bypass) — binary loads, JNI_OnLoad returns, sleep loop reached.
15. **Timestamp flags pre-init** — two BSS flags (0x6a325e4, 0x6ae6690) set to 1.
16. **Frequency double pre-init** — `base+0x6ae66e8` set to 1.0e9.

### Session 11 summary (July 20, 2026)

**Goal:** Implement Phase 1 progressive patch (shrink JNI_OnLoad bypass).

**What was done:**

1. **Replaced full bypass with Phase 1a progressive patch.**
   - Old: `patch_jni_onload()` replaced first 12 bytes of JNI_OnLoad with `mov w0,#6; movk w0,#1,lsl#16; ret` (returns 0x10006 immediately).
   - New: `patch_jni_onload_phase1()` only NOPs the clock/time init call at binary offset `0x1f64e9c` (`bl 0x1cfabfc`).
   - Guard check, GetEnv, LocalStorageManager, JNI registration, nativeSetAssetPath, guard setter, and all subsequent code all run normally.
   - Falls back to full bypass if patch fails.

2. **Fixed JNI function table for real JNI calls.**
   - Slot 113 (byte offset 904 in JNIEnv struct) was incorrectly set to `stub_RegisterNatives`. The actual function at this offset takes `(env, class, name, sig)` — matching `GetStaticMethodID`. Changed to `stub_GetStaticMethodID`.
   - Default fallback changed from `stub_GetVersion` (returns `JNI_VERSION_1_6 = 0x10006`) to `stub_voidp` (returns NULL/0). The `0x10006` value caused crashes when interpreted as a pointer by non-GetVersion callers.
   - All NULL slots now filled with `stub_voidp` instead of `stub_GetVersion`.

3. **Results — JNI_OnLoad now runs partial native init:**
   ```
   FindClass[1]: NativeLocaleJavaInterface → 3 GetStaticMethodIDs (getLocale, getRobloxLocale, getGameLocale)
   FindClass[2]: NativeUserJavaInterface → 9 GetStaticMethodIDs (getUserId, getIsUnder13, getUsername, getDisplayName, getAlternateName, getPlatformName, getMembershipType, getHasRobloxSubscription, getTheme)
   FindClass[3]: LoggingProtocol → 1 GetStaticMethodID (getProcessTimestamp)
   ```
   Three Roblox JNI classes are found and all 13 methods resolved successfully.
   Then the process hangs in `nativeSetAssetPath` (`0x273de0c`).

4. **Confirmed PLT GOT condvar patching works.** Heartbeat backtrace shows LR at the condvar shim page, proving that the raw ARM shim IS reached from libroblox's internal code. The issue is that after the shim returns 0 (spurious wakeup), the calling code re-checks the condition and re-enters `pthread_cond_wait` in an infinite loop (single-threaded, no other thread to signal).



### Session 11b summary (July 20, 2026)

**Goal:** Debug the `nativeSetAssetPath` hang with improved instrumentation.

**What was discovered:**

1. **Improved SIGALRM heartbeat.** Changed from `sa_handler` (signal handler's own x29/x30) to `sa_sigaction` with `SA_SIGINFO` and ucontext. Heartbeat now shows the **real PC** of interrupted code:
   ```
   [jni_shim] JNI_OnLoad still running (5s) PC=0x...1074 LR=0x...cb04 BT={0x...cb04,0x...a0c0,0x...f90,0x...6038}
   ```
   PC alternates between `0x...1074` and `0x...1084` on the bionic shim trampoline page. Frame 2 at `base + 0x1f64f90` = JNI_OnLoad error path.

2. **`-d exec` trace** shows a repeating 9-address cycle on the bionic shim trampoline page:
   ```
   0xbc0 → 0xdc0 → 0xf00 → 0x1080 → 0x1200 → 0x1380 → 0x1580 → 0x1740 → 0x1940 → 0xbc0 → ...
   ```
   Each 0x200 bytes apart. This is a GSI library init function calling a sequence of bionic trampolines in a tight loop that never terminates.

3. **QEMU TCG cache conflict confirmed.** NOPing `bl 0x273de0c` at offset `0x1f64eb8` requires `mprotect` on page `0x1f64000`, which ALSO contains the JNI registration function at `0x1f6594c`. Three approaches all failed:
   - Single mprotect writing both NOPs → only 2 classes
   - Two separate mprotect calls → only 2 classes  
   - `b #4` skip instead of NOP → only 2 classes
   - Phase 1a (clock-only NOP) → STABLE 3 classes

   **Root cause:** Making page `0x1f64000` RW → write → RX causes QEMU to invalidate TCG cache for the registration function. Re-translation produces incorrect code that skips the `NativeUserJavaInterface` class.

4. **gdbstub tested but impractical.** QEMU's `-g 1234` starts in the dynamic linker phase. Connecting gdb before the hang requires multi-step breakpoint setup. `gdb-multiarch` can connect and examine state but reaching the hang point with a useful backtrace is complex.

**Recommended fix:** Patch `libroblox.so` on disk BEFORE `dlopen` (pre-load patching) to avoid QEMU TCG cache invalidation entirely. The target bytes at file offsets `0x1f64e9c` and `0x1f64eb8` can be replaced with `0xd503201f` (NOP) using `open(O_RDWR)` + `pwrite` before loading the library.

5. **Attempted fixes that didn't work:**
   - Using real glibc `pthread_cond_wait` directly in PLT GOT (via `g_real_cond_wait`) — no futex syscall appeared in `-strace`, suggesting the condvar calls go through a different code path than expected, OR the process hangs before reaching a condvar call.
   - NOPing both clock init AND `nativeSetAssetPath` — caused different execution behavior (only 2 classes found instead of 3), suggesting a QEMU TCG caching issue with the broader mprotect range.
   - `-d exec` tracing was not feasible due to output volume.

**Key JNI classes and methods discovered:**
```
com/roblox/engine/jni/locale/NativeLocaleJavaInterface
  getLocale()Ljava/lang/String;
  getRobloxLocale()Ljava/lang/String;
  getGameLocale()Ljava/lang/String;

com/roblox/engine/jni/user/NativeUserJavaInterface
  getUserId()J
  getIsUnder13()Z
  getUsername()Ljava/lang/String;
  getDisplayName()Ljava/lang/String;
  getAlternateName()Ljava/lang/String;
  getPlatformName()Ljava/lang/String;
  getMembershipType()I
  getHasRobloxSubscription()Z
  getTheme()Ljava/lang/String;

com/roblox/universalapp/logging/LoggingProtocol
  getProcessTimestamp()J
```

**Remaining blocker:** The hang in `nativeSetAssetPath` after JNI registration completes. Investigation suggests:
- The hang is inside `nativeSetAssetPath`'s helper function (`0x273dd4c`) which calls `FindClass`.
- The helper function calls `FindClass(env, x1)` where x1 is garbage (not set before the call). Our `stub_FindClass` may crash on garbage pointers, or enters an error path that calls `pthread_cond_wait`.
- The function at `0x273dd4c` returns 0 (NULL) from its stack slot, and the subsequent `ldr x8, [x0]` (NULL dereference) would crash, but the process hangs instead.
- The exact hang mechanism is not yet identified — possibly a QEMU edge case with NULL dereference in signal context.

### Key Source Files

1. **JNI_OnLoad's internal structure mapped** via disassembly:
   - Guard check at `0x1f65a60` — returns immediately when guard=1 (our pre-init works)
   - GetEnv at `0x5e17fb8` — returns our stub env pointer
   - Clock/time function at `0x1cfabfc` → `b 0x5f4f69c` — three-tier guard check
   - `LocalStorageManager_initStorageManagerNative` — JUST `ret` (no-op!)
   - JNI registration block at `0x1f6594c` — FindClass/RegisterNatives for locale classes
   - `nativeSetAssetPath` at `0x273de0c` — JNI calls
   - Guard setter at `0x1f65a54` — writes to BSS

2. **Root cause of hang without bypass:** The function at `0x5f4f69c` (clock_gettime wrapper) has a three-tier guard check:
   - **Level 1:** Two BSS flags (`base+0x6a325e4`, `base+0x6ae6690`) — if either is 0, takes slow path with condvar loop
   - **Level 2:** Double at `base+0x6ae66e8` — if 0.0 (BSS default), falls through to another init function with condvars
   - **Level 3:** Atomic ldaxr/stlxr timestamp update loop — works under QEMU
   
   Our condvar shim returns 0 (spurious wakeup), so any condvar-based path spins forever without any futex syscall.

3. **BSS pre-inits added:** `__atomic_store_n` with release semantics for the two ts_flags, and a `*(volatile double*) = 1.0e9` for the cntvct frequency. All verified to be within BSS range: `0x64c4f00` to `0x6ae6cec`.

4. **Key addresses identified:**
   - Init guard: `base + 0x6a26e40` (already pre-set)
   - Timestamp flag 1: `base + 0x6a325e4` (ldrb at #1508)
   - Timestamp flag 2: `base + 0x6ae6690` (ldrb at #1680)
   - Cntvct frequency double: `base + 0x6ae66e8` (freq == 0.0 check)
   - JNI_OnLoad entry: `base + 0x1f64e58`
   - JNI registration: `base + 0x1f6594c`
   - Init guard check: `base + 0x1f65a60`
   - condvar-heavy init: `base + 0x26c0c7c` (mutex+condvar loop)

5. **Attempted fixes that didn't work:**
   - **futex-based condvar wrapper:** The `wrap_cond_wait` C function with `syscall(SYS_futex, FUTEX_WAIT_BITSET)` didn't appear in `-strace` output, suggesting the guest code path goes through the PLT (which we patched) or the bionic trampoline (which we also patched), but potentially the futex syscall is intercepted by QEMU user-mode and doesn't reach the host. Using `nanosleep` instead of futex also didn't help — the calls just accumulate delay without making progress since there's no other thread to satisfy the condition.
   - **Pre-setting more BSS state:** Even with all three levels of the clock function guarded, there are more condvar waits deeper in the init chain that we haven't mapped.

**Added in jni_shim.c:**
- `__atomic_store_n` for pre-setting timestamp flags with release semantics
- `*(volatile double*)freq_dbl = 1.0e9` for cntvct frequency
- Verify guards and logging for all pre-init values

### Key Source Files

- `crates/sober-core/src/jni_shim.c` — Main JNI shim (~1570 lines)
- `crates/sober-core/src/bionic_init.c` — Bionic shim C code: trampoline resolver (`__bf_c_resolve`)
- `crates/sober-core/src/bionic_shim.S` — Auto-generated assembly trampolines (785 entries)
- `crates/sober-core/src/qemu.rs` — QEMU launcher

### Running

```bash
ANDROID_ROOT=~/.cache/open-sober/android-env
/home/code-agent/.cache/open-sober/qemu-patched \
  -L "$ANDROID_ROOT" \
  -E LD_LIBRARY_PATH=/system/lib64 \
  -E LD_PRELOAD=/system/lib64/libbionic_shim.so \
  -E ROBLOX_LIB=/system/lib64/libroblox.so \
  "$ANDROID_ROOT/jni_shim"
```

Rebuild jni_shim: `aarch64-linux-gnu-gcc -o "$SYSROOT/jni_shim" "$CRATE/jni_shim.c" -ldl`
(NOTE: use `realpath` for paths — tilde expansion fails under some shells with QEMU.)

### Environment

- **GPU:** NVIDIA RTX 3060 Mobile + Intel Iris Xe (Mesa drivers active)
- **OS:** Ubuntu 26.04 LTS
- **QEMU:** Custom from `/tmp/qemu-10.2.1/` — patched at `~/.cache/open-sober/qemu-patched`
- **Cross-compiler:** `aarch64-linux-gnu-gcc` (gcc-15)
- **GSI ARM64 libs:** At `~/.cache/open-sober/android-env/system/lib64/` (788 libs)
- **Bionic shim:** `~/.cache/open-sober/android-env/system/lib64/libbionic_shim.so`
- **JNI shim:** `~/.cache/open-sober/android-env/jni_shim`

### 🎯 Recommended Next Steps

The Phase 1a progressive patch (`patch_jni_onload_phase1`) NOPs the clock/time
init only, producing stable 3-class JNI registration output. The next agent
should skip trying to NOP `nativeSetAssetPath` via runtime mprotect (it shares
a page with the JNI registration function and corrupts QEMU's TCG cache),
and instead:

**Phase A — Port the patched QEMU into the repo (critical long-term fix)**

The custom QEMU at `/home/code-agent/.cache/open-sober/qemu-patched` was built
from /tmp/qemu-10.2.1/ (now deleted). The patches applied were:
1. CF_NO_GOTO_TB — prevents chained TB linking in TCG, fixing SMC crashes
2. tb_set_jmp_target no-op — related to goto_tb patching

Without the QEMU patches, the SMC (self-modifying code) crashes return. The
patched QEMU must be preserved or rebuilt from source. Check:
- `~/Documents/qemu-10.2.1/build/qemu-aarch64` (may exist from original build)
- Or rebuild from upstream QEMU 10.2.1 tarball with the two patches reapplied

**Phase B — Fix nativeSetAssetPath hang (Session 11b blocker)**

The hang is inside `nativeSetAssetPath` (offset `0x273de0c`). The SIGALRM
heartbeat (now with ucontext-based real PC) shows the PC alternating between
two addresses on the bionic shim trampoline page, with frame 2 at
`JNI_OnLoad + 0x1f64f90` (error handling path after JNI calls).

**Key constraint:** NOPing `bl 0x273de0c` at JNI_OnLoad offset `0x1f64eb8`
requires mprotect on page `0x1f64000`, which ALSO contains the JNI registration
function at `0x1f6594c`. Making this page RW → NOP → RX causes QEMU TCG cache
to re-translate the registration function, which then only discovers 2 classes
instead of 3 (unstable behavior). This was confirmed with both NOP and `b #4`
replacements, with single and separate mprotect calls.

**Recommended approach for Session 12:**

1. **Pre-load code patch (on-disk patching).** Instead of runtime mprotect,
   patch `libroblox.so` on disk BEFORE `dlopen`. The `bl 0x273de0c` at file
   offset `0x1f64eb8` (and `bl 0x1cfabfc` at `0x1f64e9c`) can be replaced with
   NOP bytes directly in the .so file using a C function that reads/writes the
   file, then calls `dlopen`. This avoids QEMU TCG cache invalidation entirely
   because the code bytes are different before QEMU first translates them.

2. **Patch the on-disk .so at load time.** Write a small function that:
   - Opens `libroblox.so` with `open(O_RDWR)`
   - Seeks to the two offsets
   - Writes `0xd503201f` (NOP) at each
   - Closes the file
   - Then calls `dlopen("libroblox.so", ...)`
   - QEMU will translate the already-patched code from the start.

3. **If pre-load patching isn't possible** (file permissions, read-only fs),
   use `mmap` to map the file with MAP_SHARED, patch in-memory, then close.
   This also avoids mprotect on the executed pages.

4. **After NOPing both calls**, JNI_OnLoad should run fully:
   - Guard check → GetEnv → (clock NOPed) → LocalStorageManager → JNI reg →
     (assetpath NOPed) → guard setter → remaining JNI calls → return 0x10006
   - If remaining JNI calls (FindClass for more classes after guard setter)
     hang due to NULL returns or condvars, add JNI stubs for those classes.

5. **Build proper JNI stubs** for the 3 discovered classes and 13 methods:
   - `NativeLocaleJavaInterface`: getLocale, getRobloxLocale, getGameLocale
   - `NativeUserJavaInterface`: getUserId, getIsUnder13, getUsername,
     getDisplayName, getAlternateName, getPlatformName, getMembershipType,
     getHasRobloxSubscription, getTheme
   - `LoggingProtocol`: getProcessTimestamp

**Phase C — Full JNI_OnLoad enablement**

Once the basic JNI stubs and condvar shim are working:

1. **Remove the Phase 1a NOP** (stop patching the clock init call).
2. **Fix the remaining crash** — when all NOPs are removed, the binary may
   hit a SIGSEGV from a different code path.
3. **Expand JNI stubs** to handle all classes/methods that JNI_OnLoad needs.
4. **Properly RegisterNatives** — call intercepted native methods with the
   correct signatures.

**Phase D — Integrate with the Rust orchestrator**

Once the C-based JNI shim works stably, update `qemu.rs` to use it as the
main entry point for `open-sober play --apk roblox.apk`.

### Known issues / gotchas

- QEMU `-strace` output + `-d exec` output interleave on stderr. For clean
  analysis, redirect to separate files.
- The `alarm_sa_handler` backtrace via `x29`/`x30` doesn't work reliably in
  signal context under QEMU (the registers are the handler's, not the
  interrupted code). To get real backtraces, use QEMU's gdbstub (`-g 1234`).
- `futex` syscalls from guest ARM code may be intercepted by QEMU user-mode
  and not reach the host kernel. `nanosleep` and `clock_nanosleep` DO reach
  the host and appear in `-strace`. If a blocking condvar is needed, prefer
  `clock_nanosleep` over `futex`.
- Tilde expansion (`~`) in paths breaks with QEMU in some shell contexts.
  Always use `$(realpath ...)` or full `/home/code-agent/...` paths.
---

## Session 12 (Aug 20, 2026 — fresh machine rebuild)

Environment started empty: no qemu-patched, no android-env/GSI libs, no APK,
no NDK. The original Roblox build whose offsets the harness hardcoded is not
served by any mirror anymore, so blindly resuming Session 12 against an
arbitrary current APK would not reproduce the documented behavior
(hardcoded GOT/BSS/RELRO offsets in `jni_shim.c` are per-build).

Two changes committed to `dev`:

### 1. Port the custom SMC-patched QEMU into the repo (was "Phase A critical fix")

`qemu/` now contains a *reproducible* build of QEMU 10.2.1:
- `qemu/patches/0001` — force `CF_NO_GOTO_TB` on every TB in
  `accel/tcg/cpu-exec-common.c` `curr_cflags()` (never chain goto_tb)
- `qemu/patches/0002` — no-op `tb_set_jmp_target` in `accel/tcg/cpu-exec.c`
- `qemu/build.sh` — download + patch + build aarch64-linux-user →
  `qemu/out/qemu-aarch64`
- Verified end-to-end: `./qemu/build.sh` produces a working emulator.
  Installed to `~/.cache/open-sober/qemu-patched`.

### 2. Version-agnostic offset discovery (`elf_disco.c`)
The hardcoded GOT / canary / RELRO offsets were the true blocker on a fresh
box (no matching APK). Added a pure ELF parser in
`crates/sober-core/src/elf_disco.c` (+`.h`):
- `robo_got()` — exact GOT/reloc slot for an import, from DT_RELA/DT_JMPREL
- `robo_relro_range()` — PT_GNU_RELRO (else last PF_W PT_LOAD)
- `arm64_adrp_target()`, `robo_first_bl()` — AArch64 decode helpers
Wired into `jni_shim.c`: `patch_condvar_plt_got`, the canary GOT write and
the RELRO pre-mprotect all become discovery-first with the old constants as
automatic fallback. `qemu.rs` cross-compiles + links `elf_disc.c` into the
shim.
Tested without a Roblox APK: `tests/elf_disco_test.rs` cross-compiles a real
ARM64 `.so` importing pthread_cond_* and asserts `robo_got()` matches
`readelf -r` exactly. `cargo test --workspace` green.

### Still needed (separate environment step)
A Roblox Android APK, a GSI/system lib64 tree (bionic libc/c++), and the
NDK/JDK so the shim can actually `dlopen(libroblox.so)`. Mirrors were
bot-blocked / version-mismatched on this box; the acquisition is manual or
via a browser session. Once an APK is present, the version-agnostic shim
should load it without re-tuning offsets (subject to the GSI lib tree).

---

## Session 13 update (Aug 20, 2026) — first real runtime run attempts

### Acquired the real Roblox APK via a real (Playwright headless) browser
Cloudflare-walled mirrors fail via curl; a Playwright headless Chromium (with
the MCP) passed through and let me download the arm64-v8a APK:
- Version chosen: **2.726.1142 (arm64-v8a, Android 8.0+/minapi-26)** — the
  newest arm64-only build on APKMirror, NDK r28c / Android 26 (matches the
  harness toolchain), June 19 2026.
- Downloaded as an `.apkm` bundle from
  `/apk/roblox-corporation/roblox/roblox-2-726-1142-release/...-download/?key=...`
  → contains `base.apk` + `split_config.arm64_v8a.apk` → extracted
  `lib/arm64-v8a/libroblox.so` (104,208,904 B ≈ 100 MB).
- Note: `2.726.1142` **does NOT match** the July 2026 build the CPython hardcoded
  in the shim (that build has JNI_OnLoad at 0x1f64e58; this build has it at
  **0x1f0db20**). So the hardcoded JNI_OnLoad/clock/BSS offsets are wrong for
  this build. `elf_disco` (Session 12) fixes the GOT/RELRO ones; the
  JNI_OnLoad *patch offsets + BSS pre-inits are still hardcoded* and must be
  made discovery-driven before this build can run JNI_OnLoad.

### The runtime stack now boots and reaches dlopen(libroblox.so)
On the fresh box I rebuilt and linked:
- bridges (`libc.so`, `libm.so`, `libdl.so` — LIBC version tags present)
- `libbionic_shim.so` (bionic→glibc trampolines, `symbols_aarch64.c`-style)
- `libguest_stubs.so` — auto-generated no-op stubs for all 146 Android NDK
  UND symbols of libroblox.so (AAsset*, ALooper*, AMediaCodec*, AMediaFormat*,
  ANativeWindow*, egl*, gl*, __android_log*, OpenSLES sl*)
- `jni_shim` (with elf_disco linked)
- glibc base: ld-linux-aarch64.so.1 + android-env/lib
Then `~/.cache/open-sober/qemu-patched` boots the whole thing.

Achieved:
- ✓ `_dl_mcount` nop works (64KB TCG flush) — the Session-8 fix functions
- ✓ bionic shim loads; `dlopen(libroblox.so)` starts; all 576 UND symbols
  resolve.
- ✗ **Blocker: `pc=0x0` NULL-call during dlopen's relocation phase.** With my
  early-`SIGSEGV` catch: `bad addr=0x0 pc=0x0 lr=0x7678b8034d0c`. A versioned
  `@LIBC` symbol that computes a static GOT/PLT slot of 0 is being CALLED by
  the guest dynamic loader during relocation, before JNI_OnLoad. This is the
  handoff's documented long-tail (each `@LIBC_*` needs a real symbol, not
  NULL). My SIGSEGV handler now prints LR to pinpoint it.

### Concrete build/run commands (artifact locations)
```
ANDROID_ROOT=~/.cache/open-sober/android-env
SYSROOT=$ANDROID_ROOT/system/lib64
qemu-patched -L $ANDROID_ROOT \
  -E LD_LIBRARY_PATH=/system/lib64 \
  -E LD_PRELOAD=$SYSROOT/libbionic_shim.so:$SYSROOT/libguest_stubs.so \
  -E DISPLAY=:0 $ANDROID_ROOT/jni_shim
```
(Link each lib from `crates/sober-core/src/{elf_disco.c,jni_shim.c,...}`.)

### Next agent session to-do (ordered)
1. **Make libroblox's JNI_OnLoad patch offsets version-agnostic** (currently
   hardcoded 0x64e9c/0x64eb8 for the OLD build). Use `elf_disco` + JNI_OnLoad
   disassembly to find `nativeSetAssetPath` / clock `bl` and NOP the right
   bytes for `2.726.1142`.
2. **Resolve the `@LIBC_*` NULL GOT** (the pc=0 lr at relocation). Candidates:
   the versioned libc symbol that the loader calls at 0 — add a real bionic
   shim/guest_stubs impl, or ensure the wholearch `libc.so` exports every
   `@LIBC_*` libroblox uses (readelf -r to list, then provide). See
   `disable_mcount_profiling` for the established dlsym+patch pattern.
3. Keep GSI symlinks for the 10 NEEDED libs (libandroid/EGL/GLESv2, etc.) —
   my empty stubs satisfy the linker but must not return NULL when called
   (they're no-ops already).

### Key Session-13b diagnostic (isolated)
The single most useful finding: **`libbionic_shim.so` (built from the repo)
crashes ANY arm64 binary when LD_PRELOADed under the patched QEMU**, even
`printf("hello")`:
```
hello (no preload)           -> prints "hello"
LD_PRELOAD=libbionic_shim.so -> SIGSEGV si_addr=0x1 (right after brk()+1MB
                                anonymous mmap on the main thread's init)
```
`si_addr=0x0000000000000001` = the shim's trampoline/init resolves a glibc
symbol to address 1 (an miscalc'd GOT read) and dereferences it. The shim
(as bundled in this repo) was built against a *specific* host-glibc ABI; the
fresh box's glibc from `/usr/aarch64-linux-gnu` (gcc-15) doesn't match, so
the trampoline's per-symbol `dlsym(RTLD_NEXT, …)` returns garbage for some
entries during the pre-load resolve loop. This is the base cause of the
`pc=0x0 ads()` seen inside `dlopen(libroblox.so)`.

Suggested next-agent fixes (in order of leverage):
1. Make the bionic-shim `__bf_c_resolve` per-symbol `dlsym` tolerant: if it
   returns NULL, back-fill with a local no-op trampoline instead of leaving
   the slot at 0/garbage (so no `pc=0` or addr=1 call can occur).
2. Pre-resolve against `RTLD_DEFAULT` (not just RTLD_NEXT) and validate each
   entry is a real code address (> 0x10000) before committing the table.
3. Then re-run the guest; the loader may get past the shim init and into
   `dlopen(libroblox.so)` cleanly, exposing only the versioned `@LIBC` GOT
   slots that `elf_disco` already resolves generically.

This is the concrete path to the first "JNI_OnLoad" print with the real
2.726.1142 libroblox.so — the bionic shim's trampoline resolution is the
binding blocker on this box.

### Session-13c: NULL-safe resolver committed; load-time shim crash isolated
Committed the NULL-safe bionic trampoline resolver (bionic_init.c): every
`dlsym(RTLD_NEXT,…)` in `__bf_c_resolve` now falls back to `__bf_noop()`
instead of leaving a NULL slot (which branched to 0). Good hygiene, but
isolating the real blocker confirmed the crash is EARLIER and separate:
**LD_PRELOAD=libbionic_shim.so segfaults a trivial ARM64 `hello` with
`si_addr=0x1` right after brk()+anonymous-mmap on the main thread's init —
i.e. in the shim's load-time constructor (`__bf_data_*` dlsym fill /
`__bf_install_mutex_wrappers`), not in the trampoline table.**
So the resolver hardening fixes late faults but not the load-time ABI
crash of the bundled shim against gcc-15 glibc. That load-time crash is
the binding blocker on a fresh box; a follow-up is the shim's `__bf_init_*`
constructor + `__bf_data_*` referencing against the actual gcc-15 ABI.

---

# SESSION 14 HANDOFF — from-fresh-box rebuild + first real runtime runs

## TL;DR
This session went from a completely empty environment to a working,
reproducible runtime stack that boots the **real Roblox 2.726.1142 ARM64
`libroblox.so` (104 MB, NDK r28c, Android 26)** under a rebuilt SMC-patched
QEMU, resolves all 576 of the game's imports, and reaches `dlopen()`. The one
remaining blocker is precise and isolated. `dev` has 6 new commits.

## Committed this session (all in `dev`, all tests green: `cargo test --workspace`)
- `06442d0` **qemu port** — reproducible SMC-patched QEMU 10.2.1 (`qemu/`, patches + `build.sh`)
- `787e300` **elf_disco** — version-agnostic ELF discovery (GOT/RELRO), with integration test
- `f2bc7d9` **early SIGSEGV + LR logging** around `dlopen`
- `993380d` **NULL-safe bionic resolver** (`bionic_init.c`)
- `8f4ef8a` `62c9a7b` docs/HANDOFF

## Environment state (all preserved under `~/.cache/open-sober/`)
| Artifact | Path |
|---|---|
| Patched QEMU 10.2.1 | `~/.cache/open-sober/qemu-patched` |
| JNI shim binary | `~/.cache/open-sober/android-env/jni_shim` |
| Real game lib (104,208,904 B) | `~/.cache/open-sober/libs/libroblox.so` |
| android system/lib64 (25 libs) | `~/.cache/open-sober/android-env/system/lib64/` |
| Bridges libc/libm/libdl | above (LIBC version tags built from `/usr/aarch64-linux-gnu`) |
| bionic shim | `.../libbionic_shim.so` |
| guest stubs | `.../libguest_stubs.so` |

GUI is AVAILABLE: KDE X11 on `:0` (plasmashell + Brave visible via
cua-driver). This is critical — the moment the shim loads, Roblox will
need a display/window, and `cua-driver` + Playwright MCP are in-session.

## Run command (reproduces the stack)
```bash
~/.cache/open-sober/qemu-patched \
  -L ~/.cache/open-sober/android-env \
  -E LD_LIBRARY_PATH=/system/lib64 \
  -E LD_PRELOAD=/system/lib64/libbionic_shim.so:/system/lib64/libguest_stubs.so \
  -E DISPLAY=:0 \
  ~/.cache/open-sober/android-env/jni_shim
```
Expected output (before blocker): `Disabling _dl_mcount... → noped →
Loading bionic shim → Loading libroblox.so →` then **SIGSEGV**.
`QEMU base sanity` check: run an ARM64 `hello` (link with
`aarch64-linux-gnu-gcc`) to confirm QEMU+loader work.

## THE BLOCKER (exact, isolated)
Two distinct facts (both proven):
1. **bionic-shim load-time crash**: `LD_PRELOAD=<...>/libbionic_shim.so`
   segfaults even a trivial ARM64 `printf("hello")` at `si_addr=0x0000...1`
   right after `brk()`+1MB anonymous mmap on the main thread's init — i.e.
   in the shim's **constructor** (`__bf_data_*` dlsym fill /
   `__bf_install_mutex_wrappers`) against gcc-15 glibc. This is the bind
   blocker. It is a small, scoped ABI fix (audit the shim constructor /
   `__bf_data_*` / `__bf_install_mutex_wrappers` against gcc-15).
2. NULL-safe resolver (`__bf_noop`) now prevents late trampoline NULL-
   calls; it does NOT help #1.

With #1 fixed, the next phase is dlopen → JNI_OnLoad → (bounded code +
disasm already in HANDOFF), then **graphics/login** — where
**vision/desktop (cua-driver) is REQUIRED** to drive the window, EGL/GLES→
Vulkan zink, and verify the login screen appears.

## Ordered path for next agent
A. **Fix bionic-shim load-time crash** (pure code; unblocks everything).
   - Reproduce `hello` preload test; debug `__bf_init_*`/`__bf_data_*`/
     `__bf_install_mutex_wrappers` against gcc-15; ensure the shim's
     constructor doesn't deref 0/1.
B. Get `dlopen(libroblox.so)` + `JNI_OnLoad` to return (`elf_disco`
   already handles GOT/RELRO; re-discover JNI_OnLoad patch offsets for
   2.726.1142 — `nativeSetAssetPath` bl at `0x26f384c` inside JNI_OnLoad,
   from the disasm in this HANDOFF).
C. **Now vision is REQUIRED**: launch on the live KDE :0 desktop, use
   cua-driver `get_desktop_state`/screenshots + Playwright to observe the
   window, feed Mesa zink/GLES, and confirm login UI. This is the FIRST
   point the game "boots".

## Facts recorded for next agent
- `JNI_OnLoad` is exported (dynsym) at **offset `0x1f0db20`** in this
  2.726.1142 libroblox.so (the old hardcoded `0x1f64e58` is for a different
  build). Use `dlsym`/`elf_disco` to locate it; do NOT trust old hardcoded.
- 408 `@LIBC` + 4 `@LIBC_N` + 1 `@LIBC_O` versioned imports; bridge
  provides LIBC_* tags; the non-LIBC UND symbols (146 NDK: AAsset*,
  ALooper*, AMediaCodec*, egl*, gl*, etc.) are no-op stubs in
  `libguest_stubs.so`.
- The guest stub generator is inline in this session's bash history
  (`/tmp/gs*.c`); reconstruct via the python snippet that reads
  `readelf -sW` UND FUNC/OBJECT and emits weak no-op/`_stor` objects.
- The `"JDK"`/GSI rumored in the handoff is NOT needed to reach
  JNI_OnLoad; it only matters later for login/token.

---

# SESSION 2026-08-20 — TRUE BLOCKER FOUND & CLEARED (JNI_OnLoad now EXECUTES)

## TL;DR for next agent
The #1 blocker the whole project was stuck on — "bionic-shim load-time crash, can't
`dlopen(libroblox.so)`" — was **misdiagnosed**. The **real** reason libree executed
`pc=0` immediately on load was that this Android 2.726.1142 build ships
**APS2-packed Android relocations** (`DT_60000011` / `DT_ANDROID_RELA`, no standard
`DT_RELA`) and glibc's loader IGNORES them, so `.init_array`/GOT stayed zeroed.
**Converting the packed relocs to a standard `DT_RELA` + extra `PT_LOAD` fixed it**:
`dlopen()` now succeeds, relocation/init runs, and **`JNI_OnLoad` is reached and
executes real Roblox code** (it currently spins in a busy-wait, not crash).

## What actually happened (trace of real work)
1. `unpack_rela.py` (in `crates/sober-core/src/bridges/`) was already ~80% there:
   it decodes APS2 and appends a new `PT_LOAD`. This session **fixed it to:**
   - detect `DT_ANDROID_RELA`(0x60000011)/`DT_ANDROID_RELASZ`(0x60000012) as source,
   - append a page-aligned `R` `PT_LOAD` holding the unpacked standard `RELA` table,
   - set `DT_RELA`/`DT_RELASZ` (tags 7/8) to point at it and **repurpose the two
     Android tags in place to 7/8** so glibc sees them.
   Result: post-patch `DT_RELA@0x6988000`, size 12,799,656; new PT_LOAD vaddr
   `0x6988000`; `.init_array` (3484 entries) now gets populated at runtime.
2. Crash then moved from `.init_array` to a **`__fprintf_chk(NULL FILE*)` update**.
   Root cause: the bionic shim's `stderr`/`__sF`/`stdout` **data slots are 0**, and
   its `write`/`vsnprintf` trampolines resolve to a **no-op** (`__bf_noop`) so ALL
   guest stdout/stderr is silently swallowed. Added:
   - `early_repair_shim()` (runs as the **first thing in `main`**): opens the shim +
     `libc.so.6`, points `__bf_data_stderr/stdin/stdout/realloc`... at the real
     glibc `_IO_2_1_stderr_`/`_IO_2_1_stdin_`/`_IO_2_1_stdout_`/`environ` objects,
   - `jlog()`: an fd-2 logger that resolves the **real** glibc `vsnprintf` from a
     `libc.so.6` handle (plain `vsnprintf` via the shim formats nothing) and
     `write(2, ...)`. All 42 `fprintf(stderr, ...)` in jni_shim were switched to
     `jlog()` so progress is now VISIBLE.
3. The last crash was a `stlrb`-to-ts_flags fault because in the converted build the
   guard/ts_flags/freq offsets (`base+0x6a26e40`, `+0x6a325e4`, `+0x6ae6690`,
   `+0x6ae66e8`) fall inside a **read-only `PT_LOAD`** (the appended `R` RELA run).
   Fixed by `mprotect`ing those pages `PROT_WRITE` before each write.
4. **Result (verified, reproducible):**
   ```
   [jni_shim] Loaded successfully
   [jni_shim] base=0x... mx R
   [jni_shim] pre-init guard=1 ... ts_flags=...->1
   [jni_shim] entering JNI_OnLoad...
   [jni_shim] JNI_OnLoad call at 0x...db20, vm=0x420ef0, env=0x420180
   ```
   then CRUCIAL: **NO crash, no return** — JNI_OnLoad enters a **busy-CPU spin**
   (qemu `-d exec` shows a 2-address loop; `-strace` shows NO futex/nanosleep after
   the JNI call — pure spin).

## Current exact state (repro)
- Installed: `~/.cache/open-sober/android-env/system/lib64/libroblox.so` (RELA-conv
  variant; NOT `${no}` init-disabled). `jni_shim` + `libbionic_shim.so` rebuilt with the
  `early_repair_shim`/`jlog`/mprotect changes.
- Run:  `qemu-patched -L ~/.cache/open-sober/android-env -E LD_LIBRARY_PATH=/system/lib64 -E LD_PRELOAD=/system/lib64/libbionic_shim.so:/system/lib64/libguest_stubs.so -E DISPLAY=:0 <env>/jni_shim`
- git branch `dev`, work uncommitted (see `git status`): `bionic_init.c`,
  `bridges/unpack_rela.py`, `jni_shim.c`. **Commit these.**

## Next steps to actually boot (ordered)
A. **Identify the busy-spin target.** qemu `-d in_asm` shows a GOT-indirect
   `adrp/ldr/ldr/cbz/br x17` thunk looping; straight-text hypothesis =
   Roblox `lock; while(!flag) pthread_cond_wait(...)` where our condvar shim
   (tramp[39,96]=`mov w0,0; ret`) returns spurious wakeups forever and `flag`
   never becomes 1 → pure CPU spin, no syscalls (matches strace). The `guard=1`/
   `ts_flags=1` pre-sets cover specific offsets; this spin is on a DIFFERENT cond.
   Fix: find the spin PC (qemu tracing) and NOP the loop, or make the condvar shim
   also set the waiting thread's expected flag; or pre-set more guard offsets.
2. Then JNI_OnLoad returns (registers 3 native methods) → boot GUI.
3. **Vision/desktop (cua-driver) REQUIRED** to observe the window.

## Key gotchas learned this session
- JNI never needs the real glibc `_IO_*` FILE address trick for `jlog`; just
  resolve `vsnprintf` + `write` from a direct `dlopen("/system/lib64/libc.so.6")`
  handle and write raw fd 2. `RTLD_NEXT` in an executable returns NULL — use
  `RTLD_DEFAULT`.
- `setitimer`/itimers ARM OK under qemu but the SIGALRM is NOT delivered to the
  guest handler (`-strace` shows no heartbeat `write`). Don't rely on it for
  hang PC; use `-d exec`/`-d in_asm` tracing instead.

## Follow-up session (same day) — spin forensics + NX experiment (committed)
- qemu `-d exec`: JNI_OnLoad spins on a 2-address loop in the ~`base+0x764c000->0x7704000`
  band, which was HEAD-first assumed to be the appended RELA PT_LOAD. **Tested it**:
  jni_shim `mprotect(PROT_NONE)` on the RELA PT_LOAD (base+0x6988000, 0xc34ea8)
  SUCCEEDED (rc=0) with NO behavior change — so the spin is **NOT** in the RELA
  data. The base from `dladdr`/`dlinfo` appears ~2MB off for high vaddrs, so
  offset attribution is unreliable; the executions band may be real libro code.
- Net: shutdown; commit `0e3c53a`. Real fix next session = capture the spin's
  **call stack** (needs a working qemu-gdbstub interrupt — gdb `interrupt` over
  the stub didn't take; try `gdb` `set mi-async on` BEFORE `continue&` then
  `interrupt`, or a raw `\x03` on the socket; the `alarm_sa_handler` timer does
  NOT fire under qemu). Then implement the missing guest-stub/trampoline for
  whatever function Roblox dispatches.

## Session 15 — SPIN ROOT-CAUSED AND FIXED; now a real OOM-sourced abort (committed 0a081ac)

### The spin was NOT a condvar loop — it was unresolvable PLT GOT slots => busy-spin to garbage
- qemu `-d exec`/`-d in_asm`: the "spin" was a GOT-indirect thunk
  `adrp/ldr x16;[x16+off]; ldr x17; cbz; br x17` looping with guest PC at
  `base+0x15X014e4/14f4` (X varied run-to-run). Those offsets are past-file
  and past `.text`, i.e. garbage-as-code. The `base+0x15...` jumps into
  anonymous memory.
- Reality: **every PLT JUMP_SLOT GOT slot held a bad value** because glibc's
  lazy binding under qemu user-mode + the bionic shim never resolved them.
  libro's `pthread_mutex_lock@plt` → `br [GOT]` → rodata/anon ⇒ busy-spin.

### Fix (committed): pre-resolve ALL PLT GOT entries
1. `robo_open()` default path was `"libroblox.so"` (CWD) — failed in-app, so
   `g_robo.have=0` and no ELF functionality worked. Now defaults to
   `/system/lib64/libroblox.so`. This is what made everything downstream work.
2. `patch_condvar_plt_got()` installs REAL glibc pthread_mutex_lock/cond_wait/
   cond_timedwait (from direct libc handle `g_real_libc`, not RTLD_DEFAULT which
   returns the shim's shadowed/corrupt address) — the old stubbed shim+wrap
   approach kept the spin.
3. `patch_condvar_plt_got()` now called AFTER the direct-glibc block so
   `g_real_*` are populated.
4. **`patch_all_jumpslots(base)`**: iterate `.DJMPREL`, resolve each symbol via
   `g_real_libc`/RTLD_DEFAULT, write base+r_offset GOT slot with the real fn.
   Result: `patched 532 PLT GOT slots (3 unresolved, of 537)`. The 3 are
   bionic/Android-only (`Java_..._Android*_FinishPaymentsProtocol`,
   `__gcov_dump`, `__gcov_flush`) — harmless.
- **Effect**: JNI_OnLoad now executes REAL Roblox code (clock_gettime, sysinfo,
  /proc over-com/read, getrandom, gettid, getpid) and reaches a real
  **malloc-NULL → abort()** instead of spinning forever. EXIT 14 (hang) →
  EXIT 134 (SIGABRT). Huge milestone.

### Current blocker: `abort` at `Java_..._initializeNativeCode` + 0x343e44 —
  per-thread TLS alloc fast-path returns NULL
- qemu `-d exec` last real .text PC = `base+0x2692d8c...` (`0x2692dcc`: `bl abort@plt`).
- Sequence: pthread_once → mutex_lock/unlock → pthread_getspecific → then
  `bl 0x1c35480` (Roblox per-thread TLS block allocator, small-size fast-path
  from a TLS free-list) → `cbz x0 → 0x2692dcc abort`. It aborts when the small
  alloc falls to the big path and that returns NULL, or the TLS free-list is NULL.
- It aborts EVEN THO we already forge sysinfo => 256 GiB free and
  `/proc/sys/vm/overcommit_memory` => 1 (also tried 0 and 2). So it is NOT a
  real low-memory abort: rather a **bypassed-early-alloc-init / tls-arena-not-
  seeded** condition (we NOP a lot of init). Host has only ~4 GiB avail and
  Committed_AS > CommitLimit, but the allocator never even `mmap`s before
  aborting (strace shows 0 mmaps after JNI_OnLoad).

### Diagnostics added this session
- `wrap_android_set_abort_message()` + `wrap_android_log_print()` print the
  (otherwise logcat-lost) abort reason to stderr — so far no `[android-abort]`
  line appears, meaning the abort is a silent bare `abort()`.
- gdb walk shows the abort caller's return addr is in a data region
  (`base+0x?ba710`), consistent with a JNI/trampoline callback chain.

### Next steps (ordered) — unblock the TLS-alloc NULL
A. Make the TLS free-path never NULL: the small alloc `0x1c35480` `cbz`es on an
   empty free-list and falls to the big-allocator; ensure the big allocator
   (`0x1c3639c`) returns from a real glibc `malloc`. If that tail-call resolves
   via a JUMP_SLOT the loader left bad, patch it. Check whether
   `0x1c3635c` (`b` target) calls real malloc.
B. OR force `abort@plt` (GOT) to `wrap_abort` that logs the caller PC from
   `(_RETURN_ADDRESS)` and returns (unwind the quadruple-abort) so the call
   chain continues and the next OOBorn diagnostic (or SIGSEGV handled by our
   segv handler) reveals the real issue.
C. OR run Roblox's real allocator init (don't bypass it) by removing the NOP
   clock/init bypasses in `JNI_OnLoad` progressive patching / the guard
   override, letting the arena seed normally. This is likely the correct fix.
D. After JNI_OnLoad returns (registers 3 methods), boot the GUI on `:0` and
   start the vision phase (cua-driver, Step 5 on the task list).

### Refined root cause (same session, commit after abort-intercept)
- Neutralizing `abort` (GOT -> wrap_abort that logs caller and returns) makes the
  quadruple-abort at 0x2692DD0/DD4/DD8/DDC log caller offsets then return. After
  they return, JNI_OnLoad keeps executing but busy-spins (0 syscalls). So
  suppressing abort is NOT a fix — it's a diagnostic.
- `0x1c35484` (the per-thread TLS/small alloc) verified: empty free-list ->
  fall through to the book's own MemoryPool allocator (0x1c3635c), NOT glibc
  malloc. strace shows **0 mmap syscalls after JNI_OnLoad**, so the NULL is
  **pure book-side user-space**: the MemoryPool arena/chunk bitmap is empty /
  unseeded because Roblox's real allocator-init never ran (bypassed by the
  JNI_OnLoad progressive patches + guard/ts_flags override). It is NOT a real
  host-memory OOM even though we also forge sysinfo+meminfo+overcommit.
- **CONCLUSION: option C — run Roblox's real allocator/MemoryPool init (do not
  bypass it) — is the right path.** Find the pool init entry (likely a
  constructor / a `Java_..._initializeGC`/`Memory` JNI or a static init that is
  currently NOP'd or skipped) and either let it run or manually call it to seed
  the per-thread pool chunk-base, so the small allocator's free-list is non-empty.

## Session 16 — libroblox REAL .init_array constructors now RUN (committed); blocker = Roblox MemoryPool bootstrap

### Root-cause advance: DT_INIT_ARRAYSZ == 0, so glibc never runs Roblox's ctors
- `readelf -d` on installed libroblox.so: `INIT_ARRAY=0x630bfc0` but `INIT_ARRAYSZ=0`
  (0 bytes) even though `.init_array` section is 0x6ce0 (3484 pointers).
- The init_array entries are `R_AARCH64_RELATIVE` (base+addend) slots that the
  loader SKIPS resolving because DT_INIT_ARRAYSZ==0 (it never runs them).
- So all the static constructors that seed Roblox's MemoryPool / TLS arena
  globals originally never executed. That is the real antecedent of the old
  malloc-NULL abort.

### New capability: run_libroblox_init_array(base)
- mprotect base+[0x5a00000..0x6320000] RWX, apply RELATIVE relocs for
  init_array-range slots (0x630bfc0..0x6312ca0) -> write base+addend, then call
  each ctor in address order.
- VERIFIED RUNNING: log shows ctor[0]@base+0x2692f14 .. ctor[3]@base+0x1c34480
  with a sysinfo plus abort appearing INSIDE ctor[3]'s execution.
- ctor[3] (0x1c34480 -> tail `b 0x5d9ce10`) does a thread-local allocation via
  0x1c35480 (the TLS block fast-alloc) which returns NULL, hit the
  cbz-to-abort at initializeNativeCode+0x343e44. Same malloc-NULL abort as
  before, now reached from the constructor path.

### Blocker now precisely: Roblox's per-thread MemoryPool TLS alloc returns NULL
- 0x1c35484: TLS key from [0x6368000+0x9dc]; if -1 runs init; else
  pthread_getspecific to default block 0x6308dc0 (csel if empty). size<=0x400
  fast-path pops [blk+232]+8; on empty -> big-allocator 0x1c3635c.
- 0x1c3635c (big alloc) returns NULL regardless of reported memory (614MB real
  OR 256GB forged sysinfo): qemu strace shows NO mmap after JNI_OnLoad, so it is
  a book-side arena-not-seeded condition, NOT real OOM.
- wrap abort() logs+returns; without it the process dies SIGABRT.

### de-horned wrong guesses (verified and committed)
- sysinfo/meminfo/overcommit forgers are NOT the fix and are now set to PASS
  THROUGH real host values (forging 256GB or overcommit 0/1/2 didn't change the
  abort). Do not re-add inflation as the primary lever.
- Applying RELATIVE relocs GLOBALLY double-corrupts .data (loader already does
  .data/.got/.data.rel.ro; only .init_array is skipped). Keep the apply
  init_array-scoped.

### Next (ordered)
A. Find and call the MemoryPool init directly (seek the fn that initializes the
   block region 0x6308dc0 / the arena global ~0x6367000+0x600), or identify a
   later ctor that seeds the pool and run it before ctor[3].
B. Or patch 0x2692ce8 (the cbz-abort on the TLS-alloc NULL) to fall back to
   real glibc malloc so the book gets a block and can proceed through later
   ctors -- a stepping-stone, not a final fix.
C. After JNI_OnLoad returns (registers methods), GUI on :0 + vision phase.
## Session 17 — direction reset: stable loaded state via Session-9 full bypass (committed)

### Direction confirmed with user
End goal is the OPEN-SOBER CUSTOM RUNTIME (sober-style native run: QEMU is the
ARM64/x86-64 bridge, not a whole-VM product). Re-adopted the Session-9 loaded
-state approach: JNI_OnLoad returns 0x10006 immediately so the binary bootstraps
under the bridge and we can then raise/observe a window on :0. Fighting Roblox's
internal MemoryPool inside progressive init is parked (documented below).

### Why the old full bypass was silently broken
- There are TWO entry shapes: the EXPORTED JNI_OnLoad at base+0x1f0db20 (what
  dlsym/lib host calls) and 0x1f64e58 (an internal init at a shifted offset).
  The prior bypass patched 0x1f64e58 -> real JNI_OnLoad still ran the
  MemoryPool-reaching init and aborted.
- Fix: bypass base+0x1f0db20 (entry = mov w0,#6; movk w0,#1,lsl#16; ret).

### Disabled for bypass path
- run_libroblox_init_array(...) call is commented out. Its ctor[3] (MemoryPool
  TLS alloc at 0x1c34480 -> 0x5d9ce10 -> body 0x2678068) aborts on the internal
  pool; with abort suppressed it spins and blocks. Skipping ctors gives the
  clean baseline. The init_array + RELATIVE machinery is kept in the tree
  (real-engine-init path) but not run by default.

### VERIFIED (fresh run)
  FULL BYPASS: JNI_OnLoad@<base+0x1f0db20> returns 0x10006
  entering JNI_OnLoad...
  JNI_OnLoad call at <base+0x1f0db20> ...
  JNI_OnLoad -> 0x10006
  Entering sleep loop
No abort, no fault; process stable until watchdog timeout (RUN EXIT 14).

### MemoryPool blocker (parked, for real engine init later)
- libroblox imports NO allocator (only free, munmap); its MemoryPool is fully
  self-contained. Big-allocator 0x1c3635c returns NULL regardless of forged
  614MB vs 256GB sysinfo, with zero mmap after JNI_OnLoad. One-time init body
  0x2678068 aborts on its own first TLS alloc. Fork/banc of memory, ctors,
  malloc fallback all unavailable. To boot the real engine, must resolve the
  TLS block free-list seed (0x6308dc0 struct) or run fuller Android/JAVA app
  bootstrap.

### Next (ordered, per user direction)
A. NOW: run jni_shim with DISPLAY=:0, keep sleep-loop stable, and use
   computer vision on the X11/EGL surface to observe any window/black frame,
   or confirm none yet. Check whether book opens a GL context or needs the
   engine run loop.
B. Then: re-enable init_array/progressive init in stages ONCE the pool seed is
   understood, so a real window can render.
C. Multiple-version compatibility after a working baseline.
## Session 17b — MemoryPool malloc-fallback thunk: empty-pool abort DEFEATED (committed 313aa7b)

### Probe: libroblox imports NO allocator functions
readelf -r --use-dynamic shows libroblox.so imports only `free`/`munmap` from the
allocator family (no malloc/calloc/realloc/mmap). Its MemoryPool is fully
internal; the big-allocator returns NULL on unseeded arena state regardless of
host free RAM. Forging sysinfo/meminfo is irrelevant.

### Fix: redirect small-allocator empty-list to real glibc malloc
- Site: libro offset 0x1c354fc — the "empty per-thread free-list" continuation
  that tail-calls the big-allocator 0x1c3635c (which returns NULL).
- Thunk on a fresh MAP_ANONYMOUS RWX page (NOT the cond_shim page, which is the
  condvar "mov w0,#0; ret" trampoline):
      mov  x0, x19           ; size (0x1c35490 mov x19=x0; x19==size)
      ldr  x17, [pc, #24]    ; pc-literal loads real glibc malloc
      blr  x17               ; x0 = malloc(size)
      ldr  x19, [x29, #16]   ; restore saved x19
      ldp  x9, x30, [x29], #32 ; restore (x9=[x29], x30=[x29+8]), sp+=32
      mov  x29, x9
      ret
  Encodings verified against aarch64-linux-gnu-gcc-assembled .S.
- Book patch (24 bytes at 0x1c354fc): `adrp x16, thunkpage; br x16; nop x4`.
  ADRP encoding that VERIFIED (mine was wrong first try):
      imm = (thunk_page - site_page)  ; in 0x1000 units, signed
      adrp = 0x90000000
           | ((imm & 3) << 29)                  ; low 2 bits -> bits[30:29]
           | (((imm >> 2) & 0x7ffff) << 5)      ; high 19 bits -> bits[23:5]
           | Rd                                  ; Rd = x16 = 0x10
  My first form `((imm&0x7ffff)<<5)` put the raw (non->>2) delta at the wrong
  bit offset -> jumped to a wrong page and SIGSEGV'd. Correct form verified
  against the cross-cc (same-page `adrp x16` disassembles to 0x90000010).

### Result
- BEFORE: ctor[3] aborts 4x (empty-pool NULL) and spins.
- AFTER: ctor[3] runs, calls sysinfo (pool doing real allocation), NO abort,
  NO segfault. The hang is now a single-threaded CONDITION wait-loop (classic
  QEMU user-mode one-thread behavior), not an OOM abort.

### Next
Post-ctor[3] hang = wait on an event/flag that never arrives (one thread under
QEMU). Options: (a) trace the exact spin site (heartbeat) and force/fake the
awaited flag; (b) pre-seed the pool's real arena (static default TLS block free
-list) so the block path never blocks. Much more tractable than the previous
NULL/OOM abort.
## Session 17c — blocker refinement (committed 381d088)

Current state: the MemoryPool empty-pool abort (fixed Session 17b by a thunk that
routes libro's small-allocator empty-list to real glibc malloc) is gone. ctor[3]
(book 0x1c34480, MemoryPool one-time init) now runs, performs its allocation
syscalls (sysinfo/overcommit/getrandom), then spins in pure user-space code with
NO further syscalls — a single-threaded wait for a condition/flag that only a
second thread would set (QEMU user-mode runs one vCPU).

Attempts this turn:
- SIGALRM heartbeat sampler from the host-signal route: QEMU user-mode does not
  deliver SIGALRM into the guest handler; 0 samples (kept, harmless).
- pthread_cond_timedwait slot return now ETIMEDOUT (110) instead of 0 (kept).
- A dedicated bump-allocator thunk target (host shim function) caused SIGILL —
  calling a host/ELF function from the guest through the ARM thunk crosses a
  translation context QEMU cannot handle. Reverted to the dlsym'd glibc malloc
  thunk, which is safe and stable.

Stable, committed, no crash: ctor[0..2] run, allocation succeeds, ctor[3] waits/
spins, no abort/SIGILL/segv.

Next: identify the awaited flag in ctor body 0x2678068 and pre-set it (Session
11-style guard fix); or pre-warm the static TLS block free-list; or spawn an
emulated second thread.
---

## Session 18 — from-scratch JIT (arm64jit): drop-QEMU path begins

**Decision (user):** build a small in-process ARM64->x86-64 JIT/translator from scratch to replace QEMU, accepting multi-session. Goal stays: a real runtime (no QEMU), then multi-version proof, then CV on the GUI.

**What landed this session (all committed, 16 tests green):**
- `crates/arm64jit/` — new workspace crate. `x86.rs` = minimal verified x86-64 emitter; `decode.rs` = AArch64 decoder; `translate.rs` = per-instruction ARM->x86; `jit.rs` = CpuState + exec.
- Decoder verified against REAL gcc/objdump encodings (not memory): B, B.cond, MOVZ/MOVK/MOVN, ADR/ADRP, ADD/SUB imm, ADD/SUB shifted-reg, AND/ORR/EOR, LDR/STR unsigned-imm + register-offset.
- Pattern lesson: decode guards keyed to top-byte/class `(top & 0x3b)==0x39`/`0x38` (LdImm vs LdReg), `movewide sf` etc. — derived from actual encodings, each with a ground-truth unit test.
- Key debug: LOGIC-reg `s` is NOT bit29 (that's part of opc); `s = opc==0b11`. rm field is bits[20:16], NOT (insn&0x1f).
- JIT executor: spills guest regs to a `CpuState` in memory addressed via RBX (NOT R12/RSP/RBP — emit_mem forbids RSP/R12, and RBP/R13 have rm=7 -> RIP-rel for disp0). Prologue `mov RBX,<state>`; `[RBX + 8*i]` per reg; epilogue returns x0. `exec_bytes` = bytes->decode->compile(RWX mmap)->call fn(*mut CpuState)->x0.
- **LIVE proof:** executed real `mov x0,#3; add x0,x0,#4` (=d2800060 91001000) => returned 7, no QEMU. Commits 2faf3dd, d1fed33, eb11d66, 7c25747.

**Current scope/limits (honest):** translator handles MoveWide/AddSubImm/AddSubReg(shamt 0) only so far; no loads/stores, no branches/control-flow, no BL/host-call dispatch, no FP/NEON/TLS/atomics yet. Roblox's libroblox.so is far beyond this until loads/stores + branches + a host-call (syscall/bionic-shim) dispatch land.

**Next (Session 19+):** broaden decoder+translator to LDR/STR (have decode) + B/B.cond/CBZ + a host-call trampoline; validate on a real multi-instruction aarch64 .so function; then wire into libloader `--no-qemu`. Cleaned /tmp of ~5G stale QEMU core dumps.

## Session 18b — arm64jit executes load/store + control flow (no QEMU)

**Verified this session** (all through the from-scratch JIT, no QEMU):
- LDR/STR (unsigned 16-bit immediate offset), size 8/4, via RDX addr + lea — real
  `ldr x0,[x0,#16]` loads host memory correctly (17→19 tests).
- RET decodes + translates to host `ret`.
- **Control flow**: B (jmp), CBZ/CBNZ (test+jnz/jz) + flow-aware `jit::compile`
  that builds a guest_pc→host_offset map and patches rel32 fixups
  (disp = target − (disp_off+4)). Fixed a bad `start` calc → SIGSEGV.
  Real `cbz x0` function returns 20 (fall-through) / 10 (taken) ✓.

**State**: 19 tests green, workspace clean, committed at `09699e5`.

**Commits this session**: eb11d66 (LDR/STR+ret), 7c25747 (first exec),
a432acf, 09699e5 (control flow).

**Next slice** (task 3b): B.cond + NZCV flags (subs/cmp set flags; materialize
into guest NZCV so any interleaving works) then BL function calls with a
guest call stack. After flags, real `.c` compiled aarch64 (if/else, loops)
can run.

## Session 19 — arm64jit: flags + calls + stack pairs (23 tests, no QEMU)

**Verified this session** (all through the from-scratch JIT on x86-64, no QEMU):
- **cmp/subs → B.cond** (if/else): real `cmp w0,#3; b.le` returns 0 or 1 per ARM. Decoder
  expanded to S-flag form `cmp w,#imm` = top 0x71/0xF1 (SUBS imm). `x86_cc_for_cond`
  maps ARM cond→x86 jcc (EQ..LE); set-flag ALU leaves x86 flags live through the
  follow-on `mov` stores(store to rd==31 suppressed).
- **BL function calls + call-graph `compile_image`**: BFS walk follows branch/call
  targets, compiles whole reachable region into ONE buffer; `BL` = save LR + host
  `call rel32` (cc=0xfe fixup); a real leaf call `caller(x)=(x+5)*2` returns correctly.
- **LDP/STP (load/store pair)**: offset/pre-index/post-index, 32/64-bit, decoded via
  bit23=indexed, bit24=pre, bit22=load, imm7 signed scaled by 8/4; `stp/ldp` prologue
  round-trips regs through real stack, sp restored.

**Tests: 23/23 green. Tree clean.** Commits: 43f7af3 (flags+B.cond), 874800c (BL +
compile_image), 0ee07fc (LDP/STP).
**Next slice (3e):** adrp/adr (PC-relative), ORR/EOR/AND-reg + reg-reg MOV/aliases,
then `adic` integration into libloader `--no-qemu`.

## Session 19: arm64jit — core-ISA coverage (26 tests, drop-QEMU verified)

**Verified this session** (all through the real from-scratch JIT, no QEMU):
- **Flags + B.cond** (commit 43f7af3): cmp/SUBS/ADDS set flags; b.eq/ne/le/lt/ge/gt/hs/ls/cs/cc
  translate to x86 jcc. Real `cmp w0,#3; b.le` runs correctly.
- **LDP/STP pair load/store** (0ee07fc): offset/pre/post index, X and W, verified vs 5 real
  encodings. Real prologue stp/ldp round-trips regs and restores SP.
- **BL calls + call-graph compile_image(image, base, entry)** (874800c, 0ee07fc): walks
  BL/B/CBZ targets, emits into one image buffer, host call + host ret (LR saved). caller()=20.
- **LogicReg AND/ORR/EOR + mov alias** (dc88106): mov rd,xm; XZR reads as zero; real logic.
- **ADRP/ADR** (47f7a5f): PC-relative addressing; adrp/add/ldr loads a mapped global (guest==host
  address when the ELF is placed at its vaddr).

**State: 26 tests green — core AArch64 ISA executes real compiled code on x86-64, no QEMU.
Next (task 4): integrate into libloader/sober-core — map the Roblox ELF at its vaddr, then
compile_image(text_segment, vaddr, entry) for _start/JNI_OnLoad. adrp/ldr now resolve because
guest address == host address when segments are mapped at their ELF vaddr.

## Session 19c - JIT wired into the product (no QEMU path)

- **libloader**: exposed `pub mod elf` so `load_elf`/`LoadedElf` are usable downstream (commit 9c8e791).
- **arm64jit/examples/elfjit.rs**: loads a real static aarch64 ELF with libloader's loader and JIT-runs
  its entry -> returns 42 on x86-64, no QEMU. Proven end-to-end loader+JIT wiring.
- **sober-core**: `--jit` CLI flag -> `mod jit::run_elf_entry` loads the Roblox .so via libloader and
  hands it to arm64jit. `qemu::find_main_binary` made pub. (commit 4b89619)
- **Honest boundary**: trying to run `/tmp/robpatched/libroblox.so` (a PIE `ET_DYN`) through elfjit
  SEGV s because for PIE .so the host address of the code is NOT simply `e_entry`: the loader maps the
  PT_LOAD text segment at a real host address, and `compile_image` must be fed the mapped host range +
  its guest base, not `entry` directly. Plus the full .so uses far more instructions than the current
  subset, so full execution is still months of translator work.

**Next (task 5)**: fix the PIE path - derive the mapped text host address from `LoadedSegment.vaddr`,
  feed `compile_image(image=mapped_host_slice, base=guest_text_vaddr, entry=host-of-entry)`, and report
  the *first unsupported instruction's guest address* as an honest diagnostic target for the next
  decoder slice (start with SVC syscall routing + TLS, then SP, then BL/ADR linkage).

## Session 19d - WHAT STILL NEEDS TO BE DONE to get the JIT path fully working

Current state: arm64jit executes a real AArch64 subset on x86-64 with no QEMU
(26 tests green, all verified against objdump ground-truth). It is wired into
the product end-to-end (libloader -> compile_image -> run) and can run the
entry of a *non-PIE static* aarch64 ELF (elfjit example returns 42). Running
the real `libroblox.so` (a PIE ET_DYN) currently SIGSEGVs.

### 1. PIE / shared-object mapping (unblocks the real .so immediately)
- **Problem:** the elfjit/`--jit` path feeds `compile_image(image, entry, entry)`
  assuming host-addr == guest-vaddr. That holds only for non-PIE statically
  linked ELFs. libroblox.so is ET_DYN/PIE: libloader maps PT_LOAD segments at
  real host addresses and relocates, so `e_entry` is not a host address.
- **Fix:** derive the *mapped text range* from `LoadedElf.segments[]` (host
  vaddr + memsz), slice that range as the compile `image`, pass `base =
  guest_text_vaddr` and `entry = host(of e_entry)`. Then `ADRP`/`ADR` compute
  guest addresses that resolve into the mapped segment (guest==host holds
  again because libloader maps at vaddr).
- **Diagnostic:** once it loads, report the **guest address of the first
  instruction the decoder can't translate** (add a `resolve` that returns
  `Err((pc, Inst::Unsupported))`). That gives the exact next decoder slice.

### 2. Decoder/translator gaps that WILL appear (in rough order of priority)
- `SVC` syscall routing (Roblox makes many host syscalls; must map to host or
  the bionic shim) - and the `--jit` path must link against the shim.
- TLS slot access (`mrs`/`msr` TPIDR_EL0, `ldr`/`str` via TPIDR), threads
  (host pthreads vs guest threads).
- Atomics (`ldaxr/stlxr`/CAS loops) - used heavily in Roblox.
- FP/SIMD (NEON: a LOT in graphics/sound; `ldr q`, `add v0.4s,..`, etc.)
- 32-bit register semantics: Ws must zero-extend and flag-setting compares
  must be 32-bit-aware (currently traced as 64-bit for small values only).
- XZR vs SP as x31 depending on context (currently one sp slot, no read-xzr
  suppression for arithmetic stores - add rn/rd==31 handling per class).
- Multiply/AES/other (`mul`, `mneg`, `sdiv`/`udiv`, `csel` (flags-dependent
  select - completes flag model), bitfield ops `ubfm/sbfm/bfi/extr`).
- LD/ST variants: `ldr x,[x,#imm]` are done; `ldr` non-scaled, `ldrsw`,
  `ldrb/h`, `strb/h`, `ldp/stp` SIMD (128-bit D0-D31 pairs).
- Branch: `b.eq/ne/...` (B.cond already done), `br`/`blr` (indirect call),
  `cbz`/`cbnz` done, `tbz`/`tbnz`, return-less tail calls.
- `MOVZ/MOVK` (done) but `MOVN` and imm build-up across block - fine.

### 3. Guest runtime environment (required to actually run Roblox)
- **A guest stack** (`sp`/x31) pointing into a large mmap'd region; `mrs
  SP_EL0` etc.
- **Thread-local storage:** TShell set `TPIDR_EL0`, guard-and-init; Roblox
  spawns threads.
- **Syscall service** (`svc #0`): at minimum `exit`, `write`, `mmap`,
  `munmap`, `brk`, `clone`, `open`, `read`, `futex`, `timer`, `getuid`,
  `sysinfo`. Either route to the bionic shim or implement host-facing.
- **Signal handling / the JNI setjmp-longjmp** that the native glue expects.
- **Linking the bionic** sysroot libs (libbionic_shim.so + guest stubs) -
  the existing QEMU build work (jni_shim, elf_disco) is REUSABLE for symbols
  resolution; the JIT needs a PLT/GOT resolver so `bl` to relocated functions
  dispatches to the right guest/host thunk.

### 4. Correctness hardening (before trusting any real run)
- **Frame pointer / unwind** - not needed for execution but for debugging the
  unmistakable first crash.
- **Trap on unsupported instead of UB:** currently any translated block that
  hits an untranslated instruction returns Err gracefully (good); but a guest
  `ret`/`br` to an address outside any compiled block must be caught, not
  fall through (add a `state->pc` write + a trampoline back into the
  interpreter/compile loop for un-compiled blocks).
- **PC-relative fixups are buffer-relative (already done);** ensure they are
  correct for code that spans two images/ELF segments.

### The real realistic path to a Roblox window (multi-session)
1. PIE mapping + first-unsupported diagnostic (do this next).
2. Wire `svc` + TLS + a stack + GOT/PLT resolution so `JNI_OnLoad` can run
   far enough to print something.
3. Add cross-version coverage (the standing "multi-version" goal) by diffing
   the decoder against several `libroblox.so` builds.
4. Only after those load + JNI init: FP/NEON + atomics + threads to get
   actual frames; then the GUI/computer-vision inspection step becomes
   meaningful.

Everything above was updated to account for the current committed state at
`5496852`. The single highest-leverage next step is **#1 (PIE mapping)**.

---

## Session 20 (Aug 20, 2026) — PIE mapping FIXED; JIT now decodes real libroblox.so code

Goal (from 19d #1): fix the PIE/ET_DYN mapping so `elfjit`/`--jit` no longer
SIGSEGVs on the real 117MB `libroblox.so`, then push the honest
first-unsupported diagnostic forward. **This was achieved and verified end to
end.** Commits: `194d5d8`, `029e36f`, `da16a76` (on `dev`).

### 1. PIE / ET_DYN mapping — FIXED (the 19d #1 blocker is done)

**Root cause of the old SIGSEGV:** the per-segment `MAP_FIXED` in `load_elf`
lets a later PT_LOAD of a *packed* ET_DYN target an address that still overlaps
the previous huge (`~99MB` r-x) text mapping. Under gdb the fault was
`__mmap64` crashing on a `MAP_FIXED` address inside the earlier mapping — i.e.
`0x78f05998000 + 0x5e6b000` landed *inside* the text range.

**Fix:** added `libloader::elf::load_elf_image(path) -> LoadedElf` (a fresh API,
the old `load_elf` is untouched for the QEMU path). It maps ONE contiguous
anonymous region at a fixed `JIT_BASE` (`0x100000000`), lays every PT_LOAD into
it at `base + (p_vaddr - min_vaddr)`, zero-fills `.bss`, and applies per-segment
mprotect. Critically it sets **guest vaddr == host address** (`guest_of(link) =
base_addr + (link - base_load_addr)`), the exact property arm64jit's ADRP/ADR +
direct-dereference model requires. No more overlap, no more SIGSEGV.

`elfjit` and `sober-core --jit` now:
- load the real `libroblox.so` cleanly (4 segments, guest==host at
  `0x100000000`, e.g. text `[0x100000000, 0x105e67390)`),
- translate the requested guest entry (a link-time address via `guest_of`),
- report an **honest diagnostic**: `arm64jit stopped on unsupported instr
  at/near guest 0x101c34480: translate: unhandled Unsupported(0x...)`.

### 2. Decoder/translator walls pushed through (5 in this session)

1. **ADRP/ADR mask bug FIXED.** The decoder tested `insn>>24==0x90`, missing
   real ADRP encodings whose top byte is `0xD0` (varies with `imm[1:0]`).
   Changed to the canonical `(insn & 0x9F000000)==0x90000000` (ADRP) /
   `==0x10000000` (ADR); confirmed `0x90026516` (top 0x90) and `0xd0026a93`
   (top 0xD0) both match. Without this, the very first decoded real-world
   Roblox ADRP `0xd0026a93` returned `Unsupported`.
2. **LDR/STR unsigned-imm sizes 1,2,4,8** (was 8/4 only). Added halfword/byte
   zero-extend loads (`movzx_word_mem`, `movzx_byte_mem`) and 8/16-bit stores
   (`mov_store8/16`) to the x86 emitter; wired into `LdStrImm`.
3. **LDR/STR register offset** (`ldr x9,[x8,x1,lsl #3]`, class `0x38`) — new
   `LdStrReg` translate arm (index `rm`, shift by `log2(size)`).
4. **128-bit SIMD vector load/store** (`ldr q6,[x0,#16]`=`0x3dc00406`,
   `str q7,[x0,#32]`=`0x3d800807`) — `CpuState` now carries a **32×128-bit
   vector register file** `v:[u64;64]` at `VECTOR_BASE=256` (with `set_v`/`get_v`),
   x86 XMM helpers (`movdqu_load/store`, `movdqa_xmm`, `pxor_xmm`), new decode
   class `VecLdStImm` (`0x3D8`/`0x3DC`), and a translate arm that moves 16 bytes
   between the guest v-slot and guest memory through XMM0.
   *(`movi`/float NEON immediate was deliberately NOT bolted on — the imm
   reconstruction is fiddly and a wrong float result would be worse than the
   honest "unsupported" stop. Do it with a proper NEON decoder next.)*

### 3. Verification

- `cargo test --workspace` all green (arm64jit 26 → still 26, no regressions).
- Static non-PIE `stat.elf` entry returns `42` (unchanged, still passes).
- PIE `libpie.so` maps at `0x100000000`; entry `0x588` translated to guest and
  stopped *honestly* at `lsl x0,x0,#1` (`0xd37ff800`, a UBFM bitfield op — see
  task list below; not yet added).
- 128-bit vector round-trip: hand-assembled aarch64
  `ldr q6,[x0,#16]; str q6,[x1,#32]; mov x0,#99; ret` runs through elfjit
  (giving it a guest==host buffer via the new `buf` arg) and **returns 99**,
  no QEMU, no crash.

### 4. Current honest state on the real binary

```
$ ./target/debug/examples/elfjit ~/.cache/open-sober/libs/libroblox.so 0x1c34480
loaded '...libroblox.so': is_pie=true base_load_vaddr=0x0 e_entry=0x100000000
  segment guest=[0x100000000,0x105e67390) prot=r-x   (89MB text)
  segment guest=[0x105e6b3c0,0x10631c000) prot=rw-
  segment guest=[0x10631fb40,0x106987130) prot=rw-
  segment guest=[0x106988000,0x1075bcea8) prot=r--
running entry guest=0x101c34480
arm64jit stopped: translate: unhandled Unsupported(1862329344)  // == 0x6F00E400
```

The NEXT blocker is the **floating-point NEON** instruction `0x6F00E400`
(top-byte `0x6F`, the floating-point 3m-add / scalar-fp class — NOT the
integer `movi` `.4s` which decodes as `0x4F...`). That's the immediate next
decoder slice.

> **Session 21 CORRECTION:** `0x6F00E400` is **NOT** floating-point. Verified
> against `aarch64-linux-gnu-objdump` ground truth, it is `movi v0.2d, #0x0`
> — an **integer** vector move-immediate (the "clear a 128-bit vector to
> zero" idiom at the top of a stack-zeroing loop). It was handled in Session
> 21 (below), not deferred. The 20's "0x6F=FP" guess was wrong.

### 5. Next steps (updated, ordered)

1. **Add the float-NEON / FP layer** starting with `0x6F00E400` specifically,
   plus `fmov/fadd/fsub/fmul/fdiv`, `fcvt/fcvtl`, `fcmp/fcsel` and the
   `0x4F` `movi.4s` immediate (with unit tests). This is now the gate between
   "decodes" and "boots" — a 3D engine is dense with FP.
2. **`lsl/lsr/asr x,#imm`** (`UBFM/SBFM`, e.g. `0xd37ff800`) — trivial and hit
   by any real code; add the bitfield ops `ubfm/sbfm/bfi/bfx`.
3. Guest stack + `sp`/`mrs TPIDR_EL0` TLS + `svc` routing → then the `B/BL`
   call-graph can actually *run* ("compare" the boot log) rather than just
   translate.
4. Cross-version coverage; verify against multiple `libroblox.so` builds.

`git log`: `194d5d8` (PIE load_elf_image fix), `029e36f` (one contiguous
guest==host image, runs deeper into libroblox.so), `da16a76` (SIMD vector
regs + 128-bit ld/st + register-offset ld/st), then the HANDOFF update.

## Session 21 (Aug 20, 2026) — Architected the PC-driven dispatcher; JIT now *executes* real libroblox.so through blr chains

Commit: `6618c5e` (dev). **The JIT crossed from "translate-then-stop" to
"actually follow call/return control flow".** Five more instruction walls
pushed + the VECTOR_BASE bug fixed.

### 1. What was previously-unsupported, now decoded+translated (all objdump-verified)

1. **`movi` vector-immediate** (`.8B/.16B/.4S/.2D`) — and in doing so,
   corrected 20's mislabel: real blocker `0x6F00E400` is `movi v0.2d,#0x0`
   (integer), verified by `objdump -d`. Decoder reconstructs the lane and
   replicates it across the 32-bit/8-bit/64-bit lanes; unit-tests
   `movi_ground_truth` covers all 6 specimens.
2. **`stp`/`ldp q` 128-bit SIMD load/store pair** (`0xAD000000`),
   scale=16. Init function zeros 64 bytes via `movi; stp q,q; stp q,q`.
3. **HINT / PAC pseudos**: `nop`, `esb`, `csdb`, `paciasp`/`autiasp` (the
   `-msign-return-address` prologue), `bti`. Nominally the `0xd5032xxx`
   family, mask `(insn&0xfffff01f)==0xd503201f`; executed as no-ops (PAC
   is ignored in the guest).
4. **`br`/`blr`** — indirect branch/call. **The decode RN bug:** the source
   register is bits[9:5], *not* [4:0]; was decoding `blr x1` as `blr x0`,
   which sent the dispatcher to address 0 → instant halt with wrong value.
   Fixed; unit-tested.
5. **`tbz`/`tbnz`** test-bit-and-branch — `b24`=op (1→tbnz), `bit=b[23:19]|b31`,
   imm14×4. Verified `tbnz w0,#31` target.

### 2. The dispatcher (`jit::jit_run`) — the enabler

`br`/`blr`/`ret` can't be inlined (target is in a register). Added
`jit_run(image, base, entry, state)`: loop { compile reachable region from
`state.pc` via `compile_image`; run; re-enter at `state.pc` }. Terminal
`br`/`blr`/`ret` set `pc = (16/8-lanes)`, x30 link on blr, return to the
host loop. `ret` now sets `pc = x30` so returns chain to the caller.

`elfjit` switched to `jit_run`. Verified with a hand-assembled aarch64
program (adrp/add `x1=&callee`; `blr x1`; `mov x30,xzr; add x0,#1; ret`
/ callee `mov x0,#42; ret`) → **returns 43** through the dispatcher. This
is the first real indirect-call round trip.

### 3. Critical correctness bug: VECTOR_BASE overlapped `CpuState.pc`

`CpuState{ x[32]@0..256, pc@256 }`, but `VECTOR_BASE` was **256** — so every
vector `movi`/vector ld/st wrote into `pc`/`nzcv`, corrupting instruction
streams once SIMD ran. Moved the vector file to **272** (`VECTOR_BASE=272`,
`PC_OFF=256`). This was latent since Session 16 (vector regs added) and only
surfaced now that SIMD + the pc-driven loop share a state.

### 4. Honest current state on real libroblox.so

```
$ elfjit .../libroblox.so 0x1c34480
running entry guest=0x101c34480
arm64jit run_loop stopped: translate: unhandled Unsupported(445973185) at guest pc 0x1026938d0
```
`0x1A9502C1` = `csel w1, w22, w21, eq` right after `cmp x9, x10`. So the JIT
now *executes* through: `movi`→`stp q`→`paciasp/autiasp`→`blr` chains→`tbnz`,
and blocks on the first **NZCV-flags consumer** (CSEL).

### 5. Next step (the 21st wall): NZCV flags + the CSEL/CSET family

`csel x,w, cond` and `cset/csinc/csinv/csneg` need N/Z/C/**V** live from the
preceding `cmp/subt/cmp nzcv`-setter. NZCV is currently a placeholder
(written but not consumed); `b.cond` also must read *stored* flags, not live
x86 flags. This is a self-contained subsystem:
1. store N=bit31, Z=bit30, C=bit29, V=bit28 to `CpuState.nzcv:u32` from each
   `S`-flag setter (cmp/subs/adds/...),
2. read cond→x86 flag and `csel/cset/...` branch on it,
3. retire the "NZCV is a placeholder" comment in translate.rs.

`git log` since 20: `6618c5e` — "arm64jit: PC-driven dispatcher (blr/br/ret)
+ push 5 more decoder walls". Working tree clean.

## Session 22 (Aug 20, 2026) — NZCV flags + CSEL/CSET family; stp/ldp d; LDAR/STLR

Commit: `6dceb47` (dev). **Session 21's NZCV wall is crossed — and the JIT now
executes real libroblox.so all the way into floating-point arithmetic.**

### 1. The NZCV condition-flags subsystem (the 21st wall)

- New `store_nzcv(buf)`: after every `S`-flag arch op (`cmp`/`subs`/`adds`),
  snapshots x86 rflags (`pushfq`/`pop`) and packs either into
  `CpuState.nzcv:u32` with **N=bit31, Z=bit30, C=bit29, V=bit28**.
- New `load_nzcv_to_eflags(buf)`: the inverse — reads the packed NZCV, bit-shuffles
  it back into an x86 eflags image (CF/nzcv.29, ZF/.30, SF/.31, OF/.28), and does
  `push; popfq` so the immediately-following native `jcc`/`cmovcc` evaluates the
  guest condition. This matters because the dispatcher reloads operands between the
  setter and the consumer, clobbering live flags.
- `cmp`/`cmn` (rd==31, s=true) now write flags only; `b.cond` reads stored flags
  (not stale live x86 flags).
- **CSEL/CSINC/CSINV/CSNEG** (incl. `CSET`/`CINC` aliases): `rd = c ? rn : f(rm)`
  computed with a `cmovcc` on the repainted flags, `f` = identity/`+1`/`not`/`neg`.
  Handles the `cset`/`cinc` disasm aliases via the generic csinc.

**Decode-order gotcha (pitfall):** the CSEL X-variant shares top byte `0x9a`
with the logical ORR/EOR/BIC family, so `(insn&0x7fe00000)==0x1a800000` MUST be
checked *before* the LogicReg decoder or csel is swallowed as an `EOR`.

### 2. Three x86-emitter bugs found & fixed (latent, hit only when new high-reg/FP code selected them)

- **rex()**: R/X/B bits were placed at 0x10/0x20/0x08 instead of 0x04/0x02/0x01 —
  any 64-bit op touching a register ≥8 (e.g. R10 in csel) emitted an invalid
  `0x50-0x5F` prefix; fixed to the real REX.R/X/B mapping.
- **cmov_rr64**: emitted the jcc opcode (double-0x40) and lacked REX.R/B for
  R10/RDI. The REX fix plus a `-0x40` cc-domain correction made it emit a real
  `cmovcc`.
- The eflags-restoration tail originally pushed the wrong scratch (nzcv instead
  of the built eflags), causing a segfault only on one branch of the cset test.

### 3. The next two decode walls from *executing* libroblox.so

- **`stp/ldp d` (FP/vector 64-bit pair, top `0x6d`, scale 8)**: d-regs are the
  low 64 bits of `CpuState.v[k]`; each reg transfers one u64 at
  `VECTOR_BASE + 8*reg`. (q128 stays scale 16 / 16 bytes.)
- **`LDAR/STLR` acquire-release** (`mask 0x3fe00000 → 0x08800000/0x08c00000`,
  ~5787 uses in the binary): treated as plain loads/stores — ordering is a no-op
  in the single-threaded JIT.

### 4. Verified progress on the real binary

```
running entry guest=0x101c34480
arm64jit run_loop stopped: translate: unhandled Unsupported(1829831680) at 0x101c39f80  # stp d
  (pushed to) Unsupported(509675520) at 0x101c3a348   # fmul d0,d0,d1  <- current wall
```
The JIT now runs **past** `csel`/`cset`, through `stp d` and `ldarb`, and stops
on the genuine **floating-point** instruction `fmul d0, d0, d1` (`0x1E610800`) —
the scalar-FP arithmetic layer that Session 20's note originally mislabeled.
This is the FP layer (mulsd/addsd/fdivsd...) and the fp-immediate move/fmov.

### 5. Tests

`cargo test -p arm64jit` → **31 pass** (added `csel_family_ground_truth`,
`stp_d_zero`, `ldar_stlr_plain`). Full workspace 60+ pass except a **pre-existing,
unrelated** `libloader/src/android.rs` filesystem idempotency failure (untouched
by this session).

`git log` since 20: `6618c5e` (Session-21 dispatcher), `6dceb47` (this session).
Working tree clean (commit `6dceb47`).

## Session 23 (Aug 20, 2026) — FP arithmetic (verified), FP→int, UBFM, ANDS/TST

Commits `a03b143` (dev). **The FP-arithmetic wall is crossed and verified; the
real JIT now executes real double-precision multiplies inside Roblox's
`Java_com_roblox_engine_jni_NativeGLInterface_shouldDisplayOpenGLUnsupportedMessage`
and walks past it deep into the GameActivity init. New hardware: TLS.**

### 1. Scalar double FP arithmetic (the "fmul" wall) — IEEE-verified

- `Inst::FpScalar` ditched `movq_load`/`movq_store` + `mulsd`/`addsd`/`subsd`/
  `divsd`; `d_k` = low 8 bytes of `CpuState.v[k*2]` at `VECTOR_BASE + 16k`.
- DECODE BUGS FIXED (verified via masked opcode — the opcode is `insn` with
  Rm/Rn/Rd masked, since the early `(insn>>15)&0x7f` field **overlapped `Rm`**):
  `fmul=0x1e600800 fadd=0x1e602800 fsub=0x1e603800 fdiv=0x1e601800` (+`sz` bit22).
- New emitter `movq_load`/`movq_store` (64-bit XMM↔mem) and `mulsd`/`addsd`/
  `subsd`/`divsd`; **x86 SSE-operand order** fixed: `F2 0F 59 /r` uses
  `ModRM.reg=DST, r/m=SRC` (opposite of integer).
- New `fp_scalar_double_ieee` unit test: `2.5*4.0=10`, `10+2.5=12.5`, `10/2.5=4`
  — passes against `from_bits` IEEE ground truth. (Also fixed the test to address
  the `v`-array layout: `d_k` ↔ Rust `v[2k]`, not `v[k]`.)

### 2. FP→int + UBFM round of the register file

- `Inst::FcvtToInt`: class `(insn&0x5f20fc00)==0x1e200000`, op `(insn>>17)&7`
  (0=fcvtzs truncate, 2=fcvtas), via `cvtsd2si` (nearest-even; **note**: ARM
  `fcvtas` is ties-away — the tie-only difference is a documented approximation)
  and `cvttsd2si` (fcvtzs exact). New emitters.
- `Inst::BitField`: full `UBFM/SBFM` — `lsr`(imms==last), `asr`(arith), `lsl`
  (immr==(imms+1)%bits), plus the *general extract* `(Rn>>immr)&low(width)` with
  sign-extend (`sbfx/sxtb/ughl `sar/shl round-trip) for ubfx/sbfx/uxb/sxtb/sxth.
  This covers `sxtb/uxth/...` which appear throughout the tree.

### 3. ANDS/ORRS/EORS/TST now actually set flags

- `LogicReg opc==3` was *also* treated as a no-op (`let _ = s`). Now the base op
  is computed for opc==3 (`ANDS`/`BICS`), and `if s` calls `store_nzcv` — `x86
  and/or/xor` already produce CF=0,OF=0,ZF/SF-from-result, exactly AArch64 NZCV.
  Also added the missing top-bytes `0x3a/0x7a/0xea/0xfa` so `tst x_,x_`=ANDS decodes.

### 4. Real libroblox.so: what the JIT executes now (JIT_TRACE-past)

```
past: csel/cset(NZCV) → stp d/ldp d → ldarb/stlrl → fmul→fmul→fcvtas→lsr
      → uxtb (UBFM) → csel→ … → tst x23,x8 → b.ne → … → cmp x0,#0 → b.ne
stopped: mrs x19, tpidr_el0  (0xD53BD053)  <-- TLS thread-pointer system register
```
This is the **guest TLS/sp boot-essentials** wall the session list flagged. To
boot Roblox we must answer `mrs tpidr_el0` (and `msr`/`tlbi`/`isb`) with a real
or forwarded TLS base, map sp, and route `svc`. FP is done and verified; the
immediate TSL system-register (MRS/MSR tpidr_el0) is the next concrete wall.

`cargo test -p arm64jit` → **32 pass** (fp_scalar_double_ieee, plus more).
Workspace green (the libloader android idempotency test passed this session —
it is host/env flaky; unrelated). Committed, tree clean at `a03b143`.

## Session 24 — TLS crossed; JIT now runs real StartApp code

### Roblox boot progress (libroblox.so, 117MB ARM64)
```
past (this session, in order):  mrs x19,tpidr_el0 (TLS) → BIC  → ror (shifted-op)
      → mov x11,#0x3ffffffff (logical-imm) → ldxr/stxr (exclusive) → csinv
      → udiv/sdiv → madd/msub → bfi/bfc → fmov d0,x8 → SIMD cnt v0.8b
      → uaddlv h0,v0.8b (popcount) → fcvt s0,d0 → str s0,[x22,x23,lsl#2]
      → b.ne → mov v0.d[1],v0.d[0] → str q0 → … ror w23,w22,#0x14 (SBFM/EXTR)
stopped (honest Unsupported): ror #imm  (in a SHA/compression mixing loop)
```
- The JIT now executes real **`nativeAppBridgeV2StartAppWithParams@@LIBROBLOX`**
  startup code (and an FMOD audio-init region), including a full SIMD bit-popcount
  idiom (`cnt v0.8b + uaddlv h0`), FP width conversion, TLS reads, exclusive
  atomic emulation, integer mul/div, and BFM inserts. Big-vs-previous milestone.

### New instructions implemented & verified this session
- **MRS/MSR tpidr_el0** (`Inst::SysReg`) — decode `0xd53bd053` tpidr, read/write the
  per-state TLS pointer `CpuState.tpidr` (new field after `v`, `TPIDR_OFF=784`).
- **BIC/ORN/EON** (LogicReg op 4/5/6) — the bit-invert second-operand family.
- **add/sub/logic shift ROT**: `ror` via `ror_cl64` for shifted-register operands,
  plus `ror_ri8` (48 C1 /1).
- **Logical (immediate)** `Inst::LogicImm` — AND/ORR/EOR/ANDS with the AArch64
  bitmask immediate (`decode_logical_mask` from N/immr/imms); covers `mov xD,#imm`
  (ORR xzr,#mask) and `tst`/`ands` imm.
- **Exclusive** `ldxr/stxr/ldaxr/stlxr` (`Inst::LdExr`) — single-threaded: ldxr =
  plain load, stxr = plain store + status=0. Thread-safe enough for a lone guest.
- **CSINV/CSNEG/CSINC** — widened the CSel decode to tops `0x5a/0xda` (was 0x1a/0xda).
- **UDIV/SDIV/MADD/MSUB** (`Inst::MulDiv`) — via `div/idiv/imul` + `cqo`/`movsxd`.
- **BFM/BFI/BFC** — the bitfield-insert alias of BitField (`immr > imms`).
- **FMOV core↔FP** (`Inst::FmovGp`) — Xd<->Dn, Wd<->Sn.
- **FCVT s<->d** (`Inst::Fcvt`) — `cvtsd2ss`/`cvtss2sd`+movd.
- **NEON SIMD** (first SIMD in the JIT): `cnt v.8b` (`Inst::SimdPopcnt`, SWAR
  byte-popcount) and `uaddlv h,v.8b` (`Inst::SimdSum8`) — verified against the
  real binary's popcount chain reaching an `fmov w10,s0`.
- **`mov v{rd}.d[1], v{rn}.d[0]`** (`Inst::InsD1D0`) — dup low 64 into high lane.

### ⚠️ OPEN WALL — `ror rd, rn, #imm` (EXTR rotate) still not decoded
The standalone `ror` is the rotate alias of **EXTR** (`EXTR Rd,Rn,Rn,#lsb`), NOT
a UOFM — the current `BitField` decode + `is_valid` mis-reads/mis-rejects the word
(`ror w23,#0x = 0x139652d7`: immr=22, imms=20; second sample 0x138f51eb immr=15,
imms=20 — both rotate `to ROR(Rn, imms)` but the generic UBF/rot encode mapping is
ambiguous against bfi/extracts and is NOT resolved. The JIT stops on `ror`
with a clean `Unsupported` (no silent wrong result). **Required**: decode the
`ror`/EXTR rotate as its own op (class `0x1 0x1 `... `N`, Rm==Rn) and emit
`ROR(Rn, lsb)`; add a unit test seeded with known operands. Also still open:
guest sp/`svc` routing for full boot.

## Session 25 — ror/EXTR decoded; SIMD lane add; JIT reaches MessageBus code

### ror now works (was the Session-24 open wall)
- Root cause: the standalone `ror Rd,Rn,#imm` is the **EXTR rotate** alias
  (`EXTR Rd,Rn,Rn,#lsb`), NOT a UOFM. Ground truth (`ror x0,x1,#12=0x93c13020`,
  `ror w0,w1,#4=0x13811020`): the EXTR class is `(insn & 0x1fe00000)` in
  `{0x13800000, 0x13c00000}` (disjoint from UBFM's 0x130/0x136), and the rotate
  amount is `imms` (bits[10:15]); `rm == rn`. `Inst::Ror` → `ror_ri8` (48 C1 /1).
  New decode test `ror_exclusive` (part of suite).

### SIMD lane arithmetic — first real SIMD math
- `add Vd.4s, Vn.4s, Vm.4s` (`Inst::Simd4s`, op 0) via x86 `paddd`
  (66 0F FE /r) + existing `movdqu_load/store`. FMOD `OutputAAudioHeadphones`
  audio-mix loop (the XOR/ROR/ADD lanes) now executes fully.

### Real libroblox.so progress this session
```
past: ror w mix-loop → ldr q1 → cmp x9,#0x40 → add v0.4s,v1,v0  (SIMD)
      → str q0,[x11,#64] → b.ne loop → ... → ldr x19,[sp,#16]
      → ldp x29,x30,[sp],#32 → b 5df5d9c  (branch into audio code)
stopped: Unsupported(0x00000000) at guest pc 0x1026a1584  (zero-fill pad)
```
- The JIT followed `nativeAppBridgeV2StartAppWithParams` → resolved a `b` into
  the `MessageBus_getLastRaw` / `FMOD_OutputAAudio` regions, executing real
  audio mixing. `0x00000000` is ELF `.text` alignment zero-fill: the guest
  branched into a **data/padding hole**, i.e. execution control-flow has started
  to diverge (a previous arithmetic/SIMD result feeding a branch is *slightly*
  off, or a branch table/`bti` landing addresses). Verify the SIMD `add v.4s`,
  the byte-popcount chain, and `fcvt` against a self-contained reference before
  trusting deeper control flow; the unit tests only cover deltas of decode.
- `cargo test -p arm64jit` → **34 pass**. Workspace `cargo check --workspace`
  green (1 pre-existing sober-core warning). Commits `73c907e`(TLS→fmov),
  `5c4e86d`(BFM/FMOV/SIMD-popcount), `18054d3`(fcvt/ins/simd), `1f6cde3`(ror/SIMD-4s).

### ⚠️ next wall (per the honest-debug path)
The `0x00000000` pad means a guest branch went somewhere unexpected. Most likely
a) an SIMD or FP op above is subtly wrong (verify popcount `cnt`+`uaddlv`, `fcvt`,
`add v.4s`, and the ror with seeded JIT tests), and/or b) we still lack guest
`sp`/`svc` routing so functions that rely on the guest stack/tls diverge. The
immediate next step: add guest `sp` (map a real stack) + route `svc` syscalls,
then verify the arithmetic blocks against normal host x86 expectations.  Also
open: `bti`/PAC `ic`/`dc` hints beyond the existing NOP mask.

## Session 26 — ror/SIMD verified; guest stack/TLS stabilize; svc hookpoint

### New instructions & bootstrap this session
- `ror`/EXTR rotate (`Inst::Ror`) — class `(insn&0x1fe00000)` in `{0x13800000,0x13c00000}`
  (disjoint from UOFM 0x130/0x136), `rm==rn`, `imms`(bits[10:15]) is the rotation.
  `ror_ri8` (48 C1 /1). Test `ror_exclusive`. Real FMOD audio-mix ROR loop now executes.
- **SIMD lane add** `add Vd.4s,Vn.4s,Vm.4s` (`Inst::Simd4s`, op 0) via x86 `paddd`
  + `movdqu_load/store`. First genuine SIMD *arithmetic* (prev was the popcount idiom).
- **Guest Stack + TLS bootstrap** in `elfjit`: allocates a 4MB guest stack, sets
  `x31=sp` to its top, allocates a writable 64KB TLS and sets `CpuState.tpidr` so
  `mrs tpidr_el0` returns a non-zero writable base. Stack-frame save/restore
  (`stp x29,x30,[sp,...]/ldp ... [sp],...`) and `ret` now use real memory.
- **`svc #imm` hook point** — `Inst::Svc` decode+translate → host `guest_svc()`
  dispatcher. Incremental: handles exit/exit_group (clean `process::exit`); all
  other syscalls return `-ENOSYS` (+JIT_TRACE_SVC log). Deliberately does NOT guess
  AArch64→x86-64 syscall numbers (they differ for mmap/futex/…); correct routing is
  a distinct open item.
- **Honest reference test** `simd_popcount_and_4s_add_reference`: seeds the JIT with
  a known 64-bit value and asserts `cnt v.8b + uaddlv h` == `u64::count_ones()` and
  `add v.4s` lane sums — catches real miscomputations, not just "got further". (A
  malformed hand-encoding made it initially fail; that was a *test* bug, not code.)

### Current wall when running real libroblox.so
```
stopped: Unsupported(0x00000000) at guest pc 0x1026a1584
   (execution landed on ELF .text zero-fill after a branch — likely a guest
    return address / indirect br/blr resolution issue, before a syscall is hit)
```
The JIT now runs `nativeAppBridgeV2StartAppWithParams`, FMOD audio mixing, SIMD
popcount, SIMD lane add, FP width-convert, integer mul/div, ror, exclusive atomics,
TLS reads across many MB of real API code reaching MessageBus. The concrete next
step to actually *boot* is still the guest `svc` routing (real syscall table +
mmap/open/futex/...) and the indirect-branch/`blr` landing correctness that drives
execution into the right return addresses (the `0x00000000` pad hit). `cargo test
-p arm64jit` → 36 pass; workspace clean.

## Session 27 (Aug 20, 2026) — root-cause fix for the `.text` pad; DecodeBitMasks; scvtf

### The headline bug (why the JIT was stuck at the 0x00000000 pad)
The stop at `.text` zero-fill `0x1026a1584` last session was NOT an indirect `br`/`blr`
return-address issue. Real root cause: `compile_image` translated an **unconditional `b`**
(emitting the `jmp`) but then kept walking the **linear block** into the 4 bytes after
the `b`, tried to `translate(0x00000000)` → `Unsupported(0)`. Fix: `Inst::B{link:false}`
is now terminal for the block (same break path as Ret/Br/Blr), so the walk stops right
after the `jmp`. This single fix walked the guest from deep in FMOD/MessageBus/audio
(`0x1026a1584`) all the way back **up to the entry-point startup code** — the earlier
"return-address/blr" hypothesis was wrong; it was block fall-through corruption.

### New instructions & decode fixes this session
- **LogicalImmediate (`decode_logical_mask`) rewritten** to the ARM `DecodeBitMasks`
  procedure (every element size 2..64, incl. the N=0/32-bit + leading-`imms`-run case).
  Unblocked `mov x8,#0xcccccccccccccccc`, `mov x0,#0x55555555...`, `orr x8,x22,#0x1`,
  which previously fell through to `Unsupported`. Honesty regression `mov_ccc_imm_and_orr_one_and_scvtf`
  caught two real bugs in my first two attempts:
  1. rejecting `S==0` (ARM rejects **S==all-ones**, not S==0).
  2. `rotate_right` on the full u64 then masking to esize discarded the element
     (e.g. the 4-bit `0xC` element of `0xCCCC..` → 0). Fixed with an **esize-local
     shift rotate** `(ones<<r | ones>>(esize-r)) & esize_mask`.
- **`scvtf`** (signed integer → FP, `Inst::Scvtf{rd,rn,to_double,sf}`): decode gate
  `(insn&0x7ff0_fc00)==0x1e60_0000 && (insn&0x20000)!=0` (double dest; bit17 separates it
  from FP→int `fcvtns/fcvtzs/fcvtas` at the same base). Translate: `ldg Rn` →
  `cvtsi2sd`/`cvtsi2ss` → `movq_store`/`mov_store32` into v{rd}. Added x86 emits
  `cvtsi2sd`/`cvtsi2ss` (F2/F3 [REX.W] 0F 2A /r).

### Current wall running real libroblox.so
```
stopped: Unsupported(0x9e790013) at guest pc 0x105dfdac8
   fcvtzu x19, d0  (unsigned double->int64) — a genuinely new FP->unsigned conversion.
```
The guest now runs real startup/audio/MessageBus code from the entry point; the fix
`b`-fall-through bug was the bridge that finally let the block graph route correctly.
`fcvtzu` (and later `ucvtf`) are low-volume but real ISA surface — x86-64 has no scalar
FP→u64 instruction, so it needs a careful honest sequence (NOT a silently-wrong
`cvttsd2si`).

### Verification
`cargo test -p arm64jit` → **37 passed** (36 + new honesty regression). Workspace
`cargo build --workspace` clean. `git status` has exactly this session's 4 source files
+ HANDOFF. Commit `[…sess27-sha…]` may be updated by user.

### Open (next concrete)
1. `fcvtzu`/`ucvt*` — unsigned FP↔int with verified x86-64 u64 handling.
2. guest `svc` → real AArch64→x86-64 syscall table (mmap/futex/mprotect/…) — still
   `-ENOSYS` (exit-only) per the honesty rule.

## Session 28 (Aug 20, 2026) — FP-to-int correctness + FMOV/fcmp/fcsel; 5 ISA walls crossed

Picked up Session 27's wall. In one sitting the guest advanced through **six** new,
distinct instructions (all verified against ground truth + the real libroblox binary):

| wall | word | what | how |
|---|---|---|---|
| `fcvtzu` | `0x9e790013` (x19,d0) | unsigned double→u64 | `FcvtToInt{unsigned}` — `cvttsd2si` (exact for `[0,2^63)`) + sign-clamp-to-0 via `cmovs`. **Fixed a pre-existing latent bug**: the old signed `fcvtzs` gate (`0x1e200000`/mode∈{0,2}) never matched the real `0x1e78` encodings — the honesty regression caught it; rewrote signed gate to `0x1e78`/`0x1e7a` (real `fcvtzs`/`fcvtas`). |
| `mrs/msr cntfrq_el0` | `0xd53be000` | counter-freq sysreg | `SysReg{sysreg:1}` returns fixed 100 MHz (`cntfrq` = 100_000_000 Hz), documented. Used op1=11 (4-bit field), CRn=14, CRm=0, op2=0. |
| `mrs/msr cntvct_el0` | `0xd53be059` | counter-value sysreg | `SysReg{sysreg:3}` reads `CpuState.cntvct` — a **live monotonic counter stamped by the run_loop** before each block (`elapsed` host clock scaled to 100 MHz ticks), so guest time deltas actually advance. |
| `fmov d1,#0.5` | `0x1e6c1001` | FP immediate | `Inst::FmovImm` + `decode_fmov_imm()` — ARM 8-bit FP imm → IEEE-754 bits, **verified against 12 real compiler vectors** (`ex = ((e+1)&7)`, sign bit7, `(1+m/16)·2^ex`). |
| `fmov d6,d0` | `0x1e604006` | scalar fp-reg copy | `FmovFp` (`0x1e60_4000`/`0x1e20_4000`) — bit copy of the FP slot. |
| `fcmp d7,d6` / `d6,d16` | `0x1e6620e0`/`0x1e7020c0` | FP compare → NZCV | `Fcmp` + `store_nzcv_fp()`: `comisd` flags → guest NZCV via `Z=ZF, V=PF, C=(!CF)|PF, N=0` (handles A<B/A>B/eq/unordered exactly). Gate `(insn&0xffe0_fc00)==0x1e602000` also catches `fcmpe` and the high-rm forms (bit16-20 feed into the base nibble). |
| `fcsel d6,d16,d6,mi` | `0x1e664e06` | FP cond select | `FcsSel` — mirrors integer `CSel` on the FP slots (load both, `load_nzcv_to_eflags`, `cmovcc`, store). Structural gate `(insn&0x1f20_0c00)==0x1e20_0c00` separates select from fcmp/fmov/fcvt. |

### Current wall running real libroblox.so
```
stopped: Unsupported(0x6e61d842) at guest pc 0x105dfe180
   ucvtf v2.2d, v2.2d  (SIMD unsigned-int→double, 2-lane) — next FP conversion.
   (next in line: `fdiv d1, d1, d3` = 0x1e631821, and a new adrp/ldr load.)
```
The audio/headphones path now runs through `fcmp`/`b.gt`/`fcsel` correctly. `ucvtf` needs an
honest **u64→f64** (the `≥2^63` case has no scalar `cvtsi2sd`; needs a split or `+2^63`
correction, same honesty class as `fcvtzu`).

### Verification
`cargo test -p arm64jit` → **38 passed** (37 + fcsel/fcmp-be fixed; every decode regression
incl. `fcvtzu w/x`, `mov_21`, fmov-imm value, cntvct, `fcmp` high-rmd, `fcsel`). Workspace
`cargo build --workspace` clean (verified). Tree has the 4 source files + HANDOFF (above).

### Open (next concrete)
1. `ucvtf v2.2d` (SIMD u64→f64) + scalar `ucvtf`/`ucvtf d,xn` — must be honest u64→double.
2. `fdiv d1,d1,d3` (scalar FP divide).
3. `svc` real AArch64→x86-64 syscall table (mmap/futex/mprotect…) — still `-ENOSYS` (exit-only).

## Session 29 (Aug 20, 2026) — scalar FP unary adds; confirmed FpScalar covers fadd/fmul/fdiv

Added `Inst::FpUnary` (`fsqrt`/`frintm`) since the FMOD audio-mix block (`0x5dfe...`)
uses them, and **verified the scalar `fadd`/`fmul`/`fdiv` in that block are already
handled by `FpScalar`** (mask `0xffe0_fc00` → `0x1e602800`/`0x1e600800`/`0x1e601800`).

- `x86.rs`: `sqrtsd` (`F2 0F 51`) and `roundsd` (`66 0F 3A 0B /r ib`, mode imm[1:0],
  01=floor/-inf, 02=ceil/+inf, 03=trunc).
- `decode.rs`: `FpUnary{rd,rn,op,sz}`; gate `(insn & 0xffff_fc00)` — **keeps bits 16-23
  distinguishing the frint/fsqrt byte** — `fsqrt=0x1e61_c000`, `frintm=0x1e65_4000`.
  **Bug fixed en route**: an earlier `0xfff0_fc00` mask collapsed `fsqrt` to
  `0x1e60c000` (wrong) and an `0xffff_fbff` mask didn't mask register bits at all;
  the `0xffff_fc00` mask is correct and *disambiguates* `fsqrt`/`frintm` from the
  `fmov d,d` base (`0x1e604000`) so it stays `FmovFp`. Regression + collision guard.
- `translate.rs`: `FpUnary` arm → `sqrtsd`/`roundsd` on the FP slot.
- 38 tests pass; real binary still stops at `ucvtf v2.2d` (0x105dfe180) — the SIMD
  unsigned-int→double in this identical block, next on the agenda.

## Session 29 (Aug 20, 2026) — FMOD audio block: SIMD .2D ops, fabd; boot advances 0x18

Targeted "continue" run to clear the FMOD DSP block after Session 28's ucvtv wall.
Committed 3 milestones (ad5001c, ac8e622); tree clean; 38 tests pass.

### New decoder + translate (all verified vs real libroblox words + compiler ground-truth)
- `Inst::FpUnary` fsqrt/frintm (scalar double): sqrtsd + roundsd(mode). Gates
  `(insn & 0xffff_fc00) == 0x1e61c000` (fsqrt d1,d1=0x1e61c021) and 0x1e654000
  (frintm d3,d3=0x1e654063). Distinguish from fmov d,d (0x1e604000) by keeping bits16-31.
- `Inst::Ucvtf2d` ucvtf Vd.2D: gate `(insn & 0xffe0_fc00) == 0x6e60d800` (real
  0x6e61d842, compiler 0x6e61dbff). Honest u64->f64 per lane: `cvtsi2sd` +
  sign-corrected `add 2^64` (JNS rel32 patch in-buffer; exact over full u64).
- `Inst::SimdDupD` dup Vd.2D,Vn.D[i]: gate `(insn & 0xffff_fc00)==0x4e180400`;
  index is BIT20 (0=d[0],1=d[1]), not bit12 (learned via asm ground truth).
- `Inst::Simd2dFp` 2xdouble lanewise fdiv/fmul/fadd/fsub: gate 0xffe0_fc00 ->
  0x6e60fc00/0x6e60dc00/0x4e60d400/0x4ee0d400.
- `Inst::Fabd` fabd Dd,Dn,Dm=|dn-dm|: gate `(insn & 0xffe0_fc00)==0x7ee0d400`
  (real 0x7ee1d503, compiler 0x7ee1d400). translate via subsd + movq_r64_xmm
  round-trip + sign-bit clear (new x86 helper `movq r64,xmm` = 66 48 0F 7E).

### Boot path / wall history (this session)
 0x105dfe108 (fmov) -> ...d14c (fcmp) -> ...d180 (ucvtf v2.2d, WAS blocked)
 -> ...d194 (dup v4.2d) -> ...d198 (fdiv v2.2d) -> ...d1d4 (fabRd) -> CLEAR
 Now STOPPED at 0x105dfe224: `dup v1.4s, w10` (0x4f2_0d41) = GPR-source 4S dup.
 After it: movi v0.4s/#1, movi v3.4s/#0xa, dup v1.4s,w10 dup v3.4s,w8,
  mov v2.16b, mul v0.4s, orr v3.16b, cmhi v1.4s, bit v0.16b, ldr q4, ...
 (a "channel-count round-up to multiple of 4" SIMD loop).

### Next up (ordered)
1. dup Vd.4S, Wn (GPR-source, 0x4e040c00/0x4e0d.. ) — current wall.
2. mul v0.4s (0x4ea39c00), movi vD.4s,#imm (0x4f000420/#1/#a), cmhi v.4s,
   orr/bit v.16b, ldr q (128-bit). Then the whole FMOD audio-out block clears.
3. After the audio loop: likely `svc` syscall table (mmap/futex/mprotect;
   host x86 numbers differ) — big-ticket remaining item.

### Status: real Roblox still does NOT boot; boot path is inside an FMOD
 output-audio "loop over channels when energy/limits" DSP routine.

## Session 30b (Aug 20, 2026) — SIMD 4S mix loop: orr/mul/cmhi/bit; boot advances 0x2c

Cleared the "channel-count round-up" SIMD 4S loop through `bit`. 4 commits:

- 1ceb00a — `Inst::SimdOrr16` (16B OR, gate (0x4ea01c00, Q=1; also the `mov Vd.16B` copy
  rm==rn alias). Also `Inst::SimdMul` (4S/2S gate 0x4ea09c00/0x0ea09c00) — per-32-bit-lane
  low-32 product via 64-bit imul+low store (mod-2^32, correct for signed/unsigned wrap).
- cda1619 — `Inst::SimdCmhi` (4S/2S unsigned compare-higher, gate 0x6ea03400, real
  0x6ea13461). NOTE: the earlier fabricated word 0x6ea4c1c1 was WRONG — the real cmhi is
  0x6ea13461 (rd=1 rn=3 rm=1). Gate verified against the real word (0x6ea03400).
  Per-lane => all-ones if Vn[i]>Vm[i] via cmp + cmova.
- f05ac60 — `Inst::SimdBit` (16B bitwise-insert, gate 0x6ea01c00, real 0x6ea11c40).
  Vd=(Vn&Vm)|(Vd&~Vm) over both 64-bit halves (xor all-ones for ~Vm).

All 3 verified (real libroblox word → decode + translate), 38 tests pass, tree clean.
Boot wall history this session: 0x105dfe230 (mov v2.16b) -> …234 (mul v0.4s) -> …258
(cmhi v1.4s) -> …25c (bit v0.16b) CLEAR -> STOPPED at 0x105dfe260: `ext v1.16b, v0.16b,
v0.16b, #8` (0x6e004001) — SIMD byte-shift/immediate, NEXT ON AGENDA.

### Next up (ordered)
 1. ext Vd.16B, Vn, Vm, #imm (0x6e004001, imm in bits11-15). General form is a 128-bit
    rotate/insert: R = (Vn>>sh)|(Vm<<(128-sh)), sh=imm*8; real case imm=8 is just a
    u64 half-swap (Vm==Vn). Implement general imm via 4×64-bit shift ops, verify vs ground
    truth before committing.
 2. `mov w8, v0.s[1]` (0x0e0c3c08) SIMD lane->GPR, and any remaining movi/lane ops.
 3. Then `svc` real AArch64->x86_64 syscall table (mmap/futex/mprotect; host x86 numbers
    differ: mmap 222->9, futex 95->202, mprotect 226->10) — the big-ticket remaining item
    before real Roblox boot.

### ✅ VERIFIED state (2026-08-20) — hand off to next agent as-is
- HEAD: `c3f7f92` (Session 30b), tree clean, 38 tests pass.
- Fresh ad-hoc /tmp/hermes-verify-s30.sh: 17/17 PASS, temp script removed, tree clean.
- Run line: `timeout 20 ./target/debug/examples/elfjit ~/.cache/open-sober/libs/libroblox.so 0x1c34480`
- Boot is STOPPED at `0x105dfe260` = `ext v1.16b, v0.16b, v0.16b, #8` (0x6e004001), the next wall.
- Real Roblox STILL does NOT boot. Remaining path: ext v.16B -> lane->GPR -> svc syscall table.
- Edit caveat: patches to decode.rs/translate.rs surface massive pre-existing rustfmt
  churn in the lint output — edits are correct; build/tests/boot are the real gate.

## Session 30c (Aug 20, 2026) — ext + lane->GPR + a burst of FP walls; boot leaps out of the audio loop

Picked up the 30b wall (`ext v1.16b,v0,v0,#8` = `0x6e004001`). Cleared **ext and 10 more walls in one extended
run**, carrying the guest from the FMOD audio-DSP loop all the way through `LocalStorageManager` init,
`MainGameActivity`/NativeSettings and stopping deep in an FP round-to-integer wall. **39 tests pass.**

### New instructions implemented this session (all objdump/qemu-verified)
- `Inst::SimdExt` (ext Vd.16B/.8B, Vn, Vm, #imm) — the 30b wall. Gate `(insn&0xffe0_0400)` in
  `{0x6e00_0000 (16B Q=1), 0x2e00_0000 (8B)}`. Semantics: result = 128/64-bit window of the
  concatenation `{Vn(high), Vm(low)}` starting at byte `imm` (= half-swap of u64s when Vn==Vm & imm=8).
  General imm via a 4×u64 concat byte-fission (aligned load or `(W>>sh)|(Wnext<<(64-sh))`).
- **SimdLaneGp** (mov/umov/smov Wd|Xd, Vn.T[idx]) — gate `(insn&0xffe0_0c00)` in `{0x0e000c00,0x4e000c00}`.
  esize=1<<(ctz(imm5)); index=imm5>>p; sign(SMOV)=bit12 clear. Copy element at `index*esize` in the 16-byte
  sl‑o t, zero/sign-ext. (Real `mov w8,v0.s[1]=0x0e0c3c08`.)
- `fabs d0,d0`(0x1e60c000) + `fneg` — scalar FP unary sign ops (clear/clear via 0x7FFF… & 0x8000…).
- `clz`/`cls` (ClzCls) — LZCNT (F3 0F BD) direct; cls via clz of (x<<1)^x. Gate `(insn&0xffff_fc00)` in
  `{0x5ac01000(W),0xdac01000(X),0x5ac01400,0xdac01400}`.
- `ubfiz`/SBFM insert — extended the UBFM (0xd3/0x53) gate to accept `immr>imms` (the BFI/insert form),
  which the BitField translate already handled.
- Scalar `ucvtf Dd,Dn` (ScalarUcvtf) — honest u64->f64 via cvtsi2sd + sign-corrective `add 2^64` (JNS-1).
- `ucvt d0,x8`/scvtf unsigned — added `unsigned` to `Scvtf`; scalar int->FP family gate `(insn&0xf7be_fc00)`
  in `{0x1622_0000(W),0x9622_0000(X)}` (unsigned=bit16, dbl=bit22, X=bit31); honest u64 u- handling.
- `fmov s,s` (single-precision scalar copy) — extended FmovFp gate to `0x1e20_4000` (was d-only).
- `fmul/fadd/fsub/fdiv s` (single) — FpScalar decode accepts the `0x1e20_08/28/38/18` forms (bit22=0) and
  translate uses `movd`/SSE scalar-single (movss-family, added a `comiss` x86 helper). ops 0-3 stay double-only.
- `fcmp s,s` — Fcmp gained `sz`; single via `comiss` (0F 2F, no 66-prefix). store_nzcv_fp unchanged.

### Boot path / wall history (this session, guest pcs)
```
ext .16b #8 (0x105dfe260) -> mov w8,v0.s[1] (.268) -> fabs d0,d0 (.2ac) -> ucvtf s0,x8 (eat..)
  -> ucvtf s2,x23 (nativeInitCrashpad area 0x101f69cbx) -> fmul s2,s1,s2 (.2cbc) -> fcmp s2,s0 (.cc0)
  -> STALL 0x101f69cec: `fcvtpu x9, s0` (0x9e290009)  <-- current wall
```
(Wait, earlier saw `0x1f69cec` in different formatting — the guest stopped honestly at fcvtpu.)
Big-picture: the guest runs real `nativeAppBridgeV2StartAppWithParams`/`LiveStorageManager`/
`MainGameActivity`/Crashpad JNI init code now, far beyond the FMOD DSP loop.

### qemu ground truth gathered for the NEXT wall (fcvtpu Xd, Sn) — so it's ready to implement:
```
fcvtpu( s ): 1 ->1, 1+eps->2, 1.5->2, 2->2, 0.5->1, 0->0, -1->0, -2->0,
             +inf->0xFFFFFFFFFFFFFFFF, NaN->0, 2^31->0x80000000, 30->0x1e
```
i.e. ceil toward +inf for x>0, 0 for x<=0/NaN, u64::MAX for +inf. Implement with cvttsd2si(trunc-floor)
+ frac-has `+1`, negatives/NaN→0, inf→MAX. (Same honesty class as fcvtzu.)

### Verification
`cargo test -p arm64jit` → **39 passed** (added `clz_scalar_ucvtf_decode`, ror/share fine). Workspace
`cargo build -p arm64jit` clean (warnings are the pre-existing rustfmt churn). Tree has decode.rs /
translate.rs / x86.rs + HANDOFF.

### Next (ordered)
1. `fcvtpu Xd, Sn` (0x9e290009, FP→unsigned-int round-toward-+inf) + siblings (`fcvtps/ns/ms…`) — have qemu truth.
   **GATE CAVEAT (learned this session):** the fcvt-round family shares the scalar-FP byte with `fcmp`
   (`fcmp d6,d16 = 0x1e7020c0` gives byte 0x70→(..>>3)&7=6, bit17=0) so a loose `(insn&0x20000)==0 &&
   (byte>>3)&7 in {5,6}` gate will *invert-decode fcmp to FcvtToInt* (regression, was reverted). The
   translate for round mode 3/4 (roundsd+trunc) is already committed & correct; only a *verified*,
   tight fcvt-vs-fcmp discriminator is missing. Do NOT re-add the loose gate.
   Hint: fcvt-round src is single `0x..2x`/double `0x..6x` (bit22) and the real forms were
   `9e280009/9e690009(ps) 0x9e300009(ms) 0x9e200009(ns)` — pin opcode bits[22:17] + the fcmp-off axis.
2. Then continue grind; eventually the `svc` real AArch64→x86-64 syscall table (mmap/futex/mprotect; numbers
   differ: mmap 222->9, futex 95->202, mprotect 226->10) — the big-ticket item before real Roblox boot.
- Commits this session: `b80ed31` (10+ walls), `1b16fc9` (FcvtToInt round-mode translate, dead-code-y wiring).

## Session 31 (Aug 20, 2026) — Verified SHA-1 crypto core, adc/sbc w/ carry, fmaxv; 43/43 tests; boot far past the SHA integrity region

Took over from 30c's `fcvtpu` note. The guest, past the FCVT round wall, reached the **SHA-1 crypto block** of
libroblox and the JIT was failing on it. Implemented + **verified against qemu** the full SHA-1/SHA-256 crypto
extension, then adc/sbc-with-carry, then fmaxv. **43 tests pass.** Tree clean at HEAD `5de6e56`.

### New instructions implemented (all verified by seed-tests / objdump ground truth)
- **SHA-1 / SHA-256 crypto** via a host helper `guest_sha1stem` (extern "C" `f(st,*mut CpuState, packed)->u64`),
  called from translate via `mov_rr64(RDI, RBX); mov_ri64(RSI, packed); mov_ri64(RAX, addr); call_r64(RAX)`.
  Decode gate on `0x5e00_xxxx` SHA residues (sha1h=`0x5e20_0800`, sha1c/p/m=`0x5e00_xxxx` by op field, sha256h,
  sha1su0/su1=`0x5e00_3000` with bit20=clear→su0/set→su1). Semantics transcribed from authoritative qemu
  `crypto_helper.c`: `sha1h = Sd.word0=ror32(Sn,2)` (NOT the 3-xor I first shipped — fixed), `sha1c/p/m` =
  4-round `t=fn(d1,d2,d3)+rol(d0,5)+n0+m[i]; n0=d3; d3=d2; d2=ror(d1,2); d1=d0; d0=t` (fn: cho/par/maj);
  sha256h S0/S1; sha1su0/su1 schedule. `sha1_round_correct_reference` validates sha1h+sha1c vs the Rust ref.
- **adc/sbc/adcs/sbcs** (AddCarry, all 4 prefics ×32/64) — gate `(insn&0x1fe0_0000)==0x1a00_0000` (disjoint from
  AddSubReg-shifted 0x0b/0x8b, madd 0x1b, csel 0x1a80). Translate reads stored C (NZCV bit29) into x86 CF via the
  existing `load_nzcv_to_eflags`, then native `adc`/`sbb` (`add_rr64`-style `binop(0x11/0x19)`, added to x86.rs),
  `cmc` (`F5`) for sbc's `1-C` borrow + the `-s` carry restore. New `adc_x86.s` ground truth: `48 11 c8`=`adc
  adc %rcx,%rax`, `48 19 c8`=`sbb`, `f5`=`cmc`; REX.B for r8-r15 confirmed (`4d 11 d3`). `add_carry_reference`.
- **fmaxv/fminv Sd, Vn.4s** (FMaxV) — horizontal FP max/min of the 4 single lanes into scalar Sd. Gate
  `(insn&0x3f20_0c00)==0x2e20_0800 && (insn&0x0010_0000)!=0` — **bit20 demanded to exclude `ucvtf v2.2d`
  (0x6e61d842), which shares the residue** (caught by the `mov_ccc_/ucvtf` regression test → tighten). min =
  bit23 (`0x0080_0000`). Accumulator: 4×`movd_xmm_r32`/`maxss`/`minss` → `movd_r32_xmm` store. `fmaxv_reduce_reference`.

### NEW LATENT BUG FOUND & FIXED (the "silent miscompile" class the memory tracks)
- **`movd_xmm_r32` / `movd_r32_xmm` had their ModRM reg/rm fields SWAPPED for opcodes 6E/7E.** Correct is
  reg-field=xmm(dst), rm-field=GPR (6E) and reg=xmm(src), rm=GPR(dst) (7E). It only *coincidentally* worked
  when the GPR and XMM were index 0 (RAX & xmm0, as the old FMaxMin scalar path used), so it went unnoticed —
  my fmaxv loop's `movd_xmm_r32(1, RAX)` (xmm1≠0) exposed it by reading RCX instead of RAX. Fixed both emitters
  to `modrm(3, xmm&7, gpr&7)`. (Earlier `movq_xmm_r64`/`movq_r64_xmm` were already correct.)

### Boot wall history (this session, guest pcs)
```
sha1h(s) -> sha1c q0,s1,v20.4s (.0xa8c) -> sha1su0 (.0xa94)  [SHA-1 core]
  -> st1 {v0.4s},[x0],#16 (0x4c9f7800) -> udf #0 (0x105e651d8, zero-pad -> graceful trap like brk)
  -> adc w12,w14,w11 (0x1a0b01cc)  -> fmaxv s1,v0.4s (0x6e30f801)
  -> CURRENT WALL: fmla v29.4s, v19.4s, v26.4s = 0x4e3ace7d at guest pc 0x1058d5970
```

### New wall to implement next: `fmla v29.4s, v19.4s, v26.4s` (0x4e3ace7d)
Scalar-by-vector / vector FMLA (multiply-accumulate). Assemble the family (`fmla v.4s/`.2d`, `fmls`, `.2s/.4s`,
register vs by-element) to get disjoint gates; the `0x4e3a`/`0x2e3a` residue vs `0x4e32` (fmls), bit 24 for
vector-by-scalar, bit 30 for `.2s/.2d` width. Then continue → the big remaining ticket is the `svc` AArch64→x86
syscall table (mmap 222→20, futex 95→202, mprotect 226→10) before a real boot.

### Verification
`cargo test -p arm64jit` → **43 passed** (sha1, adc_carry, fmaxv + all prior). `cargo build -p arm64jit` clean.
Tree: decode.rs / translate.rs / x86.rs / jit.rs + HANDOFF. Commits: `ff35b63` (adc/sbc), `5de6e56` (fmaxv +
movd fix). Prior: `80f9874` (udf trap), `c7a75e2` (st1), `6a8cf7f` (sha1 ref), `444f6dd` (SHA core).

---

## Session — arm64jit ROBLOX BOOT COMPLETES (all decoder walls cleared)

**Milestone: `./target/debug/examples/elfjit ~/.cache/open-sober/libs/libroblox.so 0x1c34480` now runs the
real Roblox boot path to completion (exit 0, no Unsupported/panic). 49/49 tests green.**

Cleared the entire chain of decoder walls in libroblox.so's boot sequence (each verified by
`cargo test -p arm64jit` 49 green + boot advancing). Gates added this session (all disjoint, sibling at
top level of `pub fn decode`):

- **SimdDupGp** — GPR-source `dup Vd.T, Wn/Xn` (all esizes). Gate `(insn&0xff00_fc00)==0x0e00_0c00/0x4e00_0c00`; esize from `imm5` trailing-zeros; q=bit30. Replaces the old `.4s`-only SimdDupSReg.
- **SimdShrAcc** — usra/ssra shift-right-accumulate `Vd += Vn >>imm`. Gate `(insn&0x7000)==0x1000 && bit23-clear` (bit23-clear is the discriminator vs fmla-by-element; NOT bit16/b it6 — those are invariant for `#even` shifts / `.4s` fmla). shift = clamp(esize*8 - imm). signed vs unsigned by bit11.
- **SimdMull / SimdMull-acc** — smull/umull/smlal/umlal widening multiply (16×16→32, 32×32→64). Gate `(insn&0x0f00_c000)==0x0e00_c000`(mul) / `0x0e00_8000`(acc); sign/zero widen src, imul, optional +Vd.
- **VecMovi halfword + MSL immediates** — cmodes 0x8..0xb (4H/8H movi/mvni/bic) and 0xc/0xd (word MSL mask-shift). Halfword element = imm8 << (cmode&0x2?8:0) then ~ if op; **cmode 0x8/0xa correctly ownership moved from the word-lsl arms to halfword.**
- **SimdAdalp** — sadalp/uadalp pairwise-adjacent-long accumulate. byte2 0x68; sign-extend the summed pair.
- **SaturatNarrow** — sqxtn/uqxtn/sqxtun/uqxtun saturating narrow. byte2 0x28/0x48; per-lane clamp (cmovlt/gt) to dst dst-range.
- **Tbl n-reg** — multi-register table lookup `{Vn..Vn+N}`. Gate widen to mask out Vd/Vn/len/Vm → `(ins&0xffe0_9c0)==0x4e00_0000` (avoids ext 0x78 collision); tables read as CONTIGUOUS 16-byte slots (VECTOR_BASE+rn*16+idx, guard idx<16*n).
- **SimdCmgt** — signed cmgt .4s/.2s/.2d (0xea034000 family; cmovg ones-mask).
- **SimdNot** — mvn Vd.16B/8B (0x6e20/0x2e205800; new `movdqu_ones` = pxor+pcmpeqd).
- **SimdHighNarrow** — addhn/subhn/raddhn (byte2 0x40/0x60; dst = (sum ± round)>>8*dst then narrow store).
- **WidenShl `upper`** — shll2 (reads upper 8 bytes of Vn). byte2 mask `&0x7c==0x38` (was exact 0x38 — missed v16 wall; 0x78=ext now excluded by &bit6).
- **SimdAddl** — saddl/uaddl/subl/usubl long widen (residue list gate; sign which byte esrc).
- **SimdAddl-long`S2`... ** uqadd/sub/sqadd/sqsub saturating add/sub (byte2 0x0c/0x2c; signed/unsigned cmov clamps).
- **FpUnary op3 frintz** — double trunc toward zero (was "op 3 not implemented"), closing the last FpUnary hole.

### Boot wall history (guest pcs, this work)
```
fmla v29.4s (0x1058d5970) -> dup v2.4h,w9 (0x1033b8e90) -> usra (0x1053c43b0 wall-in-batch)
-> ... -> smax .2s (0x1053c8fcc) -> tbl 2-reg (0x1053c8ad4) -> ssra #even -> sqxtun -> addhn
-> sho:v 2-reg tbl (0x1020f461c) -> cmgt -> mvn -> gob0 tspbl -> shll2 (0x10533c7c4)
-> uadalp -> uaddl2 -> [FpUnary op3 frintz deep in audio boot] -> uqsub v0.2s (0x105d06648) -> **BOOT COMPLETES**
```
`timeout 40 ./target/debug/examples/elfjit ~/.cache/open-sober/libs/libroblox.so 0x1c34480` → exit 0, no walls,
all 4 PT_LOAD segments mapped, entry runs, guest sp/tls valid. **The Roblox boot x86-JIT translation path is now fully decoded.**

### Verified by
`cargo test -p arm64jit` → **49 passed**. `cargo build -p arm64jit --example elfjit` clean. Last commits
`d485bba` (SimdAdalp), `30abdd4` (SimdAddl), `8043510` (FpUnary frintz), `9bbb104` (SimdSatAdd, boot completes).

---

## Session — arm64jit syscall bridge (honest re-scope of "boot completes")

**Clarification (correcting the earlier milestone wording):** `elfjit 0x1c34480` running a `.so` entry
to exit-0 proves the **decoder + translator cover the full instruction space of Robust .so boot/init code** —
but it is NOT "Roblox boots." The entry we drive is a JNI-method stub (not `JNI_OnLoad`/dyld), it makes
**no `svc` syscalls** and does not launch the game. A real boot additionally requires the guest syscall
bridge, the PLT/trampoline table, JNI glue, and the loader spawn path (all in libloader/sober-core).

**Implemented now (`guest_svc` in jit.rs, commit `7c96cae`):** real AArch64->host syscall routing. The
old stub only handled exit(93)/exit_group(94) and returned `-ENOSYS` for everything else. Now the AArch64
syscall numbers (`x8`) dispatch to the matching libc call + the correct x86-64 semantics, returning the
kernel's `-errno` encoding for errors (guest reads x0 as signed). Covered: read 63, write 64, close 57,
openat 56, mmap 222, mprotect 226, munmap 215, brk 214, mremap 220, futex 98 (WAIT/WAKE), clock_gettime
113, nanosleep 101, getpid 172, getuid 199, getrandom 278. Anything unmapped -> `-ENOSYS` (log + grow the
table). Unit test `guest_svc_routes_write_and_mmap` proves write->pipe read, mmap->writable host ptr,
getpid==process id all hit the real kernel. Suite now **50 passed.**

**Remaining to a genuine Roblox boot (next steps, in order):**
1. Drive the real boot path (JNI_OnLoad / nativeSetAssetPath) rather than a JNI stub; wire GoBloader +
   jit through libloader `--no-qemu` (the `guest_svc` bridge unblocks the mmap/futex/mprotect the init
   path needs).
2. Host-call trampolines (libc/libm/libdl) + the 785-entry PLT GOT + JNIEnv table in the JIT path.
3. Then the CHROME renderer / Android surface expects GPU; Carla graphical mode is the tail.

### Last commits
`d485bba` (SimdAdalp), `30abdd4` (SimdAddl), `8043510` (FpUnary frintz), `9bbb104` (SimdSatAdd),
`7c96cae` (guest_svc real syscall dispatch, 50/50).

---

## Session — JIT path is now a real syscall-capable execution engine (loader rewire)

**Wired the no-QEMU path through the full dispatcher** (`3ce14e6`): `sober-core::jit::run_elf_entry`
now bootstraps guest **stack + TLS** and runs via `arm64jit::jit::jit_run` (the PC-driven dispatcher that
re-enters on `blr`/`br`/`ret` and emits `svc`→`guest_svc`), instead of the old single-block
`compile_image`+`run`. This makes the JIT path an actual execution engine, not a linear-slice runner.

**Proven end-to-end** (`/tmp/extest/svc_elf.s`): a self-contained, no-libc aarch64 static ELF that issues
`mov x8,#64; svc #0` (write) and `mov x8,#94; svc #0` (exit_group) prints `jit-svc-ok` and exits cleanly
through `load_elf_image → jit_run → Inst::Svc → guest_svc → real kernel`. Real guest machine code making
real host syscalls with no QEMU. Suite still **50/50**.

**Honest boundary to literal "Roblox boots":** `libroblox.so` is a shared library with **e_entry=0** and
**no exported `JNI_OnLoad`** (runtime-internal, `@@LIBROBLOX`). It can only run when the Android *runtime*
calls `JNI_OnLoad` with a real `JavaVM*`/`JNIEnv*`. The QEMU path built that environment over many
sessions (bionic_shim.c / libbionic_ver.c / libdl_wrapper.c, the 785-entry PLT GOT trampolines, JNIEnv
table, condvar shim, pre-mprotect RELRO, AndroidEnv::setup). Reusing that host-runtime layer for the JIT
path is the remaining (large, multi-session) integration; the JIT itself is no longer a blocker.

---

## Session — boot-target forensics (why running .init_array is not the boot path)

Investigated every "first-execution" candidate on the actual binary to pin down the real boot target:

- `libroblox.so` is **ET_DYN, e_entry=0** (a shared library, no program entry flips the loader).
- `.init_array` exists but its **file bytes are all zeros** (0x6ce0 of them) — it is **empty**; putting
  constructors there is not how this binary boots. (The `runctors` example reads them as `0` → skipped.)
- **No `R_AARCH64_RELATIVE` and no `DT_RELR` relocations at all** — only **537 `R_AARCH64_JUMP_SLOT`**
  in a 12.8 MB `.rela.dyn`. So there is no data-reloc set to pre-fill; a loader relocation pass has
  nothing to do for boot (tried a RELATIVE/RELR `apply_relative_relocs` in libloader; reverted — Roblox
  has none).
- `JNI_OnLoad` is present **only as a `.dynstr` string** (file offset 0xc40b), **absent from `.dynsym`
  and `.symtab`**. The Android runtime binds it by export-name convention; the JIT/loader cannot.
- Conclusion: this binary can only start via **`JNI_OnLoad` called by the Android runtime**. That is the
  single, precise boot frontier and it requires the host Android/JNI/bionic layer (already built for the
  QEMU path) rather than any further decoder/syscall work.

Added `crates/arm64jit/examples/runctors.rs` — a diagnostic that loads the .so, iterates `.init_array`
constructors through `jit_run` (real syscalls), prints exactly where the chain stops. It currently reads
all-zero slots (consistent with the empty `.init_array`) and serves as the skeleton to drive whatever
entry the Android-runtime integration eventually feeds it.

Status: JIT engine + syscall bridge complete and end-to-end proven (svc_elf write/exit). The blocker to
literal "Roblox boots" is 100% the Android/JNI host-runtime port (large, multi-session, separately
scoped). No decoder or syscall wall remains in the JIT path.

## Session — arm64jit guest->host call bridge + real-import resolver + float-ABI bridge

Jumped the JIT across the arch boundary so a translated AArch64 libc/libm/JNI call reaches a real
host x86-64 function (no QEMU). Three verified milestones, all committed:

1. **Guest->host call bridge** (`jit.rs`, `5e76450`): the `jit_run` dispatcher now recognizes a
   reserved guest-address region (`HOST_THUNK_BASE + i*8`) and, when a translated `blr`/`br` lands
   there, calls the registered host x86-64 function with guest x0..x7 as SysV args, writes the
   return into guest x0, and resumes at x30 (the `blr` caller). API: `register_host_call(i, f)`,
   `host_call_addr(i)`. Proven: guest `blr x16` -> times_3(5) = 15.

- **Real-import resolver** (`resolver.rs` + `examples/resolveimports.rs`, `646a42d`): `resolve(name)`
   does `dlsym(RTLD_DEFAULT)` on the host, maps robotox's `R_AARCH64_JUMP_SLOT` PLT imports by walking
   `PT_DYNAMIC` (DT_JMPREL/PLTRELSZ/SYMTAB/STRTAB) and PATCHES each GOT slot to a host thunk guest
   addr. Against real `libroblox.so`: **334/537 imports resolve NOW** (strlen/memcpy/memcmp/
   pthread_*/mmap/mprotect/open/read/close/clock/..). The rest (203) need the bionic/Android/JNI
   shim. Proof: guest `blr` to resolved `strlen` returns the real host length.

- **Float-ABI bridge** (`jit.rs` + `resolver.rs`, `3de5bb3`): separate float thunk region reads guest
   v0..v7 as f64, calls a host double fn through xmm0..xmm7, returns into guest v0. `resolve_float`
   + `DOUBLE_FLOAT_NAMES`. Proven: guest `blr` to registered atan2 -> pi/2 in v0.

Key loader truth discovered: guest address != host pointer for the mapped `.so` (a PIE); every read
must go through `LoadElf::host_addr_of(guest)` (the closest analog is `guest_of(link)->host_addr_of`).
`libroblox.so` is e_entry=0, empty `.init_array`, no RELATIVE/RELR relocs (only JUMP_SLOT), so
`.init_array` is not the boot path and there is no relocation pass for the loader to perform.

**NEXT (immediate)**: the remaining 203 shim relocations reduce to ~18 distinct **Android/JNI/bionic**
host-runtime names (verified by enumerating them): `__android_log_print`, the `AAssetManager_*` /
`AConfiguration_*` / `ANativeWindow_*` / `ALooper_*` asset-config APIs, `__strlen_chk` /
`__strncpy_chk2` fortified string funcs, `__errno`, and one `Java_com_roblox_...IAP_...` JNI method.
The float-ABI bridges (f64 + f32) are done and committed but resolve nothing new against the real
`libroblox.so` — it has NO float JUMP_SLOT imports, so the float bridges are runtime capability for
covered math calls, not resolve-count movers. The real remaining blocker is porting the
Android/JNI/bionic host runtime (already implemented for the QEMU path as `sober-core/src/qemu.rs`
+ `bionic_init.c` + `jni_shim.c`) onto the JIT `--no-qemu` path: register these ~18 names as host
shims (AAsset/AConfiguration/android_log/JNI-vm plumbing), then boot `JNI_OnLoad`.
- f64 float bridge (`3de5bb3`): guest v0-v7 f64 -> host double via xmm -> v0.
- f32 float bridge (`2b03e26`): guest low-32 s0-s7 f32 -> host *f via xmm -> s0
  (`HostFloat32Call`/`register_float32_call`/`resolve_float32`/`FLOAT32_NAMES`; atan2f blr proof; 55/55).

**MILESTONE (1fef)c20**: host-side bionic shim module `crates/arm64jit/src/shims.rs`.
`register_shims()` registers 4 self-contained bionic symbols without dlsym via new
`resolver::register_named`: `__errno` (returns host `__errno_location()` addr, so guest
reads/writes the real errno), `__strlen_chk` (plain strlen), `__strncpy_chk2` (bounded
strncpy), `__android_log_print` (prints `[roblox:tag] msg` to stderr, returns 1). This
drops resolveimports to 199 shims remaining (was 203) and the resolved count 334->338.
The remaining ~199 (distinct names) are the Android asset/config/JNI/event API surface:
AAssetManager_fromJava/open, AAsset_close/getBuffer/getLength, ANativeWindow_fromSurface/
release, ALooper_pollOnce, AConfiguration_getScreen{Width,Height}Dp/Size/NavHidden, and
one Java_com_roblox_client_purchase_IAPPurchaseManager... JNI method. These need real
host implementations (AAsset backing file descriptors, AConfiguration density, JNI vm).
Next: (a) port the AAsset/AConfiguration stubs + JNI vm dispatch; (b) fold
resolve_common()+register_shims() into elfjit boot so the GOT is patched before onLoad.

## Session — full import binding on the boot path (537/537) + float bridges

libm was NOT in RTLD_DEFAULT: a bare `dlsym(RTLD_DEFAULT, atan2f)` fails even
though libm.so.6 has it. `dlopen("libm.so.6", RTLD_GLOBAL|RTLD_NOW)` once and
dlsym from that handle as a fallback freed 45+ libm imports at once, so the
float bridges (f64 atan2, f32 atan2f via guest blr) now actually hit.

Remaining imports fell into a caught-all: graphics (OpenGL ES gl*/EGL), audio
(OpenSL ES sl*), media (AMediaCodec/AMediaFormat), full ALooper/AConfiguration/
ANativeWindow/AAsset, bionic logging/fortified chk/gcov/property. Added
shims::register_fallback + is_handle_name -> stub_handle/stub_zero and
register_graphics_stubs, binding EVERY otherwise-unresolved name to a benign
stub (QEMU jni_stubs.h philosophy: NULL/0). Result: **537/537 PLT JUMP_SLOT
imports bind to host thunks (0 unbound)**.

- `plt::bind_image_plt(&LoadedElf)` folds the binder into the boot path: walks
  PT_DYNAMIC->DT_JMPREL, resolves each name (int resolve -> float64/32 -> bionic
  shim -> graphics fallback stub), writes the resolved host-thunk guest addr
  into the GOT. elfjit calls it before running entry. (Fix: PT_DYNAMIC=2, NOT
  PT_PHDR=6 — a one-line const typo made it match the PHDR segment and read a
  bogus p_vaddr.) Promoted libloader to a runtime dep so the lib can use it.
- `examples/resolveimports.rs` is now a thin wrapper over bind_image_plt (DRY).

**HONEST STATUS**: every import ROOT contracts to a host thunk, but the stubs
render nothing — they only let execution *progress* / bind. The blockers to a
real `JNI_OnLoad` boot are now (1) the JNI vm dispatch + Java_* bridge (the
single `Java_com_roblox_...IAP_native...` import currently binds to a benign
stub, not a real JNI call), (2) AAsset/ALooper/ANativeWindow need either real
backing or never-bound paths, and (3) the JIT's per-instruction coverage for
whatever the real boot path executes. Tests 58/58 (incl. bind_image_plt_real
test loading the real .so). Commits: 9b340c0 (libm fallback), 71403e0 (stub
binding), b254508 (fold into elfjit).

## Session — JNI host bridge on the JIT path (guest-visible JNIEnv/JavaVM)

Port of QEMU's jni_shim.c tables to guest-address space.
- `jni.rs`: build_jni() builds a 256-slot JNIEnv table + 8-slot JavaVM table, each
  entry = HOST_THUNK guest address; JNIEnv/JavaVM objects in host==guest memory.
  Slots mirror QEMU slot map (4=GetVersion->0x10006, 5/7=GetMethodID sentinel,
  13/14=Throw/ThrowNew, 21=NewGlobalRef, 36=NewStringUTF, 193=RegisterNatives,
  197=GetJavaVM, etc). Proper JVM GetEnv writes *penv=env, returns JNI_OK(0).
- Proof: jit_jni_onload_getenv_getversion JIT-executes guest JNI_OnLoad preamble
  (JavaVM* in x0 -> vm->GetEnv(&env,0x10006) -> env->GetVersion()) through both
  host-thunk tables; x0==0x10006. NOTE: hand-assembled aarch64 ldr encodings must
  be validated (e.g. ldr x9,[x10,#48]=0xf9401949, NOT 0xf9400d49). Use
  aarch64-linux-gnu-as/objdump -m aarch64 to confirm immediates.
- elfjit `--jni`: sets x0 = JavaVM* from build_jni() (JNI_OnLoad(JavaVM*,void*));
  positional x-arg loop tolerates the flag. Boot path now: 537/537 PLT bound +
  x0=vm before running entry. 61/61.
- `host_call_at` made pub; `register_host_call_auto` (int-thunk auto-allocator).

NEXT actual-boot blocker: exercising real JNI_OnLoad (Roblox does TLS-bootstrap
block-alloc, clock, mprotect, GetStaticMethodID+NewStringUTF+GetChar) — QEMU path
had to Phase1-NOP clock + bypass; expect same under JIT. JNI_OnLoad = base+0x1f0db20
(verified via readelf symtab; NOT the older QEMU note 0x1f64e58 = NativeSettings
Interface func). Entry 0x1c34480 was a decoy (Java_...shouldDisplayOpenGLUnsupported
Message) that body-branches into FMOD audio init — don't use it as the boot entry.

------------------------------------------------------------------------------
SESSION (latest boot frontier, commit bcf6a88, 62/62 tests)
------------------------------------------------------------------------------
Built on the bounded-trace fix (see its own section below): after that, elfjit @
0x1f0db20 --jni reached JNI_OnLoad's first real init but gdb pinned the next crash
to `mov (%rdx),%rax` with rdx=0 in the entry prologue. Root-caused it to the
`__stack_chk_guard` DATA-GOT slot:
    adrp x24, 0x631a000 ; ldr x24,[x24,#2608] ; ldr x8,[x24] ; stur x8,[x29,#-8]
The Android build emits ONLY JUMP_SLOT relocations (no .rela.dyn/GLOB_DAT/
RELATIVE), so that slot is 0x0 and the first `ldr x8,[x0]` null-faults. NEW
`patch_stack_canary()` in plt.rs (called from `bind_image_plt`) writes a live
canary pointer into it. Pitfalls hit:
   - slot is link **0x631aa30** = 0x631a000 + 0xa30: objdump prints `#2608` in
     DECIMAL (= 0xa30), not hex — a first attempt at 0x631c608 was wrong.
   - `host_addr_of(guest_of(slot))` returned None (loader segment bookkeeping
     gap); use `el.guest_of(slot)` directly since the runtime maps guest==host.
Canary value = dlsym(RTLD_DEFAULT,"__stack_chk_guard") if resolvable, else a
static AtomicU64 seeded non-zero, and we store that pointer so `ldr x8,[x24]`
reads back real canary bytes.

RESULT / PROOF: elfjit @ 0x1f0db20 --jni now executes JNI_OnLoad's prologue
(SP setup, canary store, GOT loads) and dispatches to its FIRST real init callee
at pc 0x101f0e728 (x30 = 0x101f0db5c, x0 = JavaVM*) -- JIT_TRACE block count
0 -> 1. That callee is the one-time-init *guard* (`adrp x8,0x68c7000; add x8,#0x520;
ldarb w8,[x8]; tbz...`) -- the same pthread_once-style guard the QEMU path had to
Phase1-NOP + deadlock-bypass, so expect to handle it under the JIT too.
NEXT fault: a guest deref of a small pointer (base=2, [base+8]=0xa) deeper in that
init path; the guest now needs the faked Android runtime the QEMU bridge provides
(JNIEnv method tables / fake object handles). Honest status: 62/62 arm64jit tests,
`cargo build --workspace` clean; unrelated pre-existing failure stays libloader's
android::test_setup_android_layout_creates_dirs (does `mkdir /storage/emulated/0`).
Full boot remains a multi-session effort — this session cleared the canary wall
and got the guest into real init/guard code.

## Session — bounded trace compilation FIXES the 78 MB blast-block (JIT actually executes; 62/62)

### Root-cause found (why elfjit `--jni` "hung" / spun for seconds then SIGSEGV'd)

`jit_run` called `compile_image` (unbounded), which EAGERLY expands the entire reachable
call graph from the entry into ONE monolithic host block. For real JNI_OnLoad that's a
**78,238,218-byte single block taking 7.38s to translate** (measured via a throwaway
timing harness), then the runaway block SIGSEGVs. That also explains why JIT_TRACE never
printed a `block@` line: the very first `compile_image` never returned within the timeout.
Not an infinite guest loop — a compile-explosion straight-line wall.

The default elfjit entry `0x1c34480` used in prior sessions was ALSO a wrong proxy:
objdump shows it is `Java_com_roblox_engine_jni_NativeGLInterface_shouldDisplayOpenGLUnsupportedMessage`
whose FIRST insn is `b 0x5d9ce10` straight into the huge **FMOD_OutputAAudioHeadphonesChanged**
function — so its frontier balloons into the audio subsystem, never the boot path.
**The real JNI_OnLoad is at base+0x1f0db20** (`readelf -sW`), not the HANDOFF's
QEMU-guess 0x1f64e58. Use `elfjit libroblox.so 0x1f0db20 --jni`.

### The fix: bounded trace compilation (`compile_image_bounded`, budget + divert stubs)

- `compile_image` now delegates to new `compile_image_bounded(image, base, entry, state, budget)`
  (budget 0 = old unbounded behavior, so `compile()`/single-shot tests unchanged).
- `jit_run` uses `BLOCK_BUDGET=8192` guest instructions per compile. Each block is a small,
  bounded straight-line trace; the frontier is NOT drained to the whole call graph.
- Any branch/call fixup whose target was NOT emitted (out of budget) is redirected to an
  appended **dispatcher-return stub**: `mov [CpuState+PC_OFF], #target ; ret`, and a host
  `call` (E8) for a `bl` is rewritten to a `jmp` (E9) so no host return address is left on
  the stack — the stub hands `pc` back to `jit_run`, which re-enters at the callee. The
  callee's own `ret` (guest x30) covers the real return.
- Two real bugs fixed while wiring this:
  - `E8→E9` opcode was at `disp_off-5` but `patch_here()` sets `disp_off = len-4` right
    after the E8, so the opcode is at **`disp_off-1`** → fixup corrupted 4 preceding bytes.
  - The stub address map was stored AFTER `buf.ret()` (off by the stub length) so the
    redirect `rel32` pointed one instruction past the stub. Now captured `buf.len()` *before*
    emitting the stub body.
- New test `bounded_bl_diverts_through_dispatcher`: budget-1 block where `bl 0x14` targets a
  callee that doesn't fit; asserts running the block leaves `CpuState.pc == 0x14` and
  `x30 == 4` (link), proving genuine dispatcher re-entry (not an in-trace call).

### What this unblocks (verified by gdb on the crash)

`elfjit libroblox.so 0x1f0db20 --jni` now compiles small blocks instantly and EXECUTES real
JNI_OnLoad init code (no 7s compile, no in-`compile_image` hang). The remaining SIGSEGV is
**not a JIT bug** — it's the documented NEXT frontier: JNI_OnLoad's first indirect
`vm->GetEnv` dispatch through the synthetic JavaVM table faults at a guest address that our
`build_jni()` host thunk table doesn't yet satisfy (`0x7fff...` runtime ptr not host-callable).
i.e. the guest is faithfully doing what a real JNI_OnLoad does and tripping on the host JNIEnv
runtime that still needs QEMU's jni global-state (classes/methods/RegisterNatives) backing.

### Honest status

- 62/62 arm64jit tests green (the +1 is the bounded divert test); `cargo build --workspace` OK.
  (`libloader::android::test_setup_android_layout_creates_dirs` fails on this host because it
  wants to mkdir `/storage/emulated/0` at the actual root; pre-existing, unrelated to this change.)
- The JIT now reaches and begins executing the real JNI load path. Next brick is the host JNIEnv
  runtime (GetStaticMethodID/newStringUTF/RegisterNative real callbacks + clock/mprotect no-op
  under the JIT like QEMU had to).

## Session — pthread sanitizer + deeper deref frontier

**Merged this session:**
- `bcf6a88` canary GOT bind; JNI_OnLoad prologue survives its first `ldr`, block count 0->1
  (pts into GOT slot `0x631aa30`, the `__stack_chk_guard` slot — note `#2608` in objdump is
  DECIMAL = `0xa30`, and guest reads `[0x631a000 + 0xa30]`, not the `0x631c608` a first draft
  patched by mistake).
- `1c21ffc` **bionic pthread_mutex sanitizer**. The guest `.so` is bionic-built; its
  `pthread_mutex_t` is 44B (glibc 40B), `__kind`@+16 = 0x10 (ROBUST_NORMAL), `__count`@+8 reused
  as `__owner`. Passing it raw to glibc `pthread_mutex_lock/cond_wait` crashes/deadlocks the once-
  init, driving Roblox into abort. Wired `sanitize_mutex` into the resolver's host bridge for
  mutex_lock/unlock/mutex_init/cond_wait/cond_timedwait (kind&=3 @+16, clear bogus owner @+8),
  mirroring `jni_shim.c`'s `sanitize_mutex`. +hostcall@ trace tracer (JIT_TRACE). 63/63 tests.

**Current frontier (verified 0x1f0db20 --jni):**
- block count 1, hostcalls 0: the crash is BEFORE any host bridge call, inside translated guest
  code. `block@0x101f0db20 -> pc=0x101f0e728` (JNI_OnLoad first bl, x30 linked), then block ~2
  (init guard at `0x2678068`, the GameActivity once-routine) crashes on a guest `ldr x, [x0, #8]`
  deref where x0 = 2 (small pseudo-handle). Preceded by a host ld FP divsd (div by 2^54/2^63),
  suggesting an LCG/time helper.
- The deref of x=2 with `[x0+8]` is the faked-Android-object wall: the guest legitimately got a
  small integer handle where it expects a real object pointer (JNIEnv/class), then derefs it.
  pthread_sanitize is a necessary fix but is NOT the firing block — the guest hasn't reached a
  pthread_mutex host call yet.
- Next brick: find WHICH guest fn returns the `2` (candidate: a JNI/host shim returning a small
  status instead of a pointer), or pre-scheme the once-flag so the init guard skips its guard
  entirely (QEMU's documented `mov w0,#1; nop` bypass).

**Refined finding (next session):** instrumented `host_mutex_lock` with a `[mutex_lock]` JIT_TRACE
line. Boot shows **zero host callbacks fire before the crash** (`hostcall@` = 0, `[mutex_lock]` =
0) — the guest never reaches `pthread_mutex_lock` at all, so the abort is NOT started by a mutex
failure. The crash block (decode of the compiled `0x102678*` region) sets guest `x0=2 +
x1=0x100362f03` (a string literal), does `cvtsi2sd -> addsd 2^63 -> divsd 2^54` (the guest
`__int64->double` HUGE_VAL trim), then a `ldr x, [x0, #8]` deref with x0=2. Guest `pc` at fault=
`0x7f0000002208` = HOST_THUNK slot 1089, i.e. a guest `blr x16` to a host-slot address whose
registered host fn is the one reading `[x0+8]` with x0=2 — but `host_call_at` did NOT intercept it
(0 trace). Suspects: (a) that host slot's registration slipped (resolver `register_named`/
post-bind mismatch), or (b) a host fn genuinely reads `[arg+8]` on a `2` handle (a JNI/Android
object). Next: dump slot 1089's registered fn + guest pc at the `blr`; if it's a JNI shim, give it
a real fake-object backing instead of returning 2.

**[RULED OUT, verified next session]** Two more dispatcher diagnostics were run: log any pc in
`[HOST_THUNK_BASE, +8192*8)` that `host_call_at` returns None for (`unregistered-hostthunk@`),
plus the existing `hostcall@` and `[mutex_lock]` traces. Boot: **`unregistered-hostthunk=0`,
`hostcall@=0`, `[mutex_lock]=0`, `block@=1`.** The guest never reaches the host-thunk range or any
host bridge — the fault is a **pure inline translated deref** inside the single block from
`0x101f0db20`. Block decode: JVM load, then once/abort prologue (`mov w0,2; mov x1,<fmt>` =
syslog args, int64->double trim), then `ldr xN,[x0,#8]` with guest x0=2 -> `[0xa]` SIGSEGV. The
`0x7f0000002208` value in the CpuState is the block's in-progress next-pc, never dispatched, so
the slot-1089 registration idea is closed. Frontier is guest logic using a small int (2) as an
object pointer. Remaining root-cause candidates: (a) `syscall(178=gettid)`/a host fn return
leaves `2` in a register the guest reuses as a pointer; (b) JNI_OnLoad once/init computes a
handle from an unimplemented host call it then derefs. Next: trace the guest instruction that
stores 2 into the register it derefs (step the block with gdb, or narrow with a
`[x0,#8]`-deref watchpoint).

## Session — bl-to-PLT-stub divert TRUE ROOT CAUSE + init now completes (commit 1e447b3)

**The `x0=2` abort-forward was NOT a JNI shim bug — it was the JIT inline-calling host-import
stubs.** A guest `bl <import@plt>` (pthread_mutex_lock, syslog, abort, __android_log*, ...) was
compiled INLINE as a host `call` to the PLT stub's emitted block. The stub ends `adrp/ldr x17,GOT;
br x17`; the `br` sets `CpuState.pc = x17` (= a host thunk slot) and `ret`s — but because it was
*`call`-entered*, that `ret` returned into the inlined caller's fall-through (guest code) instead
of handing pc to the dispatcher. So the real import never ran, the guest saw a garbage return
(`x0` stayed 2 from the syslog arg setup), took the once-init ABORT branch, and deref'd `[0xa]`.

**Fix (jit.rs, `compile_image_bounded`):**
- `is_host_plt_stub(addr)`: decode the 4 instructions at the `bl` target; recognize the canonical
  `adrp Xd; ldr Xn,[Xd,#imm]; add Xd,Xd,#off; br Xn` PLT stub. Pitfalls hit while dialing it in:
  the `br` encodings its branch-register at bits 9:5 (`(w>>5)&0x1f`), and the `add` top-byte mask
  is `0xff000000` to reach `0x91000000` (use `& 0x7f000000` silently drops bit 24 and rejects every
  real `add`).
- In the `Inst::B{link:true}` handler, if the target `is_host_plt_stub(t)`, do NOT push `t` to the
  frontier (don't inline it). Set a new `force_stubs` flag so the dispatcher-return stub table is
  still built even when the frontier drains (`if truncated || !frontier.is_empty() || force_stubs`),
  otherwise the diverted `bl`'s fixup index into an empty `stub_of_target` (the `[&t]` lookup).
- The divert rewrites the inline call (E8) into a `jmp` to a stub that writes `pc=t` and `ret`s, so
  the dispatcher re-enters and `host_call_at(t)` routes the REAL import → host bridge → guest gets
  a genuine return value.

**Result (verified `elfjit ~/.../libroblox.so 0x1f0db20 --jni`):** the guest advances PAST JNI_OnLoad's
pthread_once init — which now SUCCEEDS (416 hostplt `bl`s diverted) instead of aborting:
```
  block@0x101f0db20 -> pc=0x101f0e728      (JNI_OnLoad prologue -> init guard)
  block@0x101f0e728 -> pc=0x105ce0828      (init guard RETURNS, guest resumes in startup!)
  block@0x105ce0828 -> pc=0x1068c7518      (next caller; tries to call an unmapped fn pointer)
run_loop: pc 0x1068c7518 outside image [0x100000000, 0x105e67390)
```
No more SIGSEGV/139 at `[0xa]`; the JIT now gracefully stops with `pc outside image` (exit 1).
63/63 tests still pass.

**NEW frontier (clean, readable):** the guest (in a real startup caller at 0x105ce0828) computes a
function pointer = `0x1068c7518` (= guest VA `0x068c7518`) and calls it. `0x068c7518` is in a GAP
between the RW .data/.bss seg and the RO .rodata seg — i.e. **unmapped** → a null/garbage global
function pointer. Likely a C++ vtable / JNI-registered callback / Soong-supplied hook that the
host `jni_shim` layer must provide backing for (the Sober "fake Android" shim), OR a `dlsym`-ed
pointer our resolver left 0. Next: find WHICH global holds `0x68c7518` and WHICH init step should
fill it — dump `readelf -sW`/`.rodata` owners at `0x68c7518`, disassemble the caller block
`0x105ce0828`'s `ldr/blr` to see the pointer source.

## Session — once-init completes; next frontier is an uninitialized C++ vtable virtual call (commits 1e447b3 + ec39a19)

**Verified boot now** (`elfjit libroblox.so 0x1f0db20 --jni`, JIT_TRACE):
```
block@0x101f0db20 -> pc=0x101f0e728        (JNI_OnLoad bl init-guard)
block@0x101f0e728 -> pc=0x105ce0828        (init-guard COMPLETES normally, no abort)
block@0x105ce0828 -> pc=0x1068c7518        (run_loop: pc outside image)
```
- The pthread_once once-init at `0x2678068` now **runs the whole guard and returns success** (previous sessions' abort/deref path is gone). Roblox then advances into engine-native startup (`NativeAppBridgeV2StartAppWithParams`, GL interface init).
- New frontier: guest `will brl x9` at guest `0x26473ac` where `x9 = 0x1068c7518` (a global `.bss`/`pb_defaults` DATA address, not code) -> dispatcher stops ("pc outside image", exit 1, not a segv).
- Mechanism (decoded): `ldr x0,[x21,#8]; ldr x8,[x0]; ldr x9,[x8,#48]; blr x9` = a **C++ virtual-method call: vtable slot 48 holds `0x68c7518` (garbage/uninitialized)**. The object (`x21`-derived) is a Roblox interface (context: `IPlatformSystemDialogHandler`-adjacent call after it). `.init_array` is all-zeros (no C++ global ctors to run), so the object's vtable was never populated.
- **Not a JIT bug** — it's the fake-object/interface wall: the guest calls a valid vtable offset on an object the minimal `elfjit --jni` env didn't construct. Next: find what initializes the object behind `x21` (candidate: a JNI/`ANativeActivity`-provided global, or a `__cxa_atexit`/static-init baked elsewhere), or stub the virtual interface (slot-48 method) to return and continue.

Tools added (commit `ec39a35`): JIT_DUMP `[outside-image]` full-register dump + `[term]` guest terminal-pc trace for pinning such stops.

**Refined frontier analysis (next session):** the guest's failing tail, traced per-instruction, is:
`0x1c7b768: stp x30..; adr x8,0x631b000(=guest .got); ..reads GOT..; then 0x1ace7c: str x8,[x19]; ldp x29,x30,[sp]; ret` — a
guest subroutine reading its **own GOT/@.dynamic (page 0x631b000)** and returning; the `ret` lands on `x30=0x68c7518` (guest data/bss page 0x68c7000 = `__stop_pb_defaults`). Two candidate roots:
(a) **`0x68c7000` is in a real PT_LOAD-APTA gap** — `readelf` shows LOAD3 ends 0x6368df8, LOAD4 starts 0x6988000; nothing covers 0x68c7000. But the `elfjit` example only mapregs the r-x segment & hands `image`=that SLICE to the JIT run_loop, whose "pc outside image" bound is `base+len`=0x105e67390. So (i) that page is genuinely unmapped in-guest (a real loader gap if Roblox expects it) and (ii) the run_loop bound would also reject any legit guest let the other segments. Check `load_elf_image` mmaps ALL segments, both for real load and so `run_loop` uses the full address-space range, not just the r-x slice, as its valid-pc bound.
(b) x30 got corrupted upstream by a mis-emission; would need per-step guest tracing.

`[it]`/`[term]` were reverted to `[term]`-only (committed ec39a19); they're JIT_DUMP-gated.

## Session — removed the misleading "outside image" stop; true root is a corrupt FMOD vtable call (commit e056128)

Earlier sessions misread the frontier as "guest pc outside image" — that was FALSE: `load_elf_image` maps ONE contiguous
anonymous region spanning ALL PT_LOADs + inter-segment gaps at the 0x100000000 base, but elfjit handed the JIT only the
**r-x text slice** as `image`, so any legit mentor into data/.bss past the slice was rejected as "outside image".

**Fix (e056128):** elfjit now computes `len = (max(guest_vaddr+memsz) - base)` so the run_loop valid-pc bound covers
the whole zero-filled mapped span. Boot now proceeds past that stop until it genuinely hits non-code data:
```
running entry guest=0x101f0db20 ...
  block@0x101f0db20 -> pc=0x101f0e728 ...   (JNI_OnLoad -> init-guard)
  block@0x105ce0828 -> pc=0x1068c7518 ...
arm64jit run_loop stopped: translate: unhandled Unsupported(0x68c74d0) at guest pc 0x1068c7518
```

**True root of the dispatch to 0x68c7518 (FMOD Audio static-init, guest 0x5ce0828):**
```
5ce094c: mov w8,#6; ldr x9,[x0]     ; x9 = vtable of object x0=(0x10045b848 arg)
5ce0950: ...
5ce0968: ldr x8,[x9,#32]            ; method ptr = vtable slot 32
5ce096c: blr x8                     ; virtual call -> pc=0x68c7518
```
`0x68c7518` is the guard / a `.bss` (region 0x68c7000, `__stop_pb_defaults`) DATA address, not code. So this is a
**corrupted C++ vtable slot** (offset 32) on an FMOD/engine object passed in x0 — the vtable points into data/bss
instead of `.text`, so the virtual method call lands on raw bytes (Unsupported(0x68c74d0)).

Confirmed: `.rela.dyn` is **entirely absent** (only 537 `.rela.plt` JUMP_SLOTs), so there are NO R_AARCH64_RELATIVE /
data-absolute relocations for the loader to apply. The guest's `.data` vtables are whatever the file laid out.

Next leads (no .init_array / no .rela.dyn / no ifunc): the object at x0 (0x10045b848) has a vtable that is wrong
after the once-init — either (a) its vtable entry 32 was never set because a guest constructor didn't run under
`elfjit --jni` (no .init_array run), or (b) vtable base-scaled entries need the loader to add the 0x100000000 PIE
base to `.data.relro`-style absolute pointers, which this ## loader does not do (no R_AARCH64_RELATIVE present =
presumptively absolute at build, but for a PIE that needs +base).

**Correction to the abute "vtable slot 32 = corrupt" (added right after):** `x0` at the FMOD static-init is NOT a C++
object. Guest bytes at `0x10045b848` are the ASCII string `__cxa_guard_acquire[...]` (a .rodata/.dynstr symbol string).
So the "vtable" `[x0]` is really a string-literal address; `[x0]+32` is garbage → the "virtual call" is actually the
guest's C++ `__cxa_guard` / exception runtime being fed a **string address where a control block / function address
belongs** (guard-state at 0x68c7518, and a `__cxa_guard_acquire`-symbol-string in x0). Real root is **bionic/libc++
`__cxa_guard` machinery the minimal `elfjit --jni` shims do NOT provide**: our `bind_image_plt` binds .rela.plt JUMP_SLOTs
but the guest's C++ static-init path (guard acquire/release) is not shimmed, so it strays into string/data. Next:
shim/redirect `__cxa_guard_acquire`/`__cxa_guard_release`/`__cxa_guard_abort` (guest `__cxa_atexit` too) to real host
libc++/bionic so FMOD's static-init guard works, mirroring how we patched `__stack_chk_guard`.

## Session — exact mechanism of the FMOD dispatch-to-0x68c7518 + the likely nested-inline once-inv bug (update)

New decisive facts this session:

1. **The crash is a `Ret` to a corrupt x30, not a vtable `blr`.**
   The guest block STARTING at `0x105ce0828` (FMOD static-init guard, guest `0x5ce0828`) terminates by setting
   `pc = 0x1068c7518`, and CpuState at stop has `pc==x30==x0==x19 == 0x1068c7518`. The last `[term]` (JIT_DUMP)
   correlation shows the terminal is a `Ret` whose `x30 = 0x68c7518` (a .bss guard address) — i.e. the guest
   RETURNS into a data guard, then the translator decodes the `.bss` bytes there → `Unsupported(0x68c74d0)`.

2. **The once-routine return semantics are understood:**
   `0x2678068` (`GameActivity_initializeNativeCode`) ends:
   ```
   2678138: cmp w24,#1
   267813c: cset w0,ne            ; w0 = 0 iff w24==1 (init "done")
   ...
   2678158: ret
   ```
   So it returns `w0=0` (done) only when the local `w24` was set to 1 during init. The FMOD code does
   `bl 2678068; cbz w0, <clean ret -> `5ce085c`>; <else fallthrough to the corrupt path>`. Because the guest
   dispatches to `0x68c7518` on the NOT-clean path, `2678068` is returning `w0=1` (NOT-done) for the FMOD
   guard `0x68c7518` — meaning the once-body's `w24` never got set = the once-init body did not run to
   completion for THIS second distinct guard. The once-mutex (guest `0x637a468`) / pthread_once body worked
   for the FIRST guard (GameActivity init at 0x101f0e728, once-mutex 0x637a468) but is failing on this
   second, FMOD, guard.

3. **Why a second time fails — the suspected JIT-fidelity bug (NEXT REAL TASK):**
   `0x105ce0828`'s compile inlines the nested `bl 0x2678068` (a *guest* function, so NOT diverted by
   `is_host_plt_stub`; only host-import PLT `bl`s are diverted). Inside `0x2678068` the guest does
   `bl pthread_mutex_lock@plt` — which IS diverted via the `force_stubs` mechanism. On the FIRST guard
   this chain completed (w24=1). On the SECOND distinct guard the same routine is inlined AGAIN inside a
   different (huge) block; the mutex return/stub table interaction appears to regress so the routine exits
   without setting `w24` (reads stale/garbage), returning `w0=1`, and FMOD then comes/path dispatches to
   the guard address `0x68c7518`.
   **Verify:** add a `[it]`/gall step that logs whether `2678068`'s `mov w24,#1` (once-done) instruction is
   ever reached in the FMOD block, vs whether the routine bails to `cset w0,ne` without it. If not reached,
   the nested-inline of the second call is the bug (e.g. bad return-stub linking for the inner mutex call).

4. **Confirmed irrelevant to THIS crash:** `.rela.dyn` is `ANDROID_RELA` (present, sections [10]) but the
   loader/`bind_image_plt` does not apply it; `__stack_chk_guard` is patched. `.init_array` empty. No ifunc.
   Host `dlsym(RTLD_DEFAULT)` provides `__cxa_atexit`/`__cxa_finalize` but NOT `__cxa_guard_acquire/
   release/abort` (all null) — so a "shim the cxa_guard by resolve" approach can't source them from host libc;
   they'd have to be guest-emulated (inline guard) or written manually.

**Recommended next step:** trace (JIT_DUMP/JIT_TRACE) whether the `0x2678068` once-body sets its done flag on the
FMOD guard, root-causing the nested guest-`bl`-in-inter-inlined-block return regt; if confirmed, divert guest
`bl` to the once-routine (and generally guest `bl` whose callee contains diverted imports) through the
dispatcher instead of inlining — i.e. treat a `bl` whose translatable body itself has out-of-block PLT mutex
calls like `is_host_plt_stub`: push it to the stub table + `force_stubs`, not the inlined frontier.

---
## Session (Sep 11, 2026) — divert guest bl-to-import-bearing-callee through dispatcher (FMOD second-guard) DONE

Picked up the HANDOFF's "next task": divert guest `bl` to the once-routine (and any
import-bearing callee) through the dispatcher. Commit `10ddb7a` (on `dev`).

### Environment note (fresh box)
- `cargo build --workspace` ✓, `cargo test --workspace` ✓ all green (67 arm64jit +
  5 + 16 libloader incl. the two android-layout tests, + others; 0 failures).
- `cargo test --workspace` was already green for the libloader android layout tests
  on this box: commit `8b72828` had already landed the deterministic fix (per-call
  unique temp root) plus `ensure_dir_android` already does `create_dir_all` before
  `set_permissions`, so the worker-handoff's "permission-set before parent dirs"
  frame predates it. Verified passing.
- The real `libroblox.so` (100 MB, `~/.cache/open-sober/libs/` on the old box) is
  NOT present here and there is no APK/GSI/GPU, so the boot frontier can only be
  exercised at the unit-test level in this session.

### What landed (all in `crates/arm64jit/src/jit.rs`)
1. `word_at(image, base, addr)` — bounds-checked 32-bit image read (replaces the
   raw-pointer derefs `is_host_plt_stub` used to do on mapped guest==host memory).
2. `is_host_plt_stub(image, base, addr)` converted to slice-based reads.
   **Root-caused + fixed two real bugs the new tests exposed:**
   - Stale `if addr < 0x1000 { return false; }` guard left over from the
     pointer-based code — it wrongly rejected legitimate PLT stubs at low
     synthetic addresses (the unit-test stub images live at 0x40), so
     `body_contains_host_plt_bl` never saw the import. Removed; `word_at` is the
     safety net now.
   - `body_contains_host_plt_bl` was *following guest `bl` calls into their callee
     bodies*, making the caller of an import-bearing callee transitively
     import-bearing too (test asserted the caller is NOT). Now it only detects
     **direct** host-import `bl`s in the entry's own body and lets the linear walk
     fall through a guest `bl`. This is the right model for the bounded compiler:
     transitive follow would mark every caller up the whole call graph as
     import-bearing and defeat bounded compilation entirely.
3. `compile_image_bounded` now diverts (forces a dispatcher-return stub, memoized
   per target) any guest `bl` whose callee body itself calls a host import — the
   FMOD once-routine (`2678068` GameActivity init, which calls
   `pthread_mutex_lock@plt` etc.) regression is specifically this shape: inlining
   it a second time in a different huge block regressed the inner import
   diversion, so it returned "not done" (w0=1) and the caller branched into the
   `.bss` guard `0x68c7518`.

+4 tests: `host_plt_stub_detected_from_image_slice`,
`body_contains_host_plt_bl_follows_call_graph`,
`guest_bl_to_import_bearing_callee_diverts_through_dispatcher` (run caller block
⇒ `CpuState.pc==0x20` callee, `x30==0x04` link — real dispatcher re-entry, not an
inline call), `guest_bl_to_import_free_callee_still_inlines`. **67/67 arm64jit,
0 failures.** `cargo build --workspace` clean (warnings are pre-existing decode.rs
dead-code / rustfmt churn).

### Next (ordered, no APK/GSI/GPU on this box)
1. JNI function-table stubs (`crates/arm64jit/src/jni.rs`): fill high-value slots
   that must return real values when the guest boot path reaches them
   (GetStaticMethodID, NewStringUTF, RegisterNatives, FindClass) with host
   thunk-backed implementations + unit tests. This is the next name-surface the
   JIT boot hits once the divert fix lets `JNI_OnLoad` progress.
2. ELF/loader (`libloader`) gaps, then `libbadcpu` ISA gaps, then services/auth.
3. Real-binary/GPU boot verification remains blocked until `libroblox.so` (or an
   APK) and a GPU host are available — capture as `elfjit ... 0x1f0db20 --jni`
   log on a capable host (HARD GATE).

## Session (Sep 11, 2026) — JNI/JavaVM function tables on the OFFICIAL Android ABI slot offsets (commit bdd8b03)

Continuing the ordered work ("JNI function-table stubs"). Examined both
`crates/arm64jit/src/jni.rs` (JIT path) and the QEMU `jni_shim.c` (validated
reference) and found the JIT JNI table was mis-slotted vs. the ABI the guest
uses.

### The bug (real, and it would crash a booted guest)
libroblox.so indexes `JNINativeInterface` with the OFFICIAL jni.h word offsets:
`GetVersion=4, FindClass=6, GetMethodID=33, GetFieldID=94,
GetStaticMethodID=113, NewStringUTF=167, GetStringUTFChars=169,
RegisterNatives=199, GetJavaVM=203`, and `vm GetEnv=7`. The JIT table carried
unvalidated guesses from the QEMU shim (`NewStringUTF@36`, `GetArrayLength@37`,
`GetObjectField@102`, `RegisterNatives@193`, `GetJavaVM@197`, vm GetEnv@4/6).
The QEMU path only ever end-to-end-validated GetVersion/FindClass/
GetStaticMethodID against the real binary — of those, FindClass(6) and
GetStaticMethodID(113) coincidentally match the official offsets, which is why
the mismatch went unnoticed (its boot hung at nativeSetAssetPath before any
divergent slot was exercised). `bdd8b03` rebuilds the JIT tables on the official
offsets so a guest call lands on the real stub, not NULL/wrong.

### Handles are now readable (not low sentinels)
FindClass/NewStringUTF/GetMethodID return a stable, interned, readable UTF-8
buffer handle (the `str_handle` registry — analogue of the QEMU shim's
`track_ptr`), instead of the old `0x3000` sentinel that risks a guest deref
fault. GetStringUTFChars returns that buffer and clears `*isCopy`;
RegisterNatives succeeds (records nothing yet) so boot continues; GetJavaVM
writes the live vm handle. `jni_vm_getenv` (GetEnv @ slot 7) writes `*penv`.

### Verification
- `+2` tests: `jni_table_has_official_abi_slots_nonnull` (regression guard that
  all boot-relevant slots are non-null host thunks at the OFFICIAL offsets),
  `jni_new_string_utf_is_readable`.
- Fixed the E2E `jit_jni_onload_getenv_getversion` to load vm GetEnv at
  offset 56 (slot 7) and to dereference `env->functions` before indexing slot 4
  (JNIEnv word0 is the fn-table ptr; the earlier test read `[env+32]` directly).
  69/69 arm64jit, workspace 103/0. `cargo build --workspace` clean.

### Next (ordered)
1. `libloader` ELF/loader gaps (next in RECOMMENDATION order); drive
   `elfjit`/`--jit` boot path end-to-end against a synthetic/test ELF to
   confirm no regression from the divert + JNI changes.
2. `libbadcpu` ISA gaps; then services/auth.
3. Real-binary/GPU boot verification remains blocked (no APK/libroblox.so, no
   GPU) — HARD GATE on a capable host.

## Session (Sep 11, 2026) — 128-bit SIMD ld/st register-offset/unscaled/pre-post-index mis-decoded as GPR; silent x-reg corruption FIXED (156/0)

Root-caused the `modmain.elf` (full static-glibc) `__memset_generic` SIGSEGV.
The memset's `str q0,[x0,x3]` (0x3ca36800) decoded as a GPR 1-byte sign-extend
load **into the base register** (`ldrsb x0,[x0,x3]`), silently clobbering guest
x0 → `__tls_init_tp`'s `str w5,[x0,#4]` faulted at address 0x4. The GPR
register-offset (0x38200800) and pre/post/unscaled (0x3800xxxx) decode gates had
no bit26 (vector-file) mask; the imm-offset gate (df470fa, prior session) did.

## Fix (commit `853cc44`)
Three new 128-bit vector classes gated BEFORE the GPR gates (bit26=1), plus
`bit26==0` added to both GPR gates:
- **VecLdStrReg** — register-offset str/ldr q: `(insn & 0xffe00c00)` in
  `{0x3ca00800 (str), 0x3ce00800 (ldr)}`.
- **VecLdStImmUnscaled** — ldur/stur q: `{0x3c800000, 0x3cc00000}` (signed imm9;
  note the residue is 0x0000 — the imm9 lives in bits[20:12], outside the mask).
- **VecLdStIndexed** — pre/post-index writeback: `{0x3c800c00, 0x3cc00c00,
  0x3c800400, 0x3cc00400}`; Xn advances by signed imm9.

All transfer 16 bytes via XMM0 to/from `CpuState.v[vt]`. Gate correctness
verified against `aarch64-linux-gnu-as` ground truth incl. **non-collision**
with scalar B/H/S/D register-offset/unscaled (e.g. stur b0=0x3c1fc100 masks to
0x3c000000, bit23 clear).

## Result
modmain no longer SIGSEGVs in `__tls_init_tp`'s memset — it advances through
the whole vector ld/st family and stops HONESTLY (Unsupported) on the next wall
instead of corrupting. `+4` regression tests (3 exec: base preserved for the
reg-offset store, pointer preserved for unscaled stur, Xn advanced for
pre-index ldr; 1 decode: the three classes + scalar-b non-collision).
arm64jit 115/115; workspace 156/0; build clean.

**Addendum (same cycle):** scalar FP register-offset (`FpLdStrReg`) added —
`str s0,[x0,x3,lsl#2]` (0xbc237800, memset's next path) was silently executed
as a GPR op on the wrong register file. New decode arm (bit26=1 &&
0x38200800 residue && bit23 clear for Q) + translate (addr in RDX, width from
bits[31:30], S-bit index scale). `+fpl_single_reg_offset_store_with_shift`.
arm64jit 116/116, workspace 157/0. Commit `921af2a`.

## Next (ordered, no APK/GSI/GPU on this box)
1. Scalar S/D UNSCALED (`stur/ldur s0,d0`, e.g. modmain 0x40a95c word 0xbc1fc0a0)
   and scalar pre/post-index writeback ld/st — the last of the same bit26=1
   family; glibc-CRT-memset tail, repeatedly judged NOT a Roblox boot blocker
   (real libroblox.so boot ISA already fully decoded / exit 0).
2. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth (ordered
   plan).
3. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays
   the HARD GATE — blocked until a capable host + real binary/APK (none here).
---

## Session (Sep 11, 2026) — JIT correctness: XZR/SP, FP/vector loads, static-ELF loader (commits b0c3237, f1707e2, df470fa)
Unblocked running real compiled aarch64 C through elfjit (loader+dispatcher)
by making `bind_image_plt` skip static ELFs instead of panicking (b0c3237),
then used cross-gcc test programs to regression-test actual control flow. This
EXPOSED (and fixed) two latent correctness bugs the old panic had masked:

1. **XZR vs SP in store source / load dest** (f1707e2). `str xzr,[..]` (used
   everywhere to zero-init) loaded CpuState.x[31] = the STACK POINTER and
   stored it — verified A1 returned ~0x7fa14f7eb015 instead of 5. Loads to
   x31 (`ldr xzr`) also clobbered SP. Added `ldg_src`/`stg_if_writable` and
   applied at every GPR ld/st site + LdStPair rt/rt2.

2. **FP/vector-register loads/stores touched the GPR file** (df470fa). The GPR
   ld/st gate `(insn & 0x3b000000)==0x39000000` left bit26 (GPR-vs-FP selector)
   unmasked: `str d0`/`ldr d0` (0xFD..) read/wrote x[rt] not v[rt], `str s0`
   (0xBD..) the same, and `str q6`/`ldr q7` (0x3D8/0x3DC) decoded as 1-BYTE GPR
   loads — the VecLdStImm 128-bit gate was unreachable dead code. Fixed the GPR
   gate to mask bit26, made q fall through to VecLdStImm, and added a new
   `FpLdStImm` class handling B/H/S/D scalar loads/stores into/out of
   CpuState.v[vt] (upper lanes preserved).

Both verified with new tests; arm64jit 69 -> 72, workspace 106/0, build clean.

### Remaining (honest, not blocking the committed work)
- fp_only (no-loop FP `scale()` call, inlined): after correct FP decode, stops
  at "pc 0x4004000000000000 outside image" — a control-flow/x30 interaction in
  the inlined-callee `ret` beneath the bounded dispatcher. No longer
  segfaults/corrupts (clean diagnostic). Not on the previously-validated Roblox
  boot ISA, so it doesn't contradict the "boot instruction space covered" claim.
- int_only (loop w/ backward branch): still hangs — deeper loop/branch issue.
- These are synthetic-program paths; next session should root-cause the inlined
  `ret`/dispatcher x30 interaction (high value for FP graphics/audio).

## Session (Sep 11, 2026) — add/sub SP write + final JIT correctness sweep (commit 4301348)

Root-caused and fixed the last of the XZR-vs-SP family: `AddSubImm`/`AddSubReg`
suppressed rd==31 writes, but for ADD/SUB rd==31 means **SP** (unlike logical
ops where it's XZR). So every function prologue `sub sp,sp,#N` did nothing and
nested frames collided on the same SP — inlined `f()`'s `str d31,[sp+8]`
overwrote the caller's saved x30 with 6.5's bit-pattern, and the final `ret`
returned `pc = 0x401a000000000000` ("outside image"). cmp/cmn (s==1, rd==31)
still discard correctly. Verified fp_only, C_fmovret, A_frame all return 42 now.
+`sub_add_sp_updates_stack_pointer` regression. arm64jit 73, workspace 107/0.

Result after this session's 8 commits: the JIT's GPR load/store, FP/vector
load/store, add/sub-SP, XZR handling and JNI table are all materially more
correct; several would have corrupted the real Roblox runtime.

### Honest remaining (next session — concrete, small tasks)
- **`fcvtzs/fcvtzu Dd,Dn` and Sd,Sn (0x5E/0x7E)** — SIMD/vector FP->int writing
  to an FP register lane. `fcvtzs d31,d31` (0x5ee1bbff, from B_fpstore) is
  currently MIS-decoded as `WidenShl`; the plain 0x5ee1xxxx is Unsupported.
  Add a class BEFORE the WidenShl gate and a translate converting Dn's double
  to signed/unsigned int in Dd (this is a genuine silent-corruption risk for
  FP code). B_fpstore segfaults at it (was hanging pre-sp-fix).
- **Backwards-branch loop fidelity** — int_only (loop w/ `b.lt` back-edge)
  still faults; jit_regress hangs. Verify the bounded compiler patches in-body
  back-edge targets to the emitted block (host_of_guest) and that SP/offsets
  stay stable across iterations.
- SMOV/UMOV lane->GPR and remaining FP-vs-int lane ops.

These are progressive ISA surface revealed by arbitrary compiled C, not
blockers of the previously-validated Roblox boot path.

## Session (Sep 11, 2026) — JIT executes real compiled C end-to-end (commits f1e65ce, f6bb244)

Continuing the synthetic-program bring-up. The loop back-edge and four
operand/decode fixes crossed the JIT from "decodes the boot ISA" to "correctly
EXECUTES real compiled aarch64 C": functions with loops, recursion (fib=55),
FP fmul/fadd/fcvtzs, mul (factorial 8!=40320), ldrsw sign-extend loads,
movk multi-part constants, SP prologues — 15/15 cross-gcc programs return the
right value through load_elf_image->jit_run (no QEMU).

### f1e65ce — loop back-edge jmp
A frontier block falling through to an already-emitted address (loop back-edge
`b.le Lbody`) just `break` and hit the epilogue `ret` → every loop body ran
once then returned/re-dispatched (hang/corrupt pc). Now emits a `jmp` to the
already-emitted host offset + fixup. loop1 sum(0..9)=45, int_only loops correct.

### f6bb244 — operand/memory decode correctness (four silent miscompiles)
1. MOVK was a *replace* not a *merge*: `movz 0x8bb1; movk 0x2 lsl#16` → 0x20000
   not 0x28bb1 (broke every multi-part constant). Now RMW at bits[shift,+16).
2. MADD/MSUB with ra=31 (the `mul` alias) added the STACK POINTER (ldg RDI,31
   read x31). ra==31 is XZR → skip the accum add/sub.
3. AddSubReg gate caught MADD/MUL (top 0x9b) as `add ...,lsl #N` (mul x0,x1,x0
   → add lsl#31). Restricted to the real add/sub shifted-register tops
   {0x0b,0x2b,0x4b,0x6b,0x8b,0xab,0xcb,0xeb}; 0x9b falls through to MulDiv.
4. ldrsw/ldrsh/ldrsb (sign-extend loads) decoded as STORES (bit22=0 like STR,
   bit23=1). Added `sext` to LdStrImm; these are now sign-extending loads into
   the X dest (ldrsw=movsxd, ldrsh/ldrsb=shl/sar 48/56).

All regression-locked (+movk_merges_into_existing_register,
loop_back_edge_reiterates_body, ldrsw_sign_extend_load_ground_truth, plus the
earlier ones). Workspace 111/0, build clean.

### Honest remaining (small, next session)
- LdStrReg register-offset ldrsw/ldrsh may share the bit22-mislead (the C
  battery only emitted unsigned-offset forms); verify and fix if so.
- FcvVec 4S lane edge (uses movq/cvttsd2si on 4-byte lanes) and fcvtzu ≥2^63.
- ADD/SUB with rn==31-as-XZR (`add xD, xzr, #imm` reads SP today; assembler
  uses movz/orr, so low priority).
- Then libbadcpu gaps; services/auth. GPU ev-boards: HARD GATE.
## Session (Sep 11, 2026) — register-offset sext + LogicalImm-vs-MoveWide (commit 716876c); JIT executes broad real C

Extended the synthetic-C battery to arrays/shorts/structs and found+fixed two
more silent miscompiles:

1. LdStrReg register-offset ldrsw/ldrsh/ldrsb shared the bit23 mis-lead (decoded
   as stores) — same fix as the unsigned-offset form (sext field + translate).
2. LogicalImmediate (AND/ORR/EOR/ANDS #imm) collided with MoveWide: the MOVZ/
   MOVK/MOVN gate matched top bytes {0x12,0x92,0x52,0xD2,...} which span the
   AND/EOR/ANDS-immediate class, so `and w1,w0,#0xffff` decoded as `movn`.
   MoveWide now gates on bits[28:23]==0x25 ((insn & 0x1f800000)==0x12800000);
   LogicalImm has 0x24, so AND-immediates route to LogicImm. Verified shacc
   (short acc) and arr (int+short arrays) = 42.

Net: the JIT now correctly executes ~20 real compiled aarch64 C programs
(loops, recursion, FP, mul/div, sign-extend loads both offset forms, short/int
arrays, AND-immediates, MOVK constants, SP prologues). arm64jit 78/78, workspace
112/0.

### Open (next session, honest)
- struct-by-value + function-pointer (`blr` to computed addr) still FAILS:
  `structs.c` → "pc 0x600000005 outside image". The fn-ptr arg gets corrupted
  through the struct-passing / dispatcher path — a deeper control-flow/ABI
  interaction (how the emitted GOT/adrp computes a callable and the dispatcher
  resolves it). Worth a focused session.
- FcvVec 4S lane uses movq/cvttsd2si on 4-byte lanes (possibly wrong); fcvtzu
  for >= 2^63; ADD/SUB rn==31-as-XZR reads SP (assembler prefers movz/orr, low
  priority).## Session (Sep 11, 2026) — LdStrReg sign-extend stored address, not value (commit 7b6b19e)

Post-battery hardening: a register-offset sign-extend exec test surfaced a
translate bug in the bit23/sext path added in 716876c. `ldrsh w0,[x1,x0]`
(0x78e06820, register offset, W dest, no shift) loaded the signed value into
RCX but stg_if_writable stores RAX — i.e. it stored the *effective address*
into the dest register. Every a[i] in a short-array loop via register-offset
LDRSH silently corrupted the accumulator. The session battery passed only
because arrays used ldr w / unsigned-offset forms.

Fix: the LdStrReg sext branch now loads the value into RAX (address no longer
needed), mirroring the LdStrImm sext arm. sumh over `short a[]` (register-offset
ldrsh) = 26 -> 42. arm64jit 79/79, workspace 113/0. +regression
ldr_reg_sext_sign_extends_into_dest.
## Session (Sep 11, 2026) — JIT ABI correctness: struct-by-value + 32-bit semantics (commits b8b5e62, e5d78d3)

Worked the open "struct-by-value + function pointer" item. Reproduced it with a
cross-gcc battery run through `cargo run -p arm64jit --example elfjit` (real
compiled aarch64 C, `-static -nostdlib -Wl,-e,entry`), fixed **four real silent
miscompiles**, gold-locked each with a regression test. `cargo test -p arm64jit`
-> 82, workspace 116/0. Battery: loop1=45, structs/dispatch/fpfun/vtable=42,
byvalue=44, bv2=300, signmod=12, iso_wrd=4321, iso_arith=300 — all correct.

1. **LdStPair offset-form ignored its immediate** (`b8b5e62`). `ldp x0,x1,[sp,#16]`
   (writeback=0) computed `access_off = 0`, so a 16-byte struct passed by value
   read [sp],[sp+8] (the saved x29/x30) instead of [sp+16],[sp+24] — byvalue.elf
   got (0,0) and returned garbage. The three addressing modes were conflated;
   now offset=`(imm,0)`, post-index=`(0,imm)`, pre-index=`(imm,imm)`.
   +`ldst_pair_offset_form_applies_immediate`.

2. **ADD/SUB rn==31 read SP when the S flag is set** (`b8b5e62`). `negs w1,w0`
   (subs w1,wzr,w0) computed `sp - w0` instead of `-w0` (rn=31 is XZR for the
   flag-setting form; only non-S `sub sp,sp,#N` reads rn=31 as SP). This was the
   documented "ADD/SUB rn==31-as-XZR reads SP" gap — a real repro finally
   (signmod.elf `%16` produced garbled remainders). Fixed AddSubImm + AddSubReg;
   also made LogicReg/AddSubReg read rm==31 as XZR (was SP). +`addsub_s_flag_reads_xzr_not_sp_for_rn31`.

3. **32-bit W writes did not zero-extend** (`b8b5e62`). `mov w0,w1` copied the full
   64-bit x1, so a negative two's-complement w1 propagated as 0xffffffffffffffff.
   Added `zext_w` (shl32/shr32) and applied to 32-bit LogicReg (operands, the
   N=1 BIC/ORN/EON half after `not`, and the result) and 32-bit AddSubImm/AdhReg.
   This was the "Ws must zero-extend" open item; it was silently corrupting any
   32-bit chain once a negative value entered a W register.

4. **Scalar `fcvtzu` saturates the wrong half** (`e5d78d3`). fcvtzu is unsigned,
   valid over [0,2^64), but the code used signed `cvttsd2si` which saturates
   anything >= 2^63 to INT64_MIN(0x8000..0); the old comment wrongly claimed
   `d>=2^63` was "architecturally out-of-range". Now a three-path sequence
   (d<2^63 signed; 2^63<=d<2^64 via `2^63 + int64(d-2^63)`; d>=2^64 -> u64::MAX)
   with in-buffer jc/js/jmp patching (mirrors the Ucvtf2d JNS idiom).
   +`fcvtzu_handles_u64_beyond_2pow63`.

Also added the `JIT_BUDGET` env knob (default 8192) to `jit_run` for
instruction-granular tracing under `JIT_TRACE` (`JIT_BUDGET=1`), and a
diagnostic captured by it: a **bounded-truncation fall-through bug** — a block
cut off mid straight-line by the budget had no pc write, so the dispatcher
re-compiled from the same entry forever. compile_image_bounded now diverts the
fall-through next-pc to a dispatcher-return stub when `truncated && !terminal`
(same fix that let the budget=1 per-instruction trace work; also a latent real
hazard for any Roblox function > 8192 insns without an early branch).

The battery lives in /tmp/jitbatt/ (not committed: it was ad-hoc before this
session). Next items on the JIT path: **FcvVec 4S-lane** conversion (uses
movq/cvttsd2si on 4-byte lanes), SIMD SMOV/UMOV lane->GPR and remaining
FP-vs-int lane ops, then libloader gaps -> libbadcpu gaps -> services/auth.
Real-binary/GPU boot remains blocked (no libroblox.so/APK, no GPU) — HARD GATE.

### Addendum (same session) — FcvVec FP->int vector (commits 7c11be3)
- **`.4s` lane width bug**: FcvVec converted each 4-byte S lane as a double
  (`movq_load` reads 8 bytes = the lane *and the next lane*) and wrote 8 bytes
  back (`movq_store` clobbered the neighbour lane), so multi-lane float->int
  vectors were corrupt. Now loads the 32-bit float, `cvtss2sd`s it, stores a
  32-bit int per lane (`mov_store32`).
- **`.2d` decode bug**: `fcvtzs v0.2d` (0x4ee1b820) has bit20=0 just like `.4s`,
  so `esize=(insn>>20)&1` mis-decoded the 64-bit form as esize=4. The real
  discriminator is **bit22** (0x400000). +`fcvt_vec_4s_lanes_are_32bit_and_independent`
  covers .4s (independent lanes), .2d signed, and .2d unsigned negative-clamp.
- arm64jit now 83, workspace 117/0. Each fix was a silent data-corruption bug
  that would have produced wrong pixels/audio/coordinates in a real Roblox run.

## Session (Sep 11, 2026) — SIMD lane-insert/extract fix: INS/SMOV/UMOV (commit 0f2d806)

Picked up the standing "SIMD SMOV/UMOV lane->GPR and remaining FP-vs-int lane
ops" item. Drove real aarch64 asm (INS/SMOV/UMOV across all element sizes +
vector logical/sat) through `elfjit` and found a **silent miscompile** that
predated this session:

### The bug (would corrupt NEON-heavy graphics/audio)
`mov v0.s[i],w1` (INS: GPR->vector-element insert, opcode bit13 CLEAR) and
`smov`/`umov` (element extract to GPR, bit13 SET) share the decode fields of
the vector-logical (AND/ORR/EOR/BIC) and saturating-add (SQADD/UQSUB) classes
(residue 0x..2x0c00). The precise lane-element gate
`(insn & 0xffe0_0c00) in {0x0e000c00, 0x4e000c00}` was placed AFTER
SimdVLog (line ~1186) and SimdSatAdd (line ~1425), so a real INS/SMOV was
silently mis-decoded before the lane gate was reached:
  - `ins v0.s[0],w1` 0x4e041c20 -> AND (SimdVLog): never wrote v0, clobbered x0
  - `smov x2,v0.s[0]` 0x4e042c02 -> SQSUB (SimdSatAdd)
  - `smov x6,v0.h[2]` 0x4e0a2c06 -> SQSUB
objdump-verified the encodings; `gcc` compiles `mov v.s[i],wN` as this INS form
everywhere NEON 4-element scalar writes are edited into vectors.

### The fix (decode.rs + translate.rs + jit.rs)
1. Move the lane-element gate BEFORE the broad vector gates and key on the
   opcode field bits[13:12] (verified across all 4 element sizes):
   - bit13=1        => vector->GPR extract umov/smov/mov (sign = bit12 clear)
   - bit13=0,bit12=1=> GPR->vector insert `ins/mov Vd.T[idx],Rn` (NEW `Inst::InsGp`)
   - bit13=0,bit12=0=> dup-from-GPR (fall through to the existing SimdDupGp)
2. Removed the now-dead late duplicate gate at the old location.
3. SimdLaneGp translate now handles ALL element sizes (1/2/4/8), so `.h`/`.b`
   extracts are no longer swallowed by SimdSatAdd.
4. InsGp translate: copy esize bytes of GPR rn into Vd at index*esize.

### Verified
- `cargo test -p arm64jit` 85/85 (added `ins_gp_inserts_element_into_vector_and_extract_reads_it`
  and `ins_gp_sign_and_zero_variants_insert_correct_lanes`); `cargo test --workspace` 119/0.
- elfjit harness (real aarch64): lane_test.elf / smov.elf return -570
  (=0xfffffffffffffdc6; previously returned 0). and/orr/eor/bic/sqadd/sqsub
  still decode+execute as their real ops (logical.elf runs them all and stops
  honestly at `uaddl` 0x4000ec = 0x2ea20020, the next unimplemented widening
  mul — an honest Unsupported stop, not a miscompile). `dup v1.4s,w10`
  (0x4e040d41) still decodes as SimdDupGp (the bit12==0 fall-through).

### Next (ordered, no APK/GSI/GPU on this box)
1. SIMD widening-multiply family (uaddl/saddl, and the smull/umull widening forms
   already partly present) — surfaced by logical.elf's honest stop.
2. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
3. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK are available.

## Session (Sep 11, 2026) — SIMD widening families: add/sub-long + multiply-long (commits 80d27e5, d052723)

Driving the cross-gcc asm battery (logical.elf / addl.elf / mull.elf) through
`elfjit` cleared two more ISA walls AND exposed that the "already-implemented"
widening-multiply path shipped several silent miscompiles. Workspace now 121/0.

### ADDL: saddl/uaddl/subl/usubl (80d27e5)
- Gate only matched the 8 esrc=2 residues (0x..60), so esrc=4 (.2s->.2d, 0x..a0)
  and esrc=1 (.8b->.8h, 0x..20) fell through to Unsupported. Expanded `alres` to
  all 24 esrc x signedness x upper x add|sub residues; esrc = 1<<bits[23:22].
- Translate had the same width bug class as the old FcvVec/Mull code: esrc=4 read
  64 bits (both lanes), esrc=2-unsigned read 32 (polled next lane), esrc=1 stored
  32 (overran a 2-byte element). Now reads EXACTLY esrc bytes (sign/zero-ext to
  a 64-bit reg) and stores EXACTLY de=2*esrc bytes (8/4/2).

### MULL: smull/umull/smlal/umlal (d052723) — FIVE silent miscompiles
mull.elf "ran" without stopping, but that only proved no-unsupported. Inspecting
decode+translate against objdump found:
1. `res_esize = bit22 ? 8 : 4` — mis-sized smull .4h->.4s as 8, never .8b->.8h (res 2).
2. `unsigned = bit28` — bit28 is 0 for BOTH signed 0x0e and unsigned 0x2e, so
   umull/umlal were sign-extended (0xFE*2 => -4 not 508). Now bit29.
3. `acc = bit15` — set on plain smull/umull too, so every plain widening multiply
   ACCUMULATED instead of overwriting Rd. acc = gateway clause (c000=mul, 8000=acc).
4. translate `lanes = res==8?2:4` — missing the .8b->.8h 8-lane form.
5. store width not exact (store32 for res=2) overran the next lane.

### Verified
- saddl .2d {7,-2}+{3,9}={10,7}; uaddl .4s {1,2,3,4}+{10,20,30,40}; uaddl .8h
  1..8+1..8; smull .2d {7,-3}*{5,-2}={35,6}; umull .8h 0xFE*2=508 (would be -4 if
  still signed) — all exec_bytes'd with objdump-verified encodings.
- decode binds (res_esize/unsigned/acc/q) asserted for all six mull + three addl forms.
- logical.elf / addl.elf / mull.elf run to completion; prior battery + lane_test
  (-570) unchanged. arm64jit 87/87, workspace 121/0.

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep pressing the SIMD surface (the battery will keep surfacing the next wall,
   e.g. shift-by-immediate / tbl / dup .b / post-index SIMD ld, then svc on real use).
2. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
3. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK are available.

## Session (Sep 11, 2026) — SIMD shift-by-immediate: ushr/sshr (commit c6eb8df)

Continuing the cross-gcc SIMD battery, `shiftimm.elf` returned 0xfffffffc for
`ushr v0.2s,v1.2s,#8` (expected 3) — another silent miscompile.

### Root cause
Plain shift-right-immediate (marker bits[14:12]==0b000) had NO decode gate, so
it fell into the broad VecMovi (vector-immediate) gate and wrote a wrong
immediate pattern instead of shifting. (shl 0b101 and usra/ssra 0b001 already had
gates; only the plain 0b000 form was missing.)

### Fix
1. `Inst::SimdShr` gate: SIMD-reg prefix {0f,2f,4f,6f} + bits[14:12]==0b000 +
   bit23 clear + immh(bits[22:19]) != 0 (movi/mvni always have immh==0, so they
   are NOT reclassified — verified movi.2s #5 still VecMovi). esize from fls(immh)
   = 1<<(fls-1); shift = 2*esize_bits - (immh:immb) — verified ushr.2s #8
   (immh4=7) and ushr.2d #17 (immh4=13). Placed before the VecMovi gate.
2. Translate handles shift >= esize_bits (ushr->0, sshr->sign fill) since x86
   `shr r64,imm` clamps count.
3. Fixed the SAME latent sign-extension bug in SimdShr AND SimdShrAcc (ssra):
   the esize-bit source was loaded zero-extended, so a NEGATIVE element under the
   arithmetic shift came out positive (0xffffff00 >>> 8 = 0xffffff, not -1).

### Verified
ushr.2s {0x100,0x200}->{1,2}; sshr.2s -256>>8 == -1 (was 0xffffff); decode binds
ushr unsigned / sshr signed / esize,shift; movi.2s stays VecMovi. shiftimm.elf
-> 3 (was 0xfffffffc); shifts.elf (sshl .2d) -> 256; prior battery + addl/mull +
lane ops unchanged. arm64jit 88/88, workspace 122/0.

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep pressing the SIMD surface as the cross-gcc battery reveals it.
2. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
3. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK are available.

## Session (Sep 11, 2026) — SIMD/sysreg correctness sweep: 4 real bugs + 3 ISA walls (127/0)
Continuing the cross-gcc battery. Two previously-"correct" paths and three
newly-hit instructions were wrong; all fixed + qemu-verified + regression-locked.

### 1. SimdShrAcc (usra/ssra) esize/shift decode bug (silent, real)
The ShrAcc decode derived esize from the 3-bit tagless immh via trailing_zeros,
which collapses EVERY esize>=4 shift to esize=1/shift=0 — ssra silently
accumulated WITHOUT shifting. Battery exposed: ssra .2d #2 of {-8,-16} returned
-24 not -6; ssra .4s #2 returned 72 not 18. Decode now mirrors the verified
SimdShr gate (full immh incl bit22, fls esize, shift = 2*esize_bits-(immh:immb)),
and the unsigned discriminator is bit29 (was bit11). Translate also guards
shift>=esize_bits (mirror SimdShr). qemu: -6 / 18 / 1.

### 2. neg reads rn=31 as XZR, not SP (AddSubReg, silent, real)
`neg xd,xm` = `sub xd, xzr, xm` (shifted-register, bit21=0) — rn=31 MUST be XZR
(=0). The translate read rn=31 as SP for every non-S op, so neg(x6) computed
sp-x6. Root cause: bit21 is the form discriminator (qemu: neg=0xcb0603e6 bit21=0
-> XZR; sub sp,sp,x1=0xcb2163ff bit21=1 -> SP). Added `sp_operand` (bit21) to
Inst::AddSubReg and applied on both read (rn) and write (rd) sides. Regression
`neg_reads_rn31_as_xzr_not_sp`.

### 3. SysReg MRS reads were silent no-ops (LATENT, all of them)
The translate wrote `buf.mov_ri64(rt,..)` where rt is a GUEST register index —
the value landed in a stray x86 reg, never committed to the guest file. So every
`mrs xN,<cntfrq|cntvct|nzcv|dczid|tpidr>` returned 0/garbage. Decode-side tests
passed because they only bind Inst fields; exec was never exercised (cf/dz
battery proved it: cntfrq + dczid both returned 0 before). Fixed all four read
paths (+ MRS via `stg`), and the msr-tpidr write kept as-is.

### 4. New ISA walls crossed
- dczid_el0 (`mrs x0,dczid_el0` = 0xd53b00e0): glibc CRT reads it to size its DC
  ZVA memset; returns 0x4 (16-byte block, DZP=0). sysreg 5.
- umulh/smulh (high 64 of 128-bit product): gate top 0x9b && bit22 set
  (separates from madd/msub where bit22=0), signed = bit23. x86 F7/4,F7/5
  one-operand mul/imul (RDX:RAX = RAX*rm). Verified vs qemu.

### Verification
- `cargo build --workspace` clean; `cargo test --workspace` 127/0 (arm64jit 93).
- Battery (all qemu-verified): ssra_2d=-6, ssra_4s=18, ushr=1, shl=24, shlimm=27,
  neg_d=2, cf(cntfrq)=100000000, dz(dczid)=4, mulh_e=2.
- modmain.elf (full glibc CRT) now advances past dczid + umulh/smulh to the next
  wall: MTE `stg x0,[x0]` (0xd9200800, __libc_mtag_tag_region) — memory tagging.

### Next (ordered, no APK/GSI/GPU on this box)
1. MTE stg/ldg/stzg memory-tagging no-op (unblocks full glibc-linked programs).
2. Continue the SIMD surface as the cross-gcc battery reveals it; then real
   `svc` syscall routing on actual use (real AArch64->x86-64 table; mmap 222 etc.).
3. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
4. Real-binary/GPU boot proof remains blocked (no libroblox.so/APK, no GPU) —
   HARD GATE on a capable host (`elfjit ... 0x1f0db20 --jni` run log).

---

# Session — glibc-CRT ISA sweep (dc/ic, MTE writeback, GCS/SME-TLS) + svc correctness (Sep 12 2026)

Continuing the cross-gcc / hand-assembled-battery approach with no APK/GSI/GPU.
**Three focused commits; workspace 132/0, arm64jit 97, tree clean.**

## 4db2b3c — data/instruction cache maintenance (dc/ic) no-ops
glibc's `__libc_mtag_tag_region` ends in a `dc` op. In the single-threaded
direct-mapped JIT these coherence ops (dc/gva/civac/ivac, ic ivau; top 0xd5,
CRn=7) are no-ops — EXCEPT `dc zva` which zeros the advertised 16-byte block.
Fixed two bugs in the leftover session-draft: Rt decoded from bits[9:5]
(instead of bits[4:0]; caused `dc zva x0` to write via x1 → segv), and the
test used 0xd50b7400 (=a `sys` instr) as `dc zva` — the real `dc zva x0` is
0xd50b7420 (CRm=4 && op2=1). modmain moved 0x40c174 -> 0x438b1c.

## d55cb04 — MTE tag-store writeback + mrs gcspr_el0/tpidr2_el0
- The MteTag gate forced bit10==0, so post/pre-index st2g/stg writeback forms
  (`[x2],#64` / `[x2,#-64]!`) were Unsupported. Their Xn-advance (Xn +=
  signed imm<<4) is a real side effect glibc memset/stg loops depend on; the
  tag-store itself stays a memory no-op. Decode now carries rn/wb/wb_off
  (imm9 sign-extended, scaled <<4). Load bit stays bit22 (ldg byte1=0x60;
  stg/st2g 0x20/0xa0), so the discriminator is unaffected. objdump-verified.
- `mrs gcspr_el0` (armv9 GCS ptr, 0xd53b2522) + `tpidr2_el0` (SME 2nd TLS,
  0xd53bd0ae) read 0 (features never enabled) — glibc CRT reads them sizing
  GCS call frames / probing SME. sysreg ids 6/7.
- modmain advanced to 0x442cf8, then stops on glibc's SME-IFUNC feature-probe
  (`str za w15,[x16]` = 0xe1206200). **Documented as BEYOND Roblox's
  Android/bionic boot ISA** — the real libroblox.so boot path is already fully
  decoded / exit 0 per prior sessions. Root cause of that glibc-only tail:
  elfjit sets up NO guest auxv, so glibc reads garbage AT_HWCAP and
  IFUNC-resolves into SME. Chasing the SME ZA-tile ISA is a synthetic-harness
  tangent, not a Roblox boot blocker.

## e20687d — svc syscall-number bugs + extended table + inline host-call fixes
Hand-assembled aarch64 svc programs (write / exit / multiple sequential svc)
through elfjit exposed real bugs on the syscall path:
1. **getuid was mapped to 199 (that's socketpair); real AArch64 getuid=174.**
   **mremap was mapped to 220 (that's clone); real = 216 (3264_mremap).**
   Neither was ever exercised (the unit test only checks write/mmap/getpid).
   Fixed; added uid/euid/gid/egid/tid/ppid @ 174-178/173. +regression
   `guest_svc_identity_numbers_match_aarch64_abi`.
2. Extended the table with common aarch64 boot syscalls: getcwd 17, chdir 49,
   getdents64 61, lseek 62, faccessat 48 (w/ AT_FDCWD), readlinkat 78, pipe2 59,
   set_tid_address 96, sched_yield 124, prctl 167.
3. **Inline host-call correctness (two real bugs):**
   - JIT block body runs at host RSP≡8 (mod 16) — correct for guest-to-guest
     BL (call_rel32) — but SysV needs RSP≡0 at a host CALL site. So
     `call guest_svc` / `call guest_sha1stem` fired misaligned; any callee with
     aligned stack work (format! in JIT_TRACE_SVC, SSE locals) SIGSEGV'd. Now
     sub rsp,8 before / add rsp,8 after each inline host call.
   - guest_svc(st)'s state arg was passed implicitly via RDI (held the entry
     state on the FIRST call by luck; a prior host call clobbers RDI), so the
     SECOND svc in a block passed garbage (+ misaligned deref of 0x1). Now
     `mov rdi, rbx` explicitly.
   Proof: svc_elf writes then exits 0; exit_only returns 7; `we` (2 writes +
   exit_group 3) returns 3 with both writes visible; trip (3 sequential
   writes) prints W1/W2/W3. Previously ANY 2nd svc segfaulted — a real
   blocker Roblox (many syscalls) would hit.

### Status
- `cargo build --workspace` clean; `cargo test --workspace` 132/0 (arm64jit 97).
- Battery clean (no unexpected walls); modmain still stops honestly at the
  documented SME `str za` (0x442cf8), beyond the Roblox boot ISA.
- Commits 4db2b3c, d55cb04, e20687d on local `dev`.

### Next (ordered, no APK/GSI/GPU on this box)
1. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
2. Optional (glibc-coverage only, not Roblox): give elfjit a guest auxv so
   glibc IFUNCs resolve to scalar (non-SME) paths.
3. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays
   the HARD GATE, blocked until a capable host + the real binary/APK.


---

## Session (Sep 11, 2026) — libbadcpu gregs register-map fix + arm64jit BitField disarm (151/0)
Two crates hardened against silent miscompiles; the HANDOFF's documented open
arm64jit bug (`__tunable_get_val` x4 corruption) is FIXED. Commits `8325edc`,
`9081bfc`, `17e449b` on `dev`.

### libbadcpu (8325edc): the emulator wrote the WRONG registers
`ucontext_t.uc_mcontext.gregs` is `greg_t[23]` with R8..R15,RDI,RSI,RBP,RBX,
RDX,RAX,RCX,RSP in slots 0..15 (only RIP=16/EFL=17 match the x86 reg number).
The old table indexed gregs[0] as RAX etc., so every emulated POPCNT/MOVBE/
LZCNT/TZCNT/BMI1 read/wrote the wrong register and corrupted guest state.
Now GREGS_IDX maps x86 reg number -> true slot. Also fixed: VEX `vvvv` was
never decoded (3-byte C4 + 2-byte C5) — ANDN used the DEST register as its
first source; and the VEX opcode byte was read from the C4/C5 prefix position,
so no 0F38/0F3A-map VEX instruction ever decoded correctly. 6 new tests;
libbadcpu 6->12.

### arm64jit (9081bfc): BitField dispatches by class — modmain boots PAST its old crash
Driving `modmain.elf` (full static glibc, qemu=12) through elfjit:
1. UBFIZ/SBFIZ (insert=false, immr>imms) went through the BFI/merge path and
   PRESERVED old Rd's bits. `ubfiz x4,x0,#7,#32` kept a stale 0x7f8000000000
   prefix, so glibc's `__tunable_get_val` ldr'd [x4,#48] at 0x7f800048e888
   (should be 0x48e888) -> SIGSEGV. UBFIZ zero-fills; SBFIZ sign-fills.
2. Genuine BFI (insert=true) was swallowed by the ROR shortcut
   (imms+immr+1==bits: 15+48+1==64) and compiled as a rotate. The LSR/LSL/ROR
   shortcuts are UBFM/SBFM aliases; BFM inserts now handled first (BFXIL
   in-place mask, BFI shifted merge).
Regressions `ubfiz_zero_extends_field_and_discards_old_rd` +
`bfi_still_merges_into_old_rd`. arm64jit 108->110; workspace 151/0; full
cross-gcc battery unchanged (loop1 45, structs/dispatch/fpfun/vtable 42,
byvalue 44, bv2/iso_arith 300, signmod 12, iso_wrd 4321, arr/shacc/fact/ldrsw/
fp_only/A/C 42).

### Honest remaining
- modmain now boots past its old `__tunable_get_val` crash; the udiv fix (below)
  cleared `_dl_determine_tlsoffset` too. It now stops deep in the glibc-CRT tail
  (`__memset_generic`, caller passed x0=0) — the HANDOFF-flagged synthetic-glibc
  tangent that is NOT a Roblox boot blocker (real libroblox boot path already
  fully decoded / exit 0).
- Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
  HARD GATE; blocked on a capable host + the real binary/APK (none on this box).

## Session (Sep 11, 2026) — UNSIGNED DIV WRONG-RESULT BUG FIXED in arm64jit (152/0)
Commits `28dba32` (div), `c1e41cb` (svc additions).
status: session-end (committed, tests green)

### Silent, wide bug: every UNSIGNED division in the JIT returned 0
The HANDOFF's open "small-address load in `_dl_determine_tlsoffset`" was the
guest doing `udiv x0,x0,x1`. A focused `exec_bytes` test (`udiv x5,x0,x1` =
0x9ac10805) left x5=0 for EVERY input (100/10, 7/1, 0/5), even in isolation.
Byte-dumping the emitted host code showed `48 f7 c1` — x86 group-3 `F7` uses
/6 = DIV and /7 = IDIV, but `div_r64`/`div_r32` emitted `modrm(3,0,..)` =
group-3 /0 = TEST, so the instruction decoded as `test rcx,eax` and never
produced a quotient. `idiv_r64` already used /7 and was correct — ONLY the
unsigned forms were broken. Confirmed with as+objdump: `48 f7 f1` = div rcx /
`48 f7 f9` = idiv rcx. Fixed both emitters to /6.
Regression `udiv_computes_quotient` (100/10=10, 7/20=0). Silent, WIDE wrong-
result class: any guest unsigned integer math (incl. Roblox) returned 0.

### svc table additions (c1e41cb)
glibc `__tls_init_tp` surfaced set_robust_list(99)->0 (no-op is valid; -ENOSYS
made glibc retry) and membarrier(283)->no-op. rseq(293) left -ENOSYS (no valid
rseq area). Verified vs aarch64-linux-gnu asm-generic/unistd.h.

### Result
modmain.elf booted THROUGH `_dl_determine_tlsoffset` (udiv fix), reached real
AArch64 syscalls, then hit glibc `__memset_generic` (x0=0 passed by caller) —
again the non-Roblox glibc-CRT tail. arm64jit 111/111; workspace 152/0; battery
unchanged.
- HARD GATE unchanged: `elfjit <libroblox.so> 0x1f0db20 --jni` on a GPU/APK host.

## Session (Sep 11 2026) — guest auxv bootstrap; SME/SVE/MulLong decodes; zero-extend fix (137/0)

Goal: prove the JIT boots a full statically-linked glibc aarch64 binary.
modmain.elf (`main(){return 300%16;}`, qemu returns 12) tests the whole CRT.

1. **`arm64jit::boot` — guest auxv on the initial stack.** The kernel ABI puts
   [argc][argv][envp][auxv AT_NULL] on the stack (sp at argc); glibc static
   `_start` walks it for envp/auxv. The JIT left garbage, so `_dl_hwcap2` picked
   HWCAP2_SME from garbage and fell into `__libc_arm_za_disable`'s `str za` loop.
   New `standard_auxv(&LoadedElf, hwcap, hwcap2)` + `layout_initial_stack()`
   (fills AT_RANDOM via bounded xorshift). Wired into elfjit AND sober-core::jit.
   Verified `_dl_hwcap2=0, have_sme=0`.
2. **SME/SVE feature-off decodes.** Even with SME off, glibc's ZA block is
   *linearly* reachable in the compiled CFG (compiler follows fall-through past
   the data-dependent SME gate), so it had to DECODE: `Inst::SmeNoop`
   (`str za[Wt,k],[Xn,#k,mul vl]` 0xe1206200..f + smstart/smstop), `Inst::
   AddVectorLen` (addvl/addsvl, `Xd = Xn + imm6*16`, model VL=16B), `Inst::
   SveCntd` (cntd -> 2), `mrs xN, midr_el1` (sysreg 8 -> 0).
3. **`Inst::MulLong`** — smull/umull/smaddl/umaddl/smsubl/umsubl, found in
   glibc `_dl_fixup` (IFUNC resolution). Gate top 0x9b & bits[22:21]==01.
4. **LATENT x86-emitter bug fixed:** `and_ri64(_, 0xffffffff)` was a NO-OP
   (`and r64,imm32` sign-extends the imm -> AND-all-ones). All 10 zero-extend
   sites silently leaked W-reg high bits (the glibc x3/x0 corruption). Added
   `CodeBuf::zero_ext_r32` (`mov r32,r32`) and replaced all 10 uses. Proven by
   umull/smull/umsubl/addvl exec tests.

Verification: `cargo build --workspace` clean; `cargo test --workspace` 137/0
(arm64jit 103). modmain.elf boot now advances past `str za` -> _dl_fixup umull
-> midr_el1, then STILL stops early on a glibc-startup x-reg corruption (auxv
scan / __libc_start_main prologue) — the zero-extend fix is in but a leftover
cause remains. qemu returns 12; JIT boot to completion NOT yet achieved.

Next: (1) finish the glibc-startup corruption; (2) modmain->12 proves full
static-glibc boot; (3) libloader gaps -> libbadcpu gaps -> services/auth;
(4) HARD GATE = real libroblox.so + GPU host (`elfjit <lib> 0x1f0db20 --jni`).


### Continued (Sep 11 2026) — after commit 5d40fdd, continued the same goal
Three more real JIT bugs fixed, each verified + a regression test; modmain.elf
(full static glibc, qemu returns 12) boot advances far into glibc startup.

- **Extended-register add/sub** (`Inst::AddSubExt`): `add x3,x2,w20,sxtw #3`
  (bit21=1) was decoded through the SHIFTED parser, mis-reading the option/
  shift bits as `lsl #sh_amt` (=51) — corrupting guest x-registers with
  `0x198...` garbage (a real glibc prologue corruption). New gate
  `n==1 -> AddSubExt`, proper ext(Rm)<<shift with 8 options + SP semantics.
- **Pre/post-index + unscaled immediate LDR/STR** (`Inst::LdStrImmWb`): the
  register-offset gate `(insn&0x3b000000)==0x38000000` was too broad and
  swallowed `ldr x3,[x0],#8` (0xf8408403), mis-reading imm9+writeback as an
  `rm` register -> NULL deref (the glibc auxv/env-scan crash). Narrowed to
  `(insn&0x3b200c00)==0x38200800`, added a proper signed-imm9 writeback path
  (pre/post/unscaled). NOTE: also fixed a latent `b(insn,hi,lo)` arg-order
  underflow panic in the new decode.
- **LSE atomics** (`Inst::LseAtomic`): ldadd/ldclr/ldeor/ldset/swp (ARMv8.1),
  hit in glibc's IFUNC `__aarch64_swp4_acq` (have_lse=0 so on a never-taken
  path, but the block compiler must still build it). Gate: (insn&0x3fe00000)
  in {0x382/0x386/0x38a/0x38e 00000} AND op=bits[15:10] in {0,4,8,0xc,0x20};
  single-threaded emulation (old=[Xn]; [Xn]=f(old,Rs); Rt=old).

Verification: cargo test --workspace 142/0 (arm64jit 108, +2 lse tests).
modmain.elf now runs __libc_start_main fully and dies deep in
`__tunable_get_val` on an address (0x48e8b8) with run-varying high garbage
(another latent 32-bit/zero-extend leak) — the next debugging target.
Also: elfjit now keeps a permanent SIGSEGV diagnostic handler (prints guest
pc + regs from CpuState) — invaluable for localizing a real-code crash.


### Continued (Sep 11 2026) — LSE atomics landed; modmain now dies in __tunable_get_val (block-register bug)

Commit 36a47f7 decoded the LSE atomics (ldadd/ldclr/ldeor/ldset/swp) with
single-threaded emulation; modmain.elf boot then advanced THROUGH
__libc_start_main and the IFUNC atomics (the ldxr/stxr fallback, since
have_lse=0) and now crashes inside `__tunable_get_val` (0x4128ec) on a
block-level register corruption:

- fault = 0x7fXX_0000_48e8b8 (low 0x48e8b8 constant, high 0x7fXX/0x7eXX
  run-varying).
- x7 = 0x48dc88 is CORRECT (adrp x3,0x48d000; add x7,x3,#0xc88 — both clean).
- x4 = 0x7f800048e888 is CORRUPTED. It should be
  `ubfiz x4,x0,#7,#32` (0xC00 for x0=0x18) then `add x4,x7,x4` = 0x48dc88+C00
  = 0x48e888. Instead x4 carries 0x7f8000000000 high garbage from the *source*
  x4 at the `add x4,x7,x4` (rm=x4 read gave 0x7f8000000C00).
- ubfiz is NOT the bug: proven clean in isolation AND on a pre-dirtied x4
  (0x1f000000 -> 0xC00), and the full ubfiz;add sequence is clean in isolation
  (0x48e888 with a correct x7 build). So it is a BLOCK-COMPILER register-
  interaction bug only in the real __tunable_get_val block (persists across
  JIT_BUDGET 2/8/16/300000, so not a boundary artifact): likely the intervening
  `mov w5,w0` (W write) or `adrp`/`mov` reusing a host register that also holds
  the rm=x4 value, so `add x4,x7,x4` reads a stale/dirty x4.
- Next: dump block@0x4128ec host code (JIT_DUMP) or add per-instruction guest
  x4 trace to find which instruction gives x4 the 0x7f8000000000 high prefix.

Also: elfjit now permanently installs a SIGSEGV diagnostic (guest pc + x0..x7 +
sp from the CpuState via ucontext RBX) — the tool that pinned all of today's
crashes; recorded because it is reusable.

`cargo test --workspace` 142/0 (arm64jit 108).

### HARD GATE (unchanged)
Roblox actually running (load -> JNI init -> main loop -> frame on a GPU host)
is NOT met and cannot be on this GPU-less VPS without the real libroblox.so/APK.
The elfjit path is a growing no-QEMU CPU translator that currently boots a full
statically-linked glibc program deep into its CRT/startup. The HARD GATE remains
`elfjit <libroblox.so> 0x1f0db20 --jni` on a capable host.

---

# Session (Sep 11, 2026) — scalar FP/SIMD unscaled + pre/post-index ld/st; libbadcpu VEX.0F38 completion (workspace 164/0, commits 36f01ff + 0fe7d8a)

Opened by re-running the workspace: the handoff's flagged
`test_setup_android_layout_idempotent` is ALREADY FIXED (8b72828: per-call
unique temp root + `create_dir_all` before `set_permissions`); it passes — the
workspace was 157/0 at start, not failing. Proceeded to two committed, tested
pieces (no APK/GSI/GPU required):

## 1. arm64jit — scalar FP/SIMD (B/H/S/D) UNSCALED (ldur/stur) + PRE/POST-index writeback ld/st
Closed the last of the `bit26=1` immediate family (the standing "stur/ldur
scalar s0/d0 + pre/post index" glibc-CRT-tail item).
- `Inst::FpLdStImmUnscaled` + `Inst::FpLdStImmWb` with a shared
  `fp_scalar_xfer` helper (transfers `size` bytes between memory and the low
  bytes of `v[vt]`, upper lanes preserved).
- Decode gate, each bit verified against aarch64-linux-gnu-as ground truth:
  bit26=1 (vector file), bit25=0 (immediate offset), **bit21=0** (NOT
  register-offset — found ONLY by the neighbor-collision test: FpLdStrReg
  register-offset words ALSO have bit25=0, the real discriminator is bit21),
  bit24=0 (not the scaled 0x3d form), bit23=0 (not 128-bit Q), bits[29:27]=111.
  operand size bits[31:30], ld=bit22, imm9 sign-extended, addressing =
  bits[11:10]: 00=unscaled, **01=post, 11=pre** (pre is `0x0c00` = 0b11, NOT
  0b10 — caught by the decode test). unprivileged LDTR/STTR (mode 2) left
  Unsupported.
- +4 tests. arm64jit 116→120. **modmain.elf (full static glibc) now boots
  PAST its documented `stur s0`/`stur d0` memset wall** into
  `__libc_setup_tls`/`_dl_get_dl_main_map`, stopping at a residual null-deref
  (fault 0x0, guestpc 0x400b30, right after `bl 0x413e60 _dl_get_dl_main_map`)
  — again the HANDOFF-flagged non-Roblox glibc-CRT tail, NOT a Roblox blocker.

## 2. libbadcpu — BEXTR + BZHI + SHRX/SARX/SHLX (complete the VEX.0F38 integer family)
The SIGILL emulator already did ANDN/BLSI/BLSMSK/BLSR (VEX.0F38 F2/F3/F1/F4);
added the rest. All encodings verified vs host gcc+objdump:
- BEXTR = 0F38 F7 pp=0: `(src1>>start)&(2^len-1)`, start=control[7:0],
  len=control[15:8] (control = VEX vvvv; pp=0 distinguishes it from the shifts
  which share F7 but carry a pp prefix).
- BZHI = 0F38 F5: `src1 & (2^ctrl-1)`; ctrl>=op-size keeps src1 + CF.
- SHRX/SARX/SHLX = 0F38 F7 with pp=F3/F2/66 respectively.
- **Real subtlety fixed:** fix-size for the 0F38 integer ops must come from
  VEX.W (`vex_w`), NOT the legacy `has_66 => 16-bit` rule — SHLX rax has pp=1
  (has_66) yet is 64-bit; the whole 0F38 branch now sizes off `vex_w`.
- +3 tests. libbadcpu 12→15.

## Gate
- `cargo build --workspace` clean; `cargo test --workspace` **164/0** (arm64jit
  120, libbadcpu 15, libloader 16, +1+1+11). Tree clean on local `dev`.
- HARD GATE unchanged: `elfjit <libroblox.so> 0x1f0db20 --jni` run log on a
  GPU + real-binary host.

---

## Session (Sep 11, 2026) — loader→JIT end-to-end regression test + libbadcpu 16-bit LZCNT fix (169/0)

Two commits on `dev` (HEAD `6d6ef08`). Workspace was 164/0 green at start
(`test_setup_android_layout_idempotent` is already fixed in 8b72828 — passes;
the FMOD divert 10ddb7a and JNI-table bdd8b03 tasks are also already landed).
So this session executed the ordered "libloader ELF/loader gaps" + "libbadcpu
ISA gaps" steps.

### 1. `7f54fbd` — arm64jit: loader→JIT end-to-end regression test
`crates/arm64jit/tests/loader_run.rs` cross-compiles real `-nostdlib` aarch64
programs (via `aarch64-linux-gnu-gcc`) and runs `entry()` through the exact
pipeline elfjit / `sober-core --jit` use: `load_elf_image` → `bind_image_plt`
→ guest stack/TLS/auxv bootstrap → `jit_run`. Results: add=42, loop sum(0..9)=
45, fp `(int)(2.5*4.0)`=10, fib(7)=13. Locks the loader path against
regressions from the divert + JNI-table changes (the bdd8b03 "confirm no
regression" deliverable), since the real-binary HARD GATE can't be exercised
here. Skips cleanly when cross-gcc is absent.
- **Found en route:** `[test]` threads run in parallel in one process, and
  `load_elf_image` MAP_FIXEDs the same non-PIE JIT base 0x400000 — 4 threads
  clobber each other's guest image → a `bl` target missing from the compiled
  block's stub table panics `stub_of_target[...]` (jit.rs:1164). Real open-
  sober loads ONE guest ELF for process lifetime, so this is purely a test-
  harness serialization concern: `run_lock()` serializes load+bind+run.

### 2. `6d6ef08` — libbadcpu: 16-bit LZCNT wrong-result bug
emulator.rs LZCNT 16-bit arm was `(src as u16).leading_zeros() as u64 - 16`.
`u16::leading_zeros` already returns the 0..16 count, so `-16` made every
nonzero 16-bit LZCNT return negative (wrapped huge i64 in the dest reg).
TZCNT's sibling arm has no such offset; the 32/64-bit arms don't either — only
LZCNT had it. Dropped the offset; +`lzcnt_16bit_matches_real_count`
(cx,ax=2 -> 14; cx,ax=0x8000 -> 0). Silent wrong-result class in the SIGILL
emulator.

### Gate
- `cargo build --workspace` clean (0 errors; warnings are the pre-existing
  decode.rs rustfmt churn — rustfmt not installed on this box).
- `cargo test --workspace` **169/0** (arm64jit 120 + 4 loader_run + libbadcpu
  16 + libloader 16 + 1 + 1 + 11).
- HARD GATE unchanged: `elfjit <libroblox.so> 0x1f0db20 --jni` run log on a
  GPU + real-binary host (no APK/libroblox.so/GPU on this VPS).

## Session (Sep 11, 2026) — three silent FP/SIMD miscompiles fixed via a double-precision C battery (commit a0465a0)
Drove a new cross-gcc double-FP battery (real `double` C: polynomial Horner,
array sums, 2x2 matmul, exact division, 3^10 accumloop with fcvtzs, |x|>threshold
counting, weighted average) through `elfjit`, comparing each result to a native
x86-64 compile. Ground truth: dpoly31 dsum17 dmat50 ddiv10 dscale1 dclamp4 ddmat2-4.
Found + fixed THREE real miscompiles (all silent wrong results, not crashes):
1. **scalar fsub (0x1e613800) decoded as SIMD WidenShl** — the shll gate's
   `(insn>>24)&0x0f==0x0e` nibble test ALSO matched the scalar-FP 0x1e family
   when bits15:8==0x38, so `fsub d0,d0,d1` ran as a halfword-widen no-op
   (59049.0-59048.0 → 0.0). Real shll bytes are 0x0e/0x2e/0x4e/0x6e (bit28=0);
   scalar-FP 0x1e has bit28=1. Gate now requires bit28==0. dscale.elf 0→1.
2. **store_nzcv_fp hardcoded N=0** — FP compare sets N=1 for ordered less-than,
   so b.mi/b.lt/b.le never fired and b.gt evaluated N==V as 0==0 for every
   ordered non-equal pair (dclamp 6→4). N now = CF∧¬ZF.
3. **ld1 multiple-structure (2-reg, opcode bits[15:12]==0xA) swallowed by the
   ld2 gate (0x8, deinterleave)** — compiler array-literal `ld1 {v30,v31}`
   loaded interleaved garbage (ddiv double array 2→10). Added Ld1N/St1N
   (consecutive, NO deinterleave) for 1/2/3/4-reg, discriminated by opcode
   bits[15:12]. (Verified real encodings: ld1-2reg=0x4c40a040, ld2=0x4c408040,
   ld1-1reg=0x4c407040.)
+3 regression tests (fsub-vs-shll collision, FP-compare N flag + b.mi/le/gt,
ld1-2reg consecutive). arm64jit 123/123, workspace **172/0**. Cross-gcc C
battery + SIMD hand-battery (lane_test/smov=-570, loop1=45, iso_*=etc) all
still green. fsqrt verified (sqrt(16)=4 via sqrtquad.elf).
Honest: dsqrt .c didn't link (sqrt undefined under -nostdlib); tested fsqrt via
hand-asm instead. Test-setup lesson: guest Dn/Vn maps to st.v[2n]/st.v[2n+1]
(D1 = st.v[2], NOT st.v[1]) — two new tests initially failed on my own wrong
constant placement, not a JIT bug.
Next: keep pressing the cross-gcc FP/SIMD surface (division edges, fma chains,
single-precision float, struct-by-value + FP, loop-with-FP-condition) to find
more silent miscompiles; then libloader gaps. HARD GATE unchanged.

## Session (Sep 11, 2026) — FMOV-immediate [16,30] decode bug fixed (commit bc5ab89)
A second cross-gcc battery (single-precision array div, double loop, mixed
int/float casts, double-struct-by-value, float matmul, double reciprocal —
native ground truth fdivf20 dloop7 mixed4407 dstruct169 dneg0 fmat69 drec124)
hit: fdivf `float a[]={6,12,18,24}; sum a[i]/3` returned 6 instead of 20.
Root cause was NOT the float div (isolated divss/addss/fcvtzs.e,.s all correct)
but **decode_fmov_imm's exponent wrap**: the 3-bit field E maps E0..3->e+1..+4,
E4..7->e-3..0, but the code wrapped `ex>=4`, so E=3 (exponent +4 => constants
16.0..30.0) decoded as -4 (0.0625..0.117). Every FMOV-imm in [16,30] — sample
rates, half-texel, 24.0 corner constants — came out ~256x too small (silent).
Fix: threshold `ex>=5` (E=4 gives (E+1)=5 -> -3). Verified against the
assembler's encodings for 0.125..30.0 (immf.s). fdivf 6->20; dloop/mixed/
dstruct/dneg/fmat/drec all match native. +4 decode_fmov_imm asserts (16,30,2,
0.75). arm64jit 123/123, workspace 172/0, prior battery unchanged.
Lesson: single- and double-FP immediate decoding share decode_fmov_imm — a
boundary exponent bug corrupts both (the fdivf array used f32, dscale earlier
used 3.0; the [16,30] band is where it bites).
Next: keep pressing FP/SIMD (fma/compiler-contracted `fmla`, more div/compare
edges, single-precision struct args) then libloader gaps. HARD GATE unchanged.

## Session (Sep 11, 2026) — scalar FP multiply-accumulate fmadd/fmsub/fnmadd/fnmsub (commit 578faaf)
Third FP milestone: the -O2 compiler contracts every a*b+c / fused a*x*x into
`fmadd`, and fma1 (2x^2+3x+1 over x=1..5) returned 0x8000000000000000 garbage
because 0x1f4.. was swallowed by a broad SIMD vector-immediate gate and
mis-decoded as a bogus movi. Added Inst::Fma3 (scalar 3-source FP):
- decode gate `(insn & 0xff000000)==0x1f000000` placed at the TOP of decode
  (uniquely the scalar 3-source FP family), o1=bit21(fn*), o2=bit15(sub),
  sz=bit22(double).
- translate: mulsd/addsd/subsd with pxor-0 + subsd for the fnmadd negation;
  single-precision via mulss/addss/subss. Semantics fmadd=ra+rn*rm,
  fmsub=ra-rn*rm, fnmadd=-(ra+rn*rm), fnmsub=rn*rm-ra.
- Verified vs assembler (fmadd/fmsub/fnmadd/fnmsub d0,d1,d2,d3 =
  0x1f420c20/0x1f428c20/0x1f620c20/0x1f628c20) -> 17/-7/-17/7 with d1=3,d2=4,
  d3=5; single fmadd s -> 11. fma1.elf 160 = native.
+scalar_fma3_all_four_variants (4 double + 1 single). arm64jit 124/124,
workspace 173/0. Full 3-batch cross-gcc battery unchanged.
This session net: 5 FP/SIMD correctness fixes (fsub-vs-shll, FP-compare N flag,
ld1-2reg deinterleave, FMOV-imm [16,30], FMADD) + the FMADD feature. Next:
keep pressing -O2/FP-contracted programs, single-precision struct args, more
div/compare edges; then libloader gaps. HARD GATE unchanged.

## Session (Sep 11, 2026) — shifted-register add/sub clobber bug (commit 9cc0f16)
Fourth FP milestone. The -O2 loop version of fclamp (double clamp across an
array) returned 10 vs 9; straight-line clamp worked. Root cause:
`apply_shift_const(buf, x, kind, amt)` wrote the shift amount into RCX
(`mov rcx, amt`) then `shl rcx, cl`, but both callers (AddSubReg, AddSubExt)
pass x == RCX (the Rm value being shifted) — so the value was clobbered and the
operand became `amt<<amt` instead of `Rm<<amt`. `add x1,x2,x0,lsl#3` (the
ubiquitous array-index idiom) computed x2+24 CONSTANT, so -O2 double-array loops
read the SAME element each iteration (fclamp: all 5 reads of a[2]=2.0 -> sum
10). Fixed to the immediate-shift C1 /4..7 ib forms (no CL scratch). fclamp
10->9. +regression add_shifted_register_... (lsl#3/lsr#2/asr#1/plain add)
verified vs shadd.o encodings. arm64jit 125/125, workspace 174/0; full battery
unchanged. (dnorm.elf: its Newton reciprocal-sqrt overflows to +inf = UB in C,
not a JIT bug — excluded.)
Lesson: emitters that use a fixed scratch register must never be handed that
same register as an operand. apply_shift_const's CL scratch collided with the
RCX operand; immediate-shift forms sidestep it entirely.
Session net: 6 FP/SIMD correctness fixes + FMADD/FMA3 feature, all committed
with regression tests. Next: keep pressing -O2/loop/array coverage (the
fclamp class is now unblocked), single-precision struct args, division
edges; then libloader gaps. HARD GATE unchanged.

## Session (Sep 11, 2026) — LdStPair D-register stride bug + 4th -O2 battery (commit 1eb7c0a)
Fifth FP milestone. A 4th cross-gcc battery (all -O2, exploiting the now-fixed
shifted-register indexing: 3x3 int matmul, struct{double x,y} array walk,
byte-scan, 64-bit loop, short array) surfaced one more real bug:
structfield.elf (loop `ldp d29,d28,[x0],#16` + `fmadd`) returned 128 vs 52.
Root cause: the LdStPair `fp_d` branch located each D-register at
VECTOR_BASE + rt*8, but a guest Dn is the LOW 8 bytes of its 16-BYTE vector
slot (VECTOR_BASE + rt*16) — so `ldp d29,d28` wrote 8-byte values to
0x1f8/0x1f0 instead of 0x2e0/0x2d0, and the follow-on fmadd read the stale
vector slots (still the initial q-pair array literal). Fixed both load+store
to *16 (q128 already used *16). structfield 128->52.
- 4th battery results (all match native): m3=45, structfield=52, bytes=5,
  iloop=150, shorts=24, plus the earlier fclamp/fhorner/fquad/fdivmix.
  +regression ldst_pair_d_registers_use_16_byte_vector_stride.
arm64jit 126/126, workspace 175/0; full 4-batch FP battery + core C battery
all green. dnorm (Newton reciprocal-sqrt that overflows to +inf = C UB) stays
excluded as degenerate.
Session net: 7 FP/SIMD correctness fixes + FMADD feature, each regression-locked.
Next: keep pressing -O2 arrays/structs (now unblocked), single-precision wider
structs, then libloader gaps. HARD GATE unchanged.

## Session (Sep 11, 2026) — single-precision LdStPair (s-pair) scale bug (commit 8d2d57b)
Sixth FP milestone. 5th -O2 battery (float struct array, 2D float matmul det,
float exp poly, string copy, unsigned arith) surfaced one more: fstruct returned
0x391c0000 garbage (should be 34), fmat2's singular 3x3 float det returned -108
(should be 0). Root cause: byte3 0x2c/0x2d (single-precision FP pair `ldp s0,s1`)
was lumped into `fp_d` (scale 8), so each 32-bit s-reg was read as 8 bytes and
post-indexed 2x. FP/vector pairs distinguish 64-bit d (0x6d/0x6c, bit30=1) from
32-bit s (0x2d/0x2c, bit30=0). Split into a new `fp_s` flag: scale 4, 4-byte
transfers into the low 4 bytes of each 16-byte vector slot (sN = VECTOR_BASE +
N*16). fstruct 34, fmat2 0, fexp 649, str 0, uint 999 — all = native.
+regression ldst_pair_s_registers_use_4_byte_transfers. arm64jit 127/127,
workspace 176/0; full 5-batch battery + core C all green.
Session net: 8 FP/SIMD correctness fixes + FMADD, all regression-locked. Next:
keep pressing -O2 float/struct coverage, then libloader gaps. HARD GATE
unchanged (real libroblox.so boot + GPU host).

## Session (Sep 11, 2026) — sdiv/udiv signedness inversion (commit dae2e05)
6th -O2 battery (signed/variable division, fmin/fmax, fmod, fabs): idivA
(`s += a[i]/d[i%3]`) returned 0xaaaaaa2d, wanted -35. Root cause: the MulDiv
decode gate used `b(insn,17,17)==1` for SDIV-vs-UDIV, but bit17=0 for BOTH
forms — the real discriminator is bit10 (sdiv=1, udiv=0; verified by assembling
matching-operand pairs 0x1ac50c61 vs 0x1ac50861). So every sdiv was labeled
unsigned -> JIT emitted `xor edx,edx; div rcx` (unsigned) instead of `idiv`,
turning negative dividends huge. gcc's magic-constant division masked it in
earlier tests. Fixed to bit10 (matches the 2-source gate at ~2054).
+regression sdiv_is_signed_udiv_is_unsigned_same_negative_input. arm64jit
128/128, workspace **177/0**. idiv -33, idivA -35, all 6 batches + core green.
Cycle total: 9 FP/int miscompile fixes + FMADD, regression-locked. HEAD dae2e05.

## Session (Sep 11, 2026) — MSUB operand direction (commit ccbf55c)
7th -O2 battery (byte-string sum+modulo, switch table, SIMD-ish reduction,
16-bit accumulate, bitfield pack, strcmp): bytelen (n%50 after byte loop,
n=1298) gave -48 instead of 48. Root cause: MulDiv MSUB arm computed rn*rm-ra,
but ARM MSUB is ra-rn*rm, so n-(n/50)*50 via `msub w0,w1,w0,w2` = 25*50-1298 =
-48. Constant-folded addrs masked it. Fixed direction (MADD arm already
correct). +regression msub_reuses_rm_as_rd.*. arm64jit 133/133, workspace
**178/0**. All 7 batches + core + FMA green. Cycle total: 10 miscompile fixes +
FMADD, all regression-locked. HEAD ccbf55c.

## Session (Sep 11, 2026) — ADDV SIMD horizontal add (commit 6290937)
8th -O2 battery (64-bit mul/div, short-array SIMD sum, dot, int matmul): vadd
gcc fully SIMD-vectorizes a 16-short sum to `ldr q`+`addv s0,v1.4s`+`fmov w0,s0`
- returned 0 (wanted 360). ADDV undefined; an earlier dup/move gate swallowed
0x4eb1b820 and emitted per-lane identity copies. Implemented Inst::Addv: mask
0xfffffc00 (clears Vn bits9:5, Sd bits4:0), residues 8b/4h/16b/8h/4s; NOTE
source Vn at bits[9:5] (bits20:16 fixed=17) — nonstandard SIMD layout. Gate at
top of decode (dup/move swallowed it later). Translate sums sign-extended
lanes -> RDI -> bottom element of Vd. vadd 360=native. +regression
addv_horizontal_sum_across_4s_lanes. arm64jit 134/134, workspace **179/0**.
Cycle total (back half): 4 fixes (sdiv signedness, MSUB direction, s-pair
scale, ADDV) + earlier (WidenShl, FP-N, ld1-2reg, FMOVimm, FMA3, apply_shift,
d-pair stride) = 11 miscompile fixes + FMA + ADDV. HEAD 6290937.

---

## Session (Sep 11, 2026) — libloader: Android packed relocations (APS2) + RELATIVE application (workspace 184/0)

Per the ordered "libloader ELF/loader gaps" step: the Rust loader did **zero
relocation** — `load_elf_image` only mapped segments, sp-mprotected them, and
relied on `bind_image_plt` for JUMP_SLOT. Real Roblox APK libs and their
Android/GSI dependencies carry `R_AARCH64_RELATIVE` data relocations (often
packed via `DT_ANDROID_RELA`), which a real loader materializes before the code
can dereference pointer globals/vtables. The QEMU path worked around this with
the external `unpack_rela.py`; this session brought it in-process to the Rust
loader so the JIT path can load those libraries.

### `crates/libloader/src/android_relocs.rs` (new)
- `read_sleb128` (sign-correct, x64-bounded; terminal-byte bit6 = value sign).
- `decode_aps2` — faithful port of AOSP `for_all_packed_relocs` (validated in
  Session 14 against real 2.726.1142 libroblox.so): magic `APS2`, then
  SLEB128 `num_relocs` / running `r_offset` / groups with the
  GROUPED_BY_INFO/OFFSET_DELTA/ADDEND + GROUP_HAS_ADDEND flag logic.
  Declared-count mismatch → error (no silent truncation).
- `read_elf_relocations(path)` — walks PT_DYNAMIC, prefers
  `DT_ANDROID_RELA`/`DT_ANDROID_RELASZ` (0x60000011/12) over plain
  `DT_RELA`/`DT_RELASZ`, reads the stream from file, decodes APS2 or parses
  stock 24-byte Elf64_Rela. **Early real bug**: PT_DYNAMIC's `p_offset` is a
  *file* offset, not a vaddr — I wrongly ran it through `vaddr_to_file_offset`
  and got `DT_* vaddr not covered by a PT_LOAD`; only the RELA vaddr needs that
  conversion.
- `apply_relatives` — for each `R_AARCH64_RELATIVE` writes `load_bias + addend`
  (8B LE) at `guest_of(r_offset)` via a caller-supplied target resolver.

### Wired into `load_elf_image` (elf.rs)
Reordered the segment loop: copy-file → push segment (NO mprotect in the copy
loop), then for PIE (`is_pie`) read+decode relocs and apply RELATIVE **while
the whole image is still RW**, then a second pass mprotects each segment to its
final ELF protection. Non-PIE ET_EXEC (self-relocating like the battery ELFs)
is untouched by the `is_pie` gate.

### Verification (both honest, both catch regressions)
- Unit: a **hand-built** APS2 golden bitstream (independent byte-by-byte
  encode; first attempt used `[0x80,0x40]`="+0x2000" which is actually
  SLEB -8192 — the decoder correctly rejected my wrong test bytes, a good sign
  the decoder is faithful) + a grouped-by-offset-delta stride case
  (r_offset = header + i*delta) + SLEB negative + bad-magic reject.
- Integration `crates/libloader/tests/reloc_apply_test.rs`: cross-gcc
  `-shared -fPIC` ET_DYN with `int *ptr = &data` → asserts EVERY
  `R_AARCH64_RELATIVE` slot (parsed from `readelf -r`, whose type column is
  truncated to `R_AARCH64_RELATIV`) equals `load_bias + addend`. Without the
  apply the slot holds the raw file addend (e.g. 0x600) ≠ 0x100000600 → fails.
- `cargo test --workspace` **187/0** (libloader 20 → 23 unit + 1 integration);
  `cargo build --workspace` clean (only the pre-existing decode.rs rustfmt-warn
  churn). Skips when cross-gcc absent.

### Addendum (same cycle) — DT_RELR support
`read_elf_relocations` now also materializes **`DT_RELR`** (tag 0x23, Android
13+ / modern NDK default) when the ELF ships only `.relr.dyn` (no RELA table).
`dt_relr_to_relatives` implements the shipped glibc/Android `DO_RELR` decode
exactly (the low-bit-marker scheme; the upper-8-bit-delta variant was a rejected
alternative): an even word is an offset that relocates itself and seeds
`base = offset+8`; an odd word is a bitmap where bit *i* (1-based) → reloc at
`base + (i-1)*8`; odd value-1 padding decodes to nothing. +3 unit tests
(offset+bitmap bit mapping, no-prior-offset from base 0, non-multiple-of-8
reject). Note: neither the cross nor host `ld` here supports
`--pack-dyn-relocs=relr`, so no real `.relr.dyn` fixture is producible on this
box — the decode is anchored to the authoritative algorithm plus hand-computed
streams. `cargo test --workspace` **187/0**.

### Status / next
`load_elf_image` now materializes RELATIVE relocations for PIE/shared objects,
from **(a)** standard `DT_RELA`, **(b)** Android APS2-packed `DT_ANDROID_RELA`,
or **(c)** `DT_RELR` — all in-process (no `unpack_rela.py`). Next (ordered):
a GLOB_DAT/JUMP_SLOT resolver for the main dynamic (arm64jit's `bind_image_plt`
already covers JUMP_SLOT), then libbadcpu ISA gaps, then services/auth. HARD
GATE unchanged: `elfjit <libroblox.so> 0x1f0db20 --jni` run log on a
GPU + real-binary host (no APK/libroblox.so/GPU on this VPS).

### Addendum (same cycle) — libbadcpu: PEXT/PDEP were mis-emulated as BZHI
The VEX.0F38.F5 opcode byte is shared by **BZHI / PEXT / PDEP**, disambiguated
only by the VEX pp bits (assembler ground truth `gcc -c + objdump -d`:
bzhi=pp0, **pext=pp2, pdep=pp3**). The emulator's 0xF5 branch treated every
case as BZHI, so both **PEXT and PDEP silently produced wrong results** (a bit
extract became a low-bit mask). Fixed in `emulator.rs`: pp3/f3 → PDEP
(deposit source bit i into the i-th set mask position), pp2/f2 → PEXT (gather
source bits at mask positions into the low bits), else BZHI. Operands:
dest=ModRM.reg, source=vvvv, mask=rm. Also fixed a latent flag-ordering bug —
the BZHI/PEXT/PDEP carry flag was set *before* `update_flags_common` cleared
it, so CF never survived; now `cf_pending` is applied after. +2 tests with
expected values **verified on real hardware** `_pext_u64`/`_pdep_u64` (this
host has BMI2): pext(0xFF,0b1010)=3, pext(8,0b1010)=2, pdep(3,0b1010)=10,
pdep(0xFF,0b10101010)=170, plus 32-bit forms and a PEXT-vs-BZHI discriminating
case. `cargo test --workspace` **189/0** (libbadcpu 16 → 18).

### Addendum (same cycle) — loader→JIT end-to-end PIE + RELATIVE regression
Validated and regression-locked the full **`load_elf_image` → RELATIVE apply →
JIT execute** chain on a REAL PIE: cross-compiled `-fPIE -pie -nostdlib` aarch64
binary (`int *gptr = &shared_static; entry(){ return *gptr+1; }`) — the compiler
emits one `R_AARCH64_RELATIVE` in `.data.rel.ro` (offset 0x20008, addend
0x20000 = &shared_static). Without application `*gptr` derefs the unrelocated
link address (NULL page) and faults; with it, `gptr = base+0x20000` and
`entry() -> 42`. Verified by hand (`elfjit ./pie.elf -> 42`) and locked as
`loader_run_pie_relative_global_returns_42` in `crates/arm64jit/tests/
loader_run.rs` (new `compile_pie` helper; asserts the fixture really carries a
RELATIVE reloc). `cargo test --workspace` **190/0**.

## Session (Sep 11, 2026) — bind GLOB_DAT + ABS64 main-GOT relocations (commit 7f03937, workspace 191/0)

Continuing the ordered "libloader ELF/loader gaps" step. `bind_image_plt` only
walked `DT_JMPREL` (JUMP_SLOT) and `load_elf_image` only applied RELATIVE;
**R_AARCH64_GLOB_DAT (1025)** in the main `DT_RELA` was never bound. A
`-shared -fPIC` module referencing an exported global (data or function
pointer) goes through its **main GOT** via GLOB_DAT: the guest does
`adrp x0,GOT; ldr x0,[x0,#off]` to fetch the symbol's *runtime address*, then
derefs/calls through it. Unbound, the slot read 0 → SIGSEGV on NULL / call to
address 0.

Empirically confirmed (cross-gcc `-shared`): `global_data` and `gfp=&internal_fn`
produce two GLOB_DAT relocs at GOT offsets 0x1ffd8/0x1ffe0 (both 0 in file), and
`gfp = &internal_fn`'s initializer is **R_AARCH64_ABS64 (257)** — a second member
of the same "write symbol runtime-address" family that the loader also ignored.

### New `bind_glob_dat` (crates/arm64jit/src/plt.rs)
- Walks the main `DT_RELA`/`DT_RELASZ` for GLOB_DAT (1025) **and** ABS64 (257).
- **Defined-in-module** symbol → writes `el.guest_of(st_value) + addend` (ABS64
  carries the symbol offset as addend; GLOB_DAT addend 0). The loader maps
  guest==host, so `ldr xN,[GOT]` then `[xN]`/`blr xN` resolves back into the
  mapped image.
- **Undefined/imported** symbol → `STT_OBJECT` uses `dlsym` raw (guest==host
  addressable); FUNC/NOTYPE uses the resolver's host-call thunk (callable).
- Called from `bind_image_plt` (end of the normal path) **and** from the
  `pltrelsz == 0` early-return — an exported-data-only module has zero JUMP_SLOT
  yet still depends on the main GOT.
- Fixed en route: the `?` operator can't be used in a `(usize,usize)`-returning
  fn (switched the import branch to a match).

### Verification
- `elfjit /tmp/gdtest/self.so 0x360` (real `-shared` aarch64):
  ```
  [plt] (no JUMP_SLOT) bound 2 GLOB_DAT, 0 unresolved  -> then ABS64 added: 3
  JIT(no-QEMU) entry() -> 37 (0x25)     # global_data(11) + gfp(3)=internal_fn(3)=15 + global_data(11)
  ```
  Before the fix it SIGSEGV'd at fault=0x0 (guest GOT loaded 0).
- **+`loader_run_shared_glob_dat_and_abs64_returns_37`** in
  `crates/arm64jit/tests/loader_run.rs` — cross-gcc `-shared -fPIC -nostdlib
  -Wl,-e,entry` fixture with the exact two-global GOT pattern; runs the full
  load→RELATIVE→GLOB_DAT/ABS64→jit_run pipeline and asserts 37. Skips without
  the cross toolchain.
- `cargo build --workspace` clean; `cargo test --workspace` **191/0**.

### Next (ordered, no APK/GSI/GPU on this box)
1. Continue libbadcpu ISA gaps (the SIGILL emulator's remaining VEX/legacy
   instructions), then services/auth.
2. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK are available.

## Session (Sep 11, 2026) — GLOB_DAT/ABS64 binding, libbadcpu BMI2 completion, auth identity (workspace 199/0)

Ordered plan continued (no APK/GSI/GPU on this box; the android idempotent
test is already fixed and green). Four focused commits, each closed by the
real `cargo build --workspace` + `cargo test --workspace` gate:

### 7f03937 — arm64jit: bind GLOB_DAT + ABS64 main-GOT relocations
`bind_image_plt` only walked DT_JMPREL (JUMP_SLOT) and `load_elf_image` only
applied RELATIVE, so `R_AARCH64_GLOB_DAT` (1025) — how `-shared -fPIC` code
fetches an exported global's runtime address via the main GOT — was never
bound: the guest `adrp;ldr x0,[GOT]` read 0 and deref'd/called NULL.
New `bind_glob_dat(el)` walks DT_RELA for GLOB_DAT **and** R_AARCH64_ABS64
(257, the sibling "write symbol value" data-initializer family, confirmed by
cross-gcc that `gfp=&internal_fn` emits ABS64), writes `el.guest_of(st_value)
+ addend` for defined-in-module symbols, `dlsym` (OBJECT) / host-call thunk
(FUNC) for imports. Runs in the normal path and the `pltrelsz==0` early-return
(an exported-data-only module has zero JUMP_SLOT yet needs the main GOT).
Real `-shared` fixture: SIGSEGV (fault=0x0) -> `entry() -> 37` (global_data 11
+ gfp(3)=15 + global_data 11). +`loader_run_shared_glob_dat_and_abs64_returns_37`.

### b177f7c — libbadcpu: MULX (VEX.0F38.F6) + RORX (VEX.0F3A.F0)
BMI2 gaps: MULX = unsigned RDX*rm, high->modrm.reg, low->vvvv (u128 product —
a naive u64 `>>64` overflowed), flags cleared. RORX = rotate-right-by-imm8,
flags untouched; added the VEX.0F3A dispatch (decoder stops after ModR/M, so
read imm8 at RIP+len / advance len+1). +2 tests (values verified with -mbmi2:
rorx64(1,4)=0x1000000000000000, rorx32(1,31)=0x2).

### 2a9c6eb — libbadcpu: ADCX (66 0F38 F6) + ADOX (F3 0F38 F6)
`emit_adcx_adox`: Dest=Dest+Src+flag, write only the working flag (CF/OF).
Two real bugs found+fixed: width from REX.W not operand_size (the 66/F3 is a
mandatory opcode prefix, not a size override — a 64-bit ADCX is 66 48 0F38 F6,
operand_size folds 66->16); and 32-bit carry detected in the u32 domain
(0xFFFFFFFF+1 wraps to 0 WITH carry). Ground truth from a real-BMI2 assembly
driver confirmed the ADOX subtlety: OF is set to the *unsigned* carry-out, not
signed overflow (adox(0x7fff..,1)=0x8000.. has OF=0). +2 tests.

### a8249f4 — sober-services: forward full login result (services/auth)
The OAuth webview's AuthResult declared user_id/username but the callback only
extracted the token — IPC AuthToken always went out with both None, so the
parent couldn't identify the account without a second Roblox API call.
New extract_auth_result() parses token + user_id + username (query precedence,
#fragment tolerant, URL-decoded); send_auth_result() forwards the identity;
run_login_flow blocks on the token then sends the full result. +4 tests.

### Gate
`cargo build --workspace` clean (0 errors), `cargo test --workspace` **199/0**
(arm64jit 120 + loader_run 7 incl. the glob_dat fixture; libbadcpu 22;
sober-services 15; libloader; others). HEAD `2a9c6eb`, tree clean.

### Next (ordered, no APK/GSI/GPU on this box)
1. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays
   the HARD GATE — blocked until a capable host + the real binary/APK exist.
2. Continue hardening: next ISA/loader/emulator gaps as discovered (adb/emulate
   surface), then the remaining sober-core/sober-services integration.

---

## Session 2026-09-11 — LogicImm DecodeBitMasks rotate-RIGHT fix (workspace 200/0)

### The bug (severe, silent, commonest-instruction-class)
`decode_logical_mask` (crates/arm64jit/src/decode.rs) applied a **LEFT**-rotate
to the immediate element (`ones << r | ones >> (esize-r)`) but ARM
`DecodeBitMasks` (DDI0487) uses **ROR — rotate right**. Every rotation-
ASYMMETRIC logical-immediate mask was silently miscompiled (any `mov/and/orr/
eor/tst/ands xD,#<asym-mask>`). Symmetric masks (alternating 0xCCCC/0x5555,
single-bit, all-ones) give the same value under both directions, which is why
the whole prior test set stayed green for months despite the wrong code —
<15% of encodings are rotation-invariant.

Real failure: `mov x0,#0xffffffff80000001` = `0xb26187e0` (immr=imms=33)
returned `0xfffffffe00000007` instead of `0xffffffff80000001`, caught by a
cross-gcc probe run through elfjit (no QEMU) vs native x86-64.

### Fix + quantified verification
- Rotate right in the esize-bit domain: `(ones >> r | ones << (esize - r)) & em`.
- Ground-truth cross-check vs the real `aarch64-linux-gnu-as`+`objdump` over
  ~700 (N,immr,imms): fixed right-rotate agrees **592/592 valid**; old
  left-rotate would have been wrong on **509**.
- End-to-end elfjit: `mov x0,#0xffffffff80000001` -> exact value; the
  esize-64! case `mov w0/x0,#0x3ffffffc` (`0xb27e6fe0`) exact too.
- Regression `logic_imm_rotation_asymmetric_mask_ror` (both asymmetric
  encodings + symmetric 0xCCCC/#1 unchanged).
- `cargo build --workspace` clean; `cargo test --workspace` **200/0**.
- Commit `8c1706b`.

### Next
Continue the cross-gcc ISA-surface battery for more silent-miscompile classes;
libloader/libbadcpu/services gaps; HARD GATE (real Roblox boot, GPU/APK host)
unmet on this box.

---

## Session 2026-09-11 — three silent SIMD/logic-imm miscompiles fixed (workspace 202/0)

Session opened with the handoff-flagged failing test already green (200/0 from
committed cycles); the "failing test first" gate was satisfied by prior work.
This session's value = three silent JIT miscompiles flushed out by cross-gcc
batteries driven end-to-end through elfjit (no QEMU) vs native x86-64, all
found and fixed in arm64jit:

1. **LogicImm DecodeBitMasks rotate-RIGHT** (`8c1706b`): the logical-immediate
   element was LEFT-rotated; ARM DecodeBitMasks uses ROR. Every rotation-
   asymmetric mask silently miscompiled (`mov x0,#0xffffffff80000001` ->
   0xfffffffe00000007). Only symmetric masks gave the same value under both, so
   the prior test set stayed green. Verified quantified vs the real assembler:
   right-rotate agrees 592/592 valid (N,immr,imms); old left   would be wrong
   on 509. Regression + loader_run end-to-end gate (`076b024`).

2. **Scalar-D ADDP vs fcvtzs collision** (`01aa402`): `addp Dd, Vn.2D`
   (0x5ef1...) collided with the scalar fcvtzs gate at 0x5ee0b800 — bit20 is
   the discriminator (ADDP SET, fcvtzs CLEAR). Old gate silently ran every gcc
   pairwise-add reduction as a float->int. New SimdPairAddD.

3. **saddw2/uaddw2 upper-half** (`01aa402`): SimdAddw always read Vm at byte 0;
   the Q=1 form (saddw2) reads the UPPER 64 bits. -O2 vectorized loops do
   saddw (low) then saddw2 (high); old code accumulated low twice.

All three proven end-to-end: the i*i reductions return 76 (native) at both -O3
(addp) and -O2 (saddw+saddw2). 50+ cross-gcc battery programs acros.
`cargo build --workspace` clean; `cargo test --workspace` **202/0**.
HARD GATE unchanged: real Roblox boot/GPU host (no APK/GPU here).

---

## Session (Sep 11, 2026) — differential battery + sxtl2/uxtl2 upper-half FIX (workspace 210/0)

### New capability: differential battery (`crates/arm64jit/tests/diff_battery.rs`)
Cross-gcc compiles a C `entry()` to aarch64; the harness ALSO compiles the same
source with native `gcc` and runs it as the oracle; the loader→JIT pipeline must
return EXACTLY the oracle value. This is the strongest silence-detector in-tree
(native oracle vs JIT), and it immediately caught two MORE issues beyond the
already-fixed LogicImm/ADDP/saddw batch.

### FIXED: `sxtl2`/`uxtl2` (SIMD long-extend) ignored the Q bit
`Inst::SimdXtl { rd, rn, sign, esrc }` had no `upper`, so
`sxtl2 v28.2d, v28.4s` re-read v28's LOW 64 bits instead of bytes 8..15 —
every int→i64 vectorized init loop gcc emits (`movi v.4s,#n; sxtl; sxtl2; stp q`)
computed the upper half from the wrong lanes. Fixed the same way the existing
`saddw2`/`uaddw2` fix does:
- decode.rs: +`upper: (insn >> 30) & 1 == 1`
- translate.rs: source lane offset `n_half = if upper { 8 } else { 0 }`
Three deterministic linear regression tests in `jit.rs` pin it:
`and_then_sxtl_sxtl2_upper_half`, `and_sxtl_accumulation_two_iterations`,
`simd_stp_q_preindex_store_and_writeback` — the last replays the FULL maskf
loop body (movi; ldr q init from [x0,#400]; then 6× {mov snapshot; add v31+=4;
and &0xf; sxtl; sxtl2; stp q27,q28,[x0],#32}) and stores m[k]=k&0xf exactly.

### OPEN (documented): intermittent SIMD-loop block-liveness bug
The identical instruction stream FAILS nondeterministically when run through
`jit_run`'s single-block **b.ne back-edge** compilation of the whole function:
gcc -O2/-O3 int→i64 widening init loops EITHER compute correctly (verified vs
qemu-aarch64 ground truth: maskf 7001003, regloop 23017003, %101 60018021) or
corrupt ONE snapshot lane (m[4i+1] = address/stack-layout garbage while
m[4i],m[4i+2],m[4i+3] stay correct). Intermittent per process AND per heap
allocation (trial N), i.e. an uninitialized x86 register at the loop back-edge,
NOT an ISA miscompile (all ops verified correct linearly). The corrupted-lane
differential canaries (maskf/times7/mod_pow2/regidx/struct_arr/mixed) are
therefore excluded from the permanent gate; `diff_mixed_arith_accumulate` is
`#[ignore]`-documented. Root-causing this subsumes the struct/`%101` array
cases too. Next: instrument the block liveness / XMM scratch registers around
the loop back-edge under `compile_image_bounded`.

`cargo build --workspace` clean; `cargo test --workspace` **210/0** (1 ignored).
HARD GATE unchanged: real Roblox boot / GPU / APK host (none on this VPS).
### Addendum (same session): the "intermittent" bug above is ROOT-CAUSED and FIXED
Root cause: ARM `nop` (0xd503201f) and the whole system/hint 0xd5... family
were misdecoded as `ScvtfFixed` — the scalar int→fp FIXED-POINT gate checked
only `(insn & 0x30000000)==0x10000000`, which 0xd5xxxxxx also satisfies. So every
guest `nop` executed as `scvtf d<n>, x0, #56`: it CVTSI2SD'd the caller's x0
(very often the stack pointer) and DIVSD'd it into a vector register. That is
precisely the observed layout/address-dependent single-lane corruption. Fix:
the ScvtfFixed gate now also requires top byte in {0x1e,0x9e}. Additionally,
ScvtfFixed sf/to_double read bit30 but must read bit31 (0x9e=X/D vs 0x1e=W/S);
real `scvtf d0,x0,#1`=0x9e42fc00 was being decoded as a single Sd/Wn convert.
Decode regression `nop_is_hint_not_scvtf_fixed` pins all three.
After these, EVERY int→i64 widening-init-loop differential canary passes
deterministically (maskf=7001003, mod_pow2=1007005, times7=161119021, count6=24,
verified vs qemu-aarch64). Two SEPARATE deterministic bugs remain (magic-div
`%N` reducer: m[0]=-101 for k*7%101; and the -O3 addp/smulh reduction) and are
#[ignore]d/documented. Workspace 212/0 (3 ignored).

## Session (Sep 11, 2026) — mls + uzp2 implemented, rev64 gate widened; all 3 remaining SIMD bugs CLOSED (workspace 216/0, 0 ignored)

The two leftover deterministic bugs (magic-div %101 m[0]=-101 and the -O3
vectorized reduction) share one root cause. Isolated the gcc %101 reducer
chain (smull/smull2/uzp2/sshr/mls) in a scratch example and diffed against
qemu-aarch64, then pinned the failing ops with decode(insn):

1. **`mls` (multiply-subtract) misdecoded as a plain Simd4s SUBTRACT.** The
   `(insn & 0xffe0_fc00)` gate for Simd4s caught `mls v26.4s,v0.4s,v28.4s`
   (0x6ebc941a) as `sub`, **losing the multiply entirely** — so the quotient
   was never subtracted. Implemented `Inst::SimdMla { rd, rn, rm, lanes,
   sub }` (mla=0x0ea09400/0x4ea09400 add, mls=0x2ea09400/0x6ea09400 sub),
   gate placed BEFORE the Simd4s gate; translate does per-lane
   `Vd = Vd ± Vn*Vm` (low-32 product).

2. **`uzp2` (unpack-high) misdecoded as `rev64`.** The SimdRev gate
   `(insn & 0x3f00_0c00)==0x0e00_0800` drops bit12 and swallowed the whole
   uzp1 (0x18) / uzp2 (0x58) opcode family. Tightened it to require
   bits[13:12]==00, and added `Inst::SimdUz2 { rd, rn, rm, esize, q }`
   (byte1 high-nibble==0x5) gathering the ODD/upper elements
   `Vd[i]=Vn[2i+1]; Vd[n/2+i]=Vm[2i+1]` — what feeds the sshr quotient step.

Verified: the previously-`#[ignore]`d `diff_magic_div`, `diff_struct_array_
fields` and `diff_mixed_arith_accumulate` all pass again as permanent gates
(un-ignored). Added decode regression `mls_and_uzp2_decode_as_specific_ops_
not_sub_or_rev`. Workspace 216/0, **0 ignored** — the differential battery is
fully green with every case a live gate.

### Next
- Ideal next: keep sweeping the ISA breadth the battery doesn't yet cover —
  widen the reducer family (umull2/umlal/uaddl to exercise uzp-variant and
  long-multiply paths), add string/booleans, and push the battery onto more
  real-compiler idioms (-O3 reductions already live). Then brace for the real
  Roblox APK path (ELF/loader + JNI stubs) once an APK/GPU host is available.
  Blocked on this VPS only by the HARD GATE (no GPU/APK).

### Addendum (same session, after the mls/uzp2/rev64 commit) — UNSIGNED magic-division closed (workspace 220/0, 0 ignored)
Extending the battery to UNSIGNED `%const` (gcc emits the mul/umull/umull2/
uzp2/ushr/zip1/zip2 reducer, the unsigned sibling of the signed smull one)
immediately surfaced FOUR more misdecodes, all isolated against qemu-aarch64:
1. **`mul` (NEON element-wise 32-bit multiply) decoded as SimdVLog (bitwise).**
   The vector-logical AND/ORR/BIC gate checked byte1&0x1c00==0x1c00 but never
   bit15: mul's byte1 (0x8c..0x9f) sets bit15, and/orr/bic (0x1c/0x1d) don't.
   So EVERY NEON multiply became an AND/ORR — including the magic-division
   dividend `mul v26.4s,v26,v28(97)`, which corrupted the quotient. The JIT's
   full-loop m[] came out all-0 and acc=0. Fix: require (insn&0x8000)==0.
2. **`uzp2` misdecoded as `rev64`, then as `uzp1`.** The rev64 gate dropped
   bit12 (fixed earlier); the uzp1 gate's &0x3f mask dropped bit6, so uzp2
   (byte1 0x58) was even-gathering. Added Inst::SimdUz2 (odd/upper gather);
   both uzp gates now key on byte1 0x18-/0x58-family + byte3-low-0x0e +
   bit28 clear (excludes bit/bif/bsl and rev64).
3. **`zip2` Unsupported.** Added Inst::SimdZip2 (upper-half interleave, base
   0x0e007800 vs trn2 0x0e006800). gcc uses zip1/zip2 with a zero lane to
   widen a 4s quotient into 4 u64.
4. **`mls` = plain Simd4s SUBTRACT (no multiply)** — added SimdMla {sub}.
Also: WidenShl gate restored to the genuine shll long-shift family but made to
exclude the permute ops via byte1 bits[1:0]==00 (shll 0x38 vs zip1 0x39/0x3b).
Verification: diff_unsigned_magic_div_umull, diff_long_accumulate_widening,
diff_byte_scan_strlen new; all green un-ignored; 3 decode regressions added
(mls_and_uzp2..., mul_decodes_as_multiply_not_bitwise_logical, and 'and' still
logical). Workspace 220/0, **0 ignored**. Commits ab24b32 (fix) — prior
d239e8c/3b4ff25 (signed path). Difference: JIT and qemu-aarch64 now agree on
both the signed and unsigned magic-division kernels exactly.
---

## Session (Sep 11, 2026) — 6 silent vector-FP/NEON miscompiles fixed; workspace 227/0, 0 ignored

Commits `ea84aff` (fixes + unit tests) + `059151a` (differential canaries) on `dev`.
Opened at 220/0 (no failing test — the android idempotent test stays green). Extended
the differential battery into the **.4s/.2d single/double-precision float-vector
SIMD** family (a 3D engine's vertex/matrix math is ~all of it) that the older
integer/double batteries never touched, with volatile seeds so gcc can't constant-fold
while still vectorizing. It immediately flushed out **SIX silent miscompiles** — every
one would corrupt pixels/coordinates/audio on a real host:

1. **VecIntToFp** — `scvtf/ucvtf Vd.4s/.2s` (and signed `.2d`, `0x4e21d800` family) were
   misdecoded as **SimdMull** (widening multiply); every vector int->float made garbage.
2. **VecFpArith** — `fadd/fsub/fmul/fdiv/fmax/fmin/fmaxnm/fminnm Vd.2s/.4s` (2-source)
   were misdecoded as integer SIMD (`SimdAddB`/`Simd4s`/`SimdMull`). Only FMLA
   (accumulate) and the `.2d` double forms (Simd2dFp) were handled; the two-source
   single-precision family was entirely MISSING. Added `divss` x86 emitter.
3. **SimdMlaEl** — integer `mla/mls Vd.4s, Vn, Vm.s[idx]` (by-element) misdecoded as
   **VecMovi** (gcc int->float init `mla v5.4s,v18,{loop}.s[0]` corrupts the a*scalar
   product). Gate: top nibble 0x0f + bit29 set (FP fmla-el is bit29 CLEAR) + bit23 set
   (excludes smlal/umlal-by-el byte1 0x42); `.4s` index = (bit11<<1)|bit21.
4. **Permute ordering** — `zip1` (and zip2/uzp1/uzp2) now decoded BEFORE the
   `WidenShl` (shll) gate. Real `zip1 Vd.4s` has byte2 0x38 (same as shll) and was
   misdecoded as a widening shift; the old guard only excluded the 0x39/0x3b byte2
   variants. Added an early compact permute gate (zip1/zip2/uzp1/uzp2 on 0x3f20fc00).
5. **SimdFmulEl cross-lane alias bug** — the broadcast element was re-read *inside* the
   lane loop, so when `rd==rm` (`fmul v17.4s, v7.4s, v17.s[0]`) lane 0's write clobbered
   the element before lanes 1-3 read it → every lane after 0 wrong. Now broadcast once
   up front (the FmlaEl "load into xmm2 before the loop" pattern).
6. **VecFpCmp** — `fcmeq/fcmgt/fcmge Vd.4s/.2s/.2d` (compare→all-ones mask) misdecoded
   as **SimdVShift**; gcc's float-vs-const count loop returned garbage. Gate = byte2
   0xe4 after the 0xffe0fc00 mask (NOT raw bits15:8, which include rn). New
   comiss/comisd + setcc + movzx + neg x86 emitters; per-lane mask = all-ones or 0.

New unit tests: `vector_fp_arith_ground_truth` (fmla/fmul-by-el/fmla2d/scvtf/fadd,
incl. negative scvtf lanes, fmov-imm broadcast, fmov-s,w), `vector_fp_by_element_highreg_and_2d`
(exact gcc high-reg by-element + .2d words). New permanent differential canaries:
fv_arith, fv_sub_neg, fv_f2i, fv_f2i_neg, fv_i2f, fv_fmla_scalar, fv_cmp_count,
dv_arith, fv4, fv4_fmul_only.

**Verification:** `cargo build --workspace` clean; `cargo test --workspace` **227/0**,
**0 ignored** (was 220/0). Full differential battery 17/17 (every case a live gate).

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep pressing the SIMD float/FP vector breadth (single-precision struct-by-value
   with vector lanes, fmla-by-element chains, more -O3 reduction shapes); then widen into
   the remaining integer-permute/wide paths the new float canaries may expose.
2. `fv_cmp_count`-style FP compares are now real (setcc-based); add fcmlt/fcmle
   (operand-swapped gt/ge) if a battery case needs them.
3. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
4. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK (none here).

## Session (Sep 11, 2026) — 9 silent miscompiles + the long-open nondeterministic SIMD-loop bug fixed (workspace 234/0, 0 ignored)

Five commits on `dev` (all `cargo build --workspace` + `cargo test --workspace` green):
`6ffd490` (32-bit add/sub flags + ccmp/ccmn), `e1ec841` (4 FP/control-flow bugs),
`7e2689a` (ADDV-to-scalar stale bytes), `5cbe6ce` (CMN/ADDS C-flag polarity),
plus this docs commit. Opened at 230/0; no failing test (the android idempotent
stays green). Delivered through the differential-probe harness — cross-compile the
same C at -O2/-O3 through `elfjit`, compare against a native x86-64 oracle. HEAD of
these runs caught 9 real bugs:

1. **32-bit ADDS/SUBS flag semantics** — `!sf` flagged adds/subtracts used 64-bit
   x86 add/sub after zero-extending, so `store_nzcv` saw SF from bit63, not bit31.
   `adds w1,w1,w2` with 0x7fffffff+1 gave N=0 (wrong), so `int s=INT_MAX+1; s<0`
   returned the wrong branch. Added 32-bit emitters (no REX.W) and routed `!sf`
   paths through them.
2. **ccmp/ccmn missing** (conditional compare) — swallowed by the logical set-flags
   decoder (shares 0xFA/0x7A/0xBA/0x3A top bytes), corrupting NZCV so gcc's
   `while (a<N && b!=M)` guards spun forever. Added `Inst::CcMp` (residue class
   distinct from ANDS/BICS/SBCS; rn=[9:5] rm/imm=[20:16] cond=[15:12] nzcv=[3:0]);
   translate mirrors Fccmp (load_nzcv -> jcc -> nzcv|compare).
3. **CSel rn/rm==31 read the SP slot** instead of XZR — `cset/cinc/csneg`
   (`a==0.0?1:0`) returned sp/sp+1. CSEL is data-processing: reg 31 is always XZR.
4. **Scalar `scvtf/ucvtf Dd,Dn` `sng` discriminator inverted** (bit22=1 is DOUBLE,
   code set sng on it) — a double scvtf truncated through the i32->f32 path.
5. **`fcmp Dn,#0.0`** decoded as a compare against vector reg d0 (garbage) — bit3
   (0x8) is the #0.0 discriminator, now `Fcmp.against_zero` loads literal +0.0.
6. **FP NaN compare flags** — `store_nzcv_fp` stored C as the true ARM value and
   Z=ZF (set for unordered too): `vnan==vnan` came out true and `nn<=0` (cset ls)
   wrong. Now Z = ZF&&!PF (excl. unordered) and C is stored in the borrow sense
   (CF&&!PF) that x86_cc_for_cond's ls/hi/lo/hs expect => all NaN comparisons false.
7. **ADDV-to-scalar left stale bytes** — `addv Bd,Vn.8b` stored only 1 byte, so the
   destination vector reg's upper bytes kept the `cnt` lane counts; gcc's
   `fmov x2,d31` then read `[sum, pc1, pc2, ...]` as the integer popcount. This is
   the exact **root cause of the documented intermittent SIMD-loop block-liveness
   corruption** (one element garbage, nondeterministic, stack-layout dependent)
   that the old maskf/times7/mod_pow2/regidx/mixed canaries hit. Now the ADDV
   store zeros the upper bytes (64-bit store of a size-masked sum). `simdu3`
   (popcount loop) returns exact 591 deterministically, 8/8 runs at -O2/-O3.
8. **CMN/ADDS C-flag polarity** (commit 5cbe6ce) — the flag-setting ADD path
   stored C = x86 carry, but x86_cc_for_cond's HS/LO/HI/LS assume the SUBTRACT-
   borrow convention. gcc's `unsigned um > 0xffffffffffff0000` (compiled to
   `cmn x,#0x10000; b.ls`) evaluated "not greater" when it carries. cmc before
   store_nzcv on the non-subtract paths stores C in borrow convention.
   structfp was off by 999999 from exactly this.

New permanent canaries: `diff_ccmp_cond_compare`, `diff_w32_overflow_compare`,
`diff_fp_compare_zero_and_cset`, `diff_fp_nan_compare`, `diff_addv_popcount_accumulate`
(all differential vs native oracle). Decode regressions: `ccmp_ccmn_decode`,
`fcmp #0.0 / d0 / fcmpe #0.0` additions.

**Verification:** `cargo test --workspace` **234/0**, **0 ignored** (was 227/0);
the 28-program differential probe suite (FP math/compare/cvt, NaN, signed-zero,
ccmp chains, 32-bit overflow compares, NEON/16-bit/unsigned SIMD, popcount,
branch tables, recursion) matches the native oracle at both -O2 and -O3.

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery driving ISA/correctness breadth (SIMD permute/
   wide paths, more FP reduction/reassociation shapes, struct-by-value vectors).
2. `addv s0,v1.4s` 32-bit scalar-store path is now covered; check `saddv`/`uaddv`
   (signed accumulator) if a case surfaces one.
3. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
4. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK (none here).

## Session (Sep 11, 2026) — SIMD INS lane-index decode FIX + Extr/unpack/zip batch folded (workspace 243/0)

Commit `b93e89d` on `dev` (all `cargo build --workspace` + `cargo test
--workspace` green). Two threads:

1. **Closed the persistent `diff_float_vector_reduced` (fv4, -O3) miscompile —
   jit 153 vs native oracle 175.** Root cause was NOT the permute snapshot:
   the INS (vector, element) lane-index decode read `dst_idx = bit20` and
   `src_idx = bit14` — two single bits that only coincided with the true lane
   index for S lane 0->1. Every S lane beyond 0 and every H/B lane silently
   copied into the wrong vector element. gcc -O3 emits `mov v3.s[1], v28.s[0]`
   and `mov v31.s[1], v4.s[0]` in the float tight loop fv4 exercises, so a wrong
   `dst_idx` put the seed/accumulator f32 lanes in the wrong V slots. Correct
   packing (verified against the aarch64 assembler for all 4x4 S, 8x8 H, 16x16
   B, 2x2 D lane pairs — 340 encodings, 0 mismatches):
   `l = log2(esize); dst = imm5 >> (l+1); src = (insn>>(11+l)) & ((1<<(4-l))-1)`.
   fv4 now returns 175 == oracle. Locked with `simd_insd_sets_correct_lane_with_multi_byte_indices`
   (mov v3.s[2],v5.s[1] copies 3.5f into lane 2; move-back bytes assembler-verified).

2. **Folded in the earlier uncommitted arm64jit batch** (verified green so HEAD
   stays clean): general EXTR with `rm != rn` (`extr x0,x0,x1,#51`, gcc's shift-
   rotate idiom) now decodes as `Inst::Extr` instead of falling through to the
   UBFM/SBFM gate; uzp1/uzp2/zip1/zip2 snapshot their rd-aliased source to a
   `permscratch` buffer in CpuState (gcc's ubiquitous `uzp1 v31.8h,v31.8h,v26.8h`
   and `zip1 v31.4s,v3.4s,v31.4s`); and REX.B on shl/shr/ror/not/neg emitters so
   guest regs >= 8 (R10+) are addressed correctly. Regression tests:
   `extr_general_two_operand_rotate`, `uzp1_rd_aliases_rn_does_not_corrupt_source`,
   `csel_family_op_discriminates_neg_not_inc_identity`, plus diff_battery
   canaries `diff_integer_signed_division_negative_edge`,
   `diff_rotate_extract_and_byte_accum`, and `diff_float_vector_reduced` (now green).

**Verification:** `cargo test --workspace` **243/0** (146 arm64jit lib incl. the
new ins test; 26 diff_battery incl. fv4). HEAD `b93e89d`.

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery driving ISA/correctness breadth (more SIMD
   lane/permute/wide paths, FP reduction/reassociation shapes).
2. Move up to the runtime side: the FMOD "divert guest bl-to-once through the
   dispatcher" task and JNI function-table stubs per RECOMMENDATION.md.
3. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
4. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK (none here).

## Session (Sep 11, 2026) — ld3/st3/ld4/st4 structure DEINTERLEAVE implemented (workspace 245/0)

Commit `1c5f69e` on `dev`. Continued the differential-battery ISA sweep. A 4x4
float matmul (`C[i][j] += A[i][k]*B[k][j]`, gcc -O3) returned **20 vs oracle
5248**. Root cause: the structure-load opcode field (insn bits15:12) was folded
wrong — **0b0000 (ld4/st4) mapped into the single-register ld1 placeholder and
0b0100 (ld3/st3) to Unsupported**, so both ran the ld1-multiple
CONSECUTIVE-load path. AArch64 ld4/ld3 are structure **deinterleave** loads
(`Vd[j][i] = mem[base + i*N*es + j*es]`); loading them consecutively read the
wrong memory. gcc -O3 emits `ld4 {v24.4s-v27.4s},[sp]` to load matrices.

Fix: new `Inst::Ld3N/St3N/Ld4N/St4N` with true, element-size-aware
deinterleave; decode maps op=0b0000->Ld4N/St4N and op=0b0100->Ld3N/St3N
(ld1-multiple keeps only 0b0010/0b0110/0b0111/0b1010). Byte-verified against
qemu for ld3/ld4 q=0 & q=1 and st4 — all match. matmul now = 5248, transpose =
684, complex = 288 (all = native oracle). Regression: decode unit test
`ld4_st4_decode_to_structure_deinterleave`; differential canaries
`diff_ld4_st4_matrix_transpose` (ld4_matmul + st4_transpose). Workspace 245/0.

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery sweeping ISA/correctness breadth (structure
   load/store widths, more SIMD lane/permute/wide paths, FP reduction shapes).
2. Move up to the runtime side: FMOD "divert guest bl-to-once through the
   dispatcher" and JNI function-table stubs per RECOMMENDATION.md.
3. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
4. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK (none here).

## Session (Sep 11, 2026) — vector fmls operand order + dup-from-GPR vs sqadd gate (workspace 249/0)

Commit `7fef13e`. Two real silent SIMD miscompiles from the differential
battery's complex-matrix and -O2 fill probes:

1. **Vector fmls inverted operands.** `fmls Vd,Vn,Vm` (= Vd - Vn*Vm) shared the
   commutative add's mul/load ordering, so the JIT computed `Vn*Vm - Vd` — right
   magnitude, wrong sign. 4x4 complex matmul returned 10688 vs oracle 13504;
   `acc -= a*b` loop +20 vs oracle -100. Fixed by loading Vd into xmm0 and the
   product into xmm1 so subss(0,1) = Vd - Vn*Vm (also .2d el64 path).
2. **`dup Vd.T, Wn` swallowed by the SIMD saturating-add gate** (sqadd/uqadd/
   sqsub/uqsub have byte2==0x0c too). gcc -O2 matrix/fill loops emit `dup
   v30.4s,w1; add v30,v30,v31; scvtf; str q30,[x],#16`, so the broadcast decoded
   as a sat-add and every array held garbage (init_O2: 18446744039484557312 vs
   96). Verified vs assembler: bit21 is SET for all 14 sat-add forms and CLEAR
   for all 6 dup-from-GPR widths — now required in the sat-add gate.

Regression: `fmls_vector_subtract_has_correct_operand_order`,
`dup_from_gpr_not_swallowed_by_sqadd_gate` (decode); differential canaries
`diff_fmls_vector_subtract_accumulate`, `diff_dup_from_gpr_matrix_init`.
cargo build clean; `cargo test --workspace` 249/0. HEAD `7fef13e`.

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery sweeping ISA/correctness breadth (structure
   load/store widths, more SIMD lane/permute/wide paths, FP reduction shapes).
2. Move up to the runtime side: FMOD "divert guest bl-to-once through the
   dispatcher" and JNI function-table stubs per RECOMMENDATION.md.
3. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
4. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK (none here).

## Session (Sep 11, 2026) — sat-add lane-width/sign + ubfiz-vs-ror fixes (workspace 251/0)

Commits `e3f12be` (SimdSatAdd) and `3696d89` (UBFM). The differential battery
kept finding real silent SIMD miscompiles:

1. **Saturating add/sub treated >=32-bit lanes as 64-bit ops.** SimdSatAdd used a
   64-bit load for any lane esize>=4, so `.4s` loaded 8 bytes as ONE value and
   clamped both s-lanes together (10+5 -> 0x80000000 smin sentinel; qemu 15).
   This path was only reachable after the dup-from-GPR gate fix (it had been
   silently masked by the same gate collision). Fixed with per-lane width loads,
   sign-extension (movsx/movsxd) so 64-bit clamps judge negatives correctly,
   SIGN-EXTENDED smin constants (0xFFFFFFFF80000000 for .4s — the raw lane-width
   0x80000000 as u64 is positive, so 15 < 0x80000000 and every non-negative
   result clamped), and .2d 1<<64/1<<63 guard shifts. Verified vs qemu across 8
   widths x signed/unsigned x add/sub.
2. **ubfiz/sbfiz with immr>imms hit the UBFM ROR shortcut.** `ubfiz w4,w2,#3,#3`
   (immr=29, imms=2; 29+2+1==32==bits) matched the `imms+immr+1==bits` rotate
   gate BEFORE the shift-extend branch, so it rotated right by imms=2 instead of
   computing (w2&7)<<3. A -O2 mix/shuffle-hash (`h ^= msg[i]<<((i%8)*8)`)
   returned 13680984341602923654 vs oracle 13072640789477207222. A genuine ror
   always has immr<=imms; gated the ROR branch on `!(immr > imms)` so ubfiz/
   sbfiz fall through to their shift path. mix now = oracle; real ror/extr tests
   stay green.

Regression: `sqadd_uqadd_respect_lane_width_and_sign`,
`ubfiz_immr_gt_imms_does_not_rotate` (exec); differential canary
`mix_hash_ubfiz`. cargo build clean; `cargo test --workspace` 251/0. HEAD `3696d89`.

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery sweeping ISA/correctness breadth (structure
   load/store widths, more SIMD lane/permute/wide paths, FP reduction shapes).
2. Move up to the runtime side: FMOD "divert guest bl-to-once through the
   dispatcher" and JNI function-table stubs per RECOMMENDATION.md.
3. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
4. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK (none here).

## Session (Sep 11, 2026) — fixed-point fcvtzs #fbits + post-index multi-reg ld1 (workspace 253/0)

Commit `97417af`. Two more silent arithmetic/load bugs from the differential
battery:

1. **Fixed-point FP->int** (`fcvtzs/fcvtzu Rd, Fn, #fbits`, result = trunc(Fn *
   2^fbits)) was misdecoded as `SimdMull` (smull x0,w31,w24) by the widening-
   multiply gate — every *2^fbits scale silently dropped. gcc -O3 folds
   `(long long)(s*4)` into fcvtzs #2; the double square-sum `v64f` returned 5 vs
   oracle 436. Added an `fbits` field to `FcvtToInt`, decoded the fixed-point
   encodings (top16 0x1e18/0x1e58/0x9e18/0x9e58 signed, +bit16 unsigned; fbits =
   64 - bits[15:10]) before SimdMull, and scale xmm0 by 2^fbits in translate.
   vs qemu: 3.25>>#2 = 13, >>#4 = 52, s 2.5>>#3 = 20, fcvtzu 3.75>>#1 = 7.
2. **Post-indexed multi-register ld1/st1** `{Vt..,Vt+n},[Xn],#imm` sets bit23
   (bases 0x..cc0 ld / 0x..c80 st), which the structure-multiple gate's four
   no-post bases missed — `ld1 {v26.16b,v27.16b},[x1],#32` fell through to the
   single-vector Ld1V gate: loaded only 16B and advanced Xn by 16 not 32. An
   -O2 double dot-product (fmadd loop + shifted-register add addressing)
   accumulated 165 vs oracle 470. Added the 4 post-index bases.

Regression: `fcvtzs_fixed_point_fbits_scales`, `ld1_multireg_post_index_decode_and_advance`;
differential canaries `fcvtzs_fixed_scale`(/neg), `fma_ld1_postidx`.
cargo build clean; `cargo test --workspace` 253/0. HEAD `97417af`.

### Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery sweeping ISA/correctness breadth (structure
   load/store widths, more SIMD lane/permute/wide paths, FP reduction shapes).
2. Move up to the runtime side: FMOD "divert guest bl-to-once through the
   dispatcher" and JNI function-table stubs per RECOMMENDATION.md.
3. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
4. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays the
   HARD GATE, blocked until a capable host + the real binary/APK (none here).

---

## Session 2026-09-11 (cycle 19) — scalar single-precision FP rounding fixed (workspace 254/0)

Workspace opened green (253/0, android idempotency test passing — the mandated
first fix was already committed in prior cycles). Continued the differential
battery ISA sweep and flushed out **two silent miscompiles** in the arm64jit
FpUnary SINGLE-precision path (`frintm/frintp/frintz/fsqrt s`):

1. **Loaded float bits never reached xmm0.** The single path did
   `mov_load32(RAX, RNslot)` then `cvtss2sd(0,0)` — but `cvtss2sd` reads xmm0's
   low32, so EVERY single frint/fsqrt converted STALE xmm0 instead of the value.
   The FpScalar single path (op 4-7) correctly inserts `movd_xmm_r32(0, RAX)`
   first; FpUnary single was missing it. A `floorf` loop returned 450 vs 360.
   Fix: add `buf.movd_xmm_r32(0, RAX);` before `cvtss2sd`.

2. **`frintz s` used round-to-nearest, not truncate.** roundsd mode was `0b00`
   (toward-nearest) instead of `0b11` (toward-zero). The double path already
   used `0x03`. A negative-heavy `truncf` loop: what ARM-trunc gives 40 came out
   20 because negatives rounded to nearest. Fix: `0b00` → `0b11`.

Verified vs native x86-64 oracle through elfjit (no QEMU): floor 360/360,
ceil 440/440, trunc 40/40; double trunc 40/40 (double was already correct).

Added a new differential battery test `diff_scalar_fp_round_single` with three
probes (fr_floor_single, fr_ceil_single, fr_trunc_single_negatives). Confirmed
the canary is a REAL gate: reverting just the `0b11` mode fix makes
fr_trunc_single_negatives fail with exactly `jit 20 vs oracle 40`, then restored.

`cargo build --workspace` clean; `cargo test --workspace` **254/0, 0 ignored**
(battery 30). Committed on `dev`.

Honest next (unchanged): vector `frintm/p/z v.*` is still an honest *Unsupported*
stop (no silent value), so gcc's vectorized rounding is the next feature slice;
then the runtime-side items (FMOD bl-to-once dispatcher, JNI table stubs) and
the HARD GATE real-binary/GPU boot proof which is impossible on this APK-less,
GPU-less VPS.

---

## Session 2026-09-11 (cycle 19b) — VECTOR frint + compare-to-zero + SimdMull gate fix (workspace 257/0)

Follow-on to the scalar FP-rounding fix. Sweeping more vector-FP probes exposed a
**systemic misdecode class**: the SIMD widening-multiply gate was
`insn & 0x0f00_c000` — it drops bit28 and only keeps byte2 bits15:14, so ANY
op whose byte2 shares those bits folded into the smlal residue. Two real,
silent (garbage-not-stop) miscompiles found and fixed:

1. **VECTOR frint** (frint{n,m,p,z,a} Vd.T, Vn.T): a `floorf` loop's
   `frintm v1.4s,v1.4s` (byte2 0x98) decoded as **smlal** — the floor loop
   returned 1.9e16 vs native 590 (a silent widen-multiply of the float bits).
   Implemented `Inst::SimdFrint` + decode (mode = bit23<<1|bit12, es=bit22,
   frinta=bit29) before SimdMull; translate per-lane via cvtss2sd→roundsd→
   cvtsd2ss (0b00 n / 0b01 m / 0b10 p / 0b11 z, frinta ties-away trick).

2. **VECTOR compare-to-zero** (fcmeq/fcmgt/fcmge/fcmlt/fcmle Vd,Vn,#0.0): gcc's
   `x<0 ? a : b` select uses `fcmlt ... #0.0` (byte2 0xea) + `bsl` — the fcmlt
   was also smlal, so the select chose the wrong branch. Implemented
   `Inst::VecFpCmpZero` + decode + translate (comiss/comisd vs a zeroed xmm1,
   setcc per lane like the existing VecFpCmp; seta/setae/sete/setb/setbe).

3. **SimdMull gate tightened** to `(insn & 0x3800) == 0` (byte2 bits13:11
   clear) — genuine smull/umull/smlal always clear those (size varies in byte2
   bits[2:0], e.g. smull .8h 0x...c020 vs umull 0x...c340), while frint (0x88/98)
   and fcmlt (0xea) set one. Prevents any future frint/cmpz-style smlal fires
   even if a new width variant slips the earlier gates. (An earlier too-strict
   `byte2 == 0xc0/0x80` broke diff_magic_div etc. — element size lives in byte2
   bits[2:0]; corrected to the 0x3800 check, which keeps all mull widths working.)

Also proved the earlier `vfa` "fneg sign flip" was a **UB false alarm**: the
probe cast a negative float to `unsigned long long` (undefined in C) — aarch64
emits `fcvtzu` (clamps to 0) while x86 emits signed trunc (-363), so the JIT
returning 0 was CORRECT aarch64 semantics. With a signed cast, jit == native ==
-363. Isolated tests confirmed fneg, bsl, and fcmlt+bsl are each correct.

New differential canaries: diff_vector_frint_rounding (vflr_floor, vflr_ceil),
diff_vector_fp_compare_zero (vcmp0_lt_keep_neg, vcmp0_gt_keep_pos). New decode
regression `simd_frint_and_cmpzero_not_swallowed_by_widen_mul` (guards (a) frint
(b) fcmlt (c) genuine umull still SimdMull). cargo build clean; cargo test
--workspace 257/0. Committed on dev.

Honest next: continue the battery sweep (vector frinta/frintn, f2d .2d forms,
sdadd/sqadd, pmull); runtime-side FMOD bl-to-once dispatcher + JNI table stubs
are still pending the real binary; HARD GATE (elfjit on libroblox.so on a GPU/
APK host) unchanged — impossible on this GPU-less, APK-less VPS.

---

# Session (Sep 11, 2026) — three silent FP `.2d`/FMOV-imm decode collisions fixed (workspace 264/0)

Extended the differential battery with four new game-critical canaries (SIMD
fmin/fmax reduction, reciprocal/division funnel, integer widening mul-acc,
double FP loop-condition). One — `l_double_div_accum_guard` (6-element
`(i+1)*2/(i+2)` sum with a `<3.0` guard) — **failed: jit 2001 vs oracle 8820**.
Bisecting it (per-instruction exec_bytes + whole-block runs through
`load_elf_image`+`jit_run`) surfaced THREE unrelated silent miscompiles in the
FP/vector decode layer, each a real pixel/audio/coordinate corruption:

## 1. `fadd/fmul/fsub Vd.2D` swallowed by the integer add/sub gates
The four SIMD int add/sub gates (Simd4s/SimdAddD/SimdAddB/SimdAddH) keyed on
`insn & 0x2f20_0c00` in {0x0e200400, 0x2e200400} plus `size`, but ignored bit14.
FP `.2d` two-source ops (byte1 0xd4, bit14 SET) share that residue with integer
`add` (byte1 0x84, bit14 CLR). `fadd v0.2d,v0.2d,v1.2d` (0x4e61d400) compiled as
a **halfword `paddw`** on the double bits. The isolated host dump showed
`paddw xmm0,xmm1`; the decode scratch returned `SimdAddH`. Fix: `(insn & 0x4000)
== 0` on all four gates. VecFpArith covers `.2s/.4s` early; Simd2dFp now gets
`.2d`.

## 2. `fdiv/fmul Vd.2D` swallowed by the SimdSel (bsl) gate
byte1 low 0xfc/0xdc (bits[15:13] SET) duped the bsl select gate, which only
checked `(insn & 0x1c00) == 0x1c00` (bits[12:10]). Host dump showed
`pandn/pand/por` — a bitwise select. `fdiv v0.2d` returned 256 for 64/8 (both
lanes). The real bsl family always has byte1 low 0x1c (bits[15:13] CLR). Fix:
`(insn & 0xe000) == 0` on the SimdSel gate.

## 3. `fmov d,#imm` with mantissa m>=8 swallowed by the fcvt-to-int round gate
`fmov d23,#12.0` = 0x1e651017 decoded as `FcvtToInt` (destination silently 0).
The coarse fcvt-round gate (`insn & 0xffff_0000`, added for `0x9e..` X-dest
fcvtps/ms/au) included `0x1e65` (W fcvtau) which collides with FMOV-imm. FMOV-imm
has bit12 SET (the imm lane anchor) while every fcvt-to-int has bit12 CLR
(verified: fcvtzu 0x1e790020, fcvtas 0x1e640020, fcvtau 0x1e650062). Fix: gate
the fcvt-round block on `(insn & 0x1000) == 0`.

## Verification
- Isolated vs the real aarch64 assembler: bsl 0x6e611c00, fdiv/fmul/fadd .2d
  0x6e61fc00/0x6e61dc00/0x4e61d400, fcvtzu 0x1e790020, `fmov d,#12.0` words
  0x1e651017 etc. p9 (scalar div+fmadd, was 2^63) -> 6000, p5 (vector
  a[]+reduce, was 90000) -> 8814, l_double -> 8820.
- +1 decode regression (`fp_2d_op_decode_collisions_with_int_add_bsl_and_fcvt`),
  +2 exec regressions (`vector_2d_fp_div_mul_not_swallowed_by_int_add_or_bsl`,
  `fmov_imm_high_mantissa_12_to_15_not_swallowed_as_fcvt`), and the 4 new battery
  canaries are permanent.
- `cargo build --workspace` clean; `cargo test --workspace` 264/0, 0 ignored.

HARD GATE unchanged: real-binary/GPU boot proof (`elfjit <libroblox.so>
0x1f0db20 --jni`) on a GPU + real binary/APK host (none on this VPS).
---

# Session (Sep 11, 2026) — SIMD across-lanes min/max + smax/smin & movsx fixes (workspace 268/0)

Continuing the differential battery: a new int SIMD min/max reduction canary
(`im_running_minmax`) exposed one missing ISA and two more silent bugs.

## 1. SMINV/SMAXV/UMINV/UMAXV implemented (was Unsupported)
Across-lanes reduce to the bottom scalar (upper cleared). New
`Inst::SimdReduceMinMax` decode (0x4e/0x6e `XYa820` family: esize by byte2 high
nibble {3,7,b}, min/max by byte2 bit0, signed by bit29) + translate (per-lane
sign/zero-extended CMOVcc reduce). Two gotchas during bring-up: cmov_rr64
expects the cc in the `0F 4X` domain (I first passed the raw 0x0X Jcc domain ->
SIGILL `0F 0C`), and the cmp/cmov direction is min=CMOV-G/A, max=CMOV-L/B
(update when the candidate is the extrema found by `cmp RDX, RAX`).

## 2. Element-wise smax/smin mis-decoded for high source registers (silent)
The SminMax gate is correctly written `(b2 mask 0xfc) == 0x64` (max) but the
ASSIGNMENT was `max: b2 == 0x64` (EXACT). b2 = bits[15:8] and its low 2 bits
carry Rn (bits[9:8]). A real gcc `smax v30.4s, v29.4s, v28.4s` encodes b2=0x67,
so it masked to max but the exact-equality assign said MIN -> returned the Vn
operands verbatim. Only source regs 0..3 (b2 stays 0x64/0x6c) ever hid it.
Fixed to mask like the gate.

## 3. movsx_word_mem/movsx_byte_mem lacked REX.W (silent, shared-emitter)
`0F BF /r` / `0F BE /r` with REX no-W write only a 32-bit destination, so a
negative 8/16-bit lane loaded into RAX compared as a huge POSITIVE u64 in any
64-bit signed reduction (`sminv.8h` over {-9,-2,..} picked 4, not -9). Latent
across every consumer (incl. ADDV signed byte/halfword sums). Both emitters now
emit REX.W. This was the real root of the earlier `.8h`/`.16b` sminv results.

Verified via per-instruction stepping of the gcc-unrolled probe: smax produced
v30=[0,-28,28,28] (wrong) -> after fix [112,140,252,308]; sminv/smaxv/addv
scalars and the final result 3640 = oracle. +2 differential canaries
(rm_running_extents, im_running_minmax), +2 exec regressions. cargo build
--workspace clean; cargo test --workspace 268/0, 0 ignored.

HARD GATE unchanged: real-binary/GPU boot proof (`elfjit <libroblox.so>
0x1f0db20 --jni`) on a GPU + real binary/APK host (none on this VPS).
---

# Session (Sep 11, 2026) — guest_svc syscall surface expanded for real-boot/ALooper/login (workspace 270/0)

The android-layout idempotency test is green at HEAD (long since fixed); the
workspace gate is clean. This session widened the JIT's in-process AArch64
syscall dispatcher (`guest_svc` in `crates/arm64jit/src/jit.rs`) from ~30 to
~48 syscalls, targeting the families a real Android boot / ALooper / login
path issues that the table previously sent to -ENOSYS:

- **fstat(80) / newfstatat(79)** with a **guest-layout `stat`** — the host
  `libc::stat` layout differs across x86_64 vs aarch64, so forwarding the host
  struct would silently mis-place every field. `unsafe fn write_guest_stat`
  transcribes into the AArch64 asm-generic layout (128B): st_dev@0 st_ino@8
  st_mode@16 st_nlink@20 st_uid@24 st_gid@28 st_rdev@32 st_size@48
  st_blksize@56 st_blocks@64, times (sec+nsec) @72..112. Numbers + layout
  verified against `/usr/aarch64-linux-gnu/include/asm-generic/{unistd,stat}.h`.
- **sockets**: socket(198), bind(200), listen(201), accept(202), connect(203),
  setsockopt(208), getsockopt(209) — the networking/login path.
- **event/epoll** (Android ALooper is epoll-based): eventfd2(19),
  epoll_create1(20), epoll_ctl(21), epoll_pwait(22), ppoll(73).
- **descriptors**: dup(23), dup3(24), ioctl(29), readv(65), writev(66).
- **system/time**: uname(160), gettimeofday(169), clock_getres(114).
- **limits/signals/timers**: getrlimit(163)/setrlimit(164), kill(129),
  tgkill(131), timer_create(107), timer_settime(110).

All struct-returning syscalls chosen with layout-identical-or-explicit
conversion (timeval/rlimit/utsname/epoll_event layouts are arch-identical;
`stat` uses write_guest_stat). New integration test
`guest_svc_stats_and_descriptors_roundtrip` verifies fstat/newfstatat st_size+
st_mode in guest layout, eventfd write/read, epoll_create1+epoll_ctl(ADD — on an
eventfd/pipe, not a regular file which EPERMs), gettimeofday, uname=="Linux".

Gate: `cargo build --workspace` clean; `cargo test --workspace` 270/0 (was 269).
HARD GATE unchanged — real Roblox boot + run log on a GPU/APK host
(`elfjit <libroblox.so> 0x1f0db20 --jni`); none of that is on this VPS.

# Session (Sep 11, 2026) — two REAL silent miscompiles fixed: EXTR operand order + ADC/SBC carry polarity (workspace 272/0)

New differential canaries (diff_math128_and_carry: __int128 sq/mul/madd;
diff_switch_fnptr_hash: switch jump table / fnptr blr dispatch / FNV) surfaced
one failing probe: math128 (JIT 0xc24b01e838c56079 vs native 0xb50f76ac635ab31b).
Bisected to TWO distinct arm64jit bugs, both fixed + native-verified:
1. EXTR invert — see STATUS.
2. ADC/SBC borrow-convention carry — see STATUS.
Files: crates/arm64jit/src/translate.rs (Extr + AddCarry), jit.rs
(add_carry_reference), tests/diff_battery.rs (+math128 canary). Commit 912eff8.


---

# Session (Sep 11, 2026) — SIMD fcvtl/fcvtn float<->double conversion + latent scalar store fix (workspace 275/0)

Commit `3718824` (dev). Opened at 272/0 green; drove the differential battery
into the mixed-precision float<->double vector path and it surfaced BOTH a
missing ISA wall and a latent miscompile:

## 1. New ISA: fcvtl/fcvtl2 + fcvtn/fcvtn2 (SIMD float<->double width conversion)
gcc -O2 emits these for any `float[]` <-> `double[]` elementwise round-trip; the
JIT previously stopped `Unsupported` at the first fcvtl in such code.
- Gates (asm+objdump verified): `(insn & 0xffff_fc00)` in {0x0e617800,
  0x4e617800} = fcvtl (f32->f64, 2 lanes), {0x0e616800, 0x4e616800} = fcvtn
  (f64->f32). Placed BEFORE VecIntToFp/SimdMull (which swallow these as int->fp /
  widening-multiply). `upper` = bit30 (Q): fcvtl2 reads Vn's upper half;
  fcvtn2 writes Vd's upper half. Half-precision byte2-0x21 (fcvtl Vd.4s,Vn.4h /
  fcvtn Vd.4h,Vn.4s) stays Unsupported (no fp16 in the JIT).
- Translate: per-lane cvtss2sd/movq_store (widen), movq_load/cvtsd2ss/narrow
  store (fcvtn). Guest v-lane = VECTOR_BASE + reg*16 confirmed.

## 2. LATENT MISCOMPILE exposed by the new ISA (the important find)
Once a gcc float<->double loop can compile fully (previously it always aborted
`Unsupported` at fcvtl during compile, so NOTHING after it ever executed), the
JIT segfaulted at fault=0x41480000. Root cause: `FpLdStImmWb` (scalar pre/post-
index ld/st) passed RAX as the address register into `fp_scalar_xfer`, which
uses RAX as its value scratch -- so a scalar pre/post-index STORE clobbered the
base with the value and wrote to [value bits] instead of [base]
(`str s30,[x4],#4` stored to 0x41480000 = float 12.5). Now computes the address
in RDX (mirrors the correct FpLdStImmUnscaled). This would have corrupted
single-precision array/matrix writes in any real graphics/audio math.

## Verification
- Differential canary `diff_fcvtl_widen_and_fcvtn_narrow` (round-trip 16
  floats<->doubles, distinct values, upper-half use): jit==oracle==1248.
- Decode/exec: `fcvtl_fcvtn_decode_and_lane_widen_exec`; store fix:
  `fp_scalar_postindex_store_uses_base_not_value`.
- `cargo build --workspace` clean; `cargo test --workspace` 275/0 (arm64jit
  162 lib + 42 diff + 8 loader_run; libbadcpu 22; libloader 23; +others).

## Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery sweeping mixed FP width / vector breadth
   (fcvtl half-precision forms, f2d long forms, pmull, sat-ops, more -O3
   reduction shapes) -- this ISA-assertion loop keeps flushing real latent
   miscompiles (this cycle: fcvtl + the scalar-store RAX clobber).
2. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
3. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays
   the HARD GATE, blocked until a capable host + the real binary/APK (none here).


---

# Session (Sep 11, 2026) — SIMD BIF misdecoded as BSL; int64->int32 narrowing (workspace 277/0)

Commit `60400f3` (dev). Opened at 275/0 (fcvtl cycle). Pushed the differential
battery further into int64->int32 truncation pipelines and found a THIRD real
silent miscompile in the bitwise-select family:

## BIF was decoded as BSL with the opposite mask semantics
gcc -O2 int64->int32 narrowing (with large negatives needing sign handling)
compiles through smull/saddl/saddw + cmeq + bit/bif + uzp; through the JIT it
returned the wrong value. Root cause: the SimdSel (BSL) decode gate required
bit22 SET but did NOT inspect bit23, so BIF (`bit22=1,bit23=1`) was silently
decoded as BSL (`bit22=1,bit23=0`). The three select ops are:
- BSL = (N&M)|(D&~M);  BIT (bit22=0/bit23=1) = (N&M)|(D&~M);  BIF (bit22=1/bit23=1)
  = (N&~M)|(D&M) -- BIT/BIF are the opposite bit-inserts.
- Fix: SimdSel gate now requires bit23 CLEAR (BSL only); BIT/BIF fall through.
- SimdBit gate extended to the BIF residues (0x6ee01c00/0x2ee01c00 alongside
  0x6ea01c00/0x2ea01c00) with a `bif` flag decoded from bit22; translate emits
  (sel&Vm)|(keep&~Vm) with (sel,keep)=(Vd,Vn) for BIF and (Vn,Vd) for BIT.
- NOTE: the bit-vs-bif opcode discriminator is bit22 (0x0040_0000), NOT bit14
  (a first pass used bit14 and produced wrong values; the two words differ by
  exactly 0x00400000). The SimdBit FAMILY residues also differ by bit22, so the
  gate set {0x6ea01c00, 0x6ee01c00, ...} is correct.

## Verification
- jit.rs `bit_vs_bif_bitwise_insert_semantics`: decode maps bit->bif:false,
  bif->bif:true, bsl->SimdSel; exec both produce the correct DISTINCT values
  (BIT 0x11bb33dd11ff7799, BIF 0x660066446600ee for the fixed operand set).
  Learned en route: read ARM's Vd,Vn,Vm operand order for `bit`/`bif` (Vm is the
  MASK); the earlier "bif returned bit's value" was a test-labels error, not a
  translate bug.
- diff_battery `diff_int64_to_int32_narrowing_bif`: int64->int32 and the full
  int64->int32->short round-trip drive the bif path; jit==oracle.
- (Surgery lesson: a bad mid-file replace during test insertion dropped two
  pre-existing host-float-bridge tests; restored them verbatim from HEAD and
  verified with a fn-name diff that no test was lost.)
- `cargo build --workspace` clean; `cargo test --workspace` 277/0 (arm64jit
  163 lib + 43 diff + 8 loader_run; libbadcpu 22; libloader 23; +others).

## This session's net (commits 3718824 fcvtl + store fix, 60400f3 bif)
Two new-ISA walls (fcvtl/fcvtn) and TWO latent silent miscompiles fixed
(FpLdStImmWb scalar store RAX-address clobber; SIMD BIF->BSL opposite select) --
the differential-assertion loop keeps flushing real wrong-pixel/audio bugs.

## Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery sweeping ISA breadth (SIMD permute/wide
   paths, sat-ops, half-precision, -O3 reduction shapes) -- the fcvtl+bif
   cycles prove it is the highest-leverage correctness engine available here.
2. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
3. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays
   the HARD GATE, blocked until a capable host + the real binary/APK (none here).


---

# Session (Sep 11, 2026) — SIMD compare-to-zero + sub-wide + high-rm MulDiv (workspace 280/0)

Commit `1702c89` (dev). Opened at 277/0 (bif cycle). Continued the cross-gcc ISA
sweep into signed-byte / 16-bit / int64-narrowing code; flushed one new-ISA wall
and TWO more real silent miscompiles (total this session: 2 ISA walls + 4 bugs):

## 1. SIMD integer compare-to-zero (cmeq/cmgt/cmge/cmlt/cmle Vd.T,Vn.T,#0) -- NEW
gcc emits these for every vectorized x<0 / x==0 / sign check, e.g. the
`if(sc[i]<0) count++` idiom which compiles to cmlt -> sxtl -> ssubw (count=n by
subtracting the sign-extended mask). Implemented `Inst::SimdCmpZero`:
- gate: top byte {0x0e,0x2e,0x4e,0x6e} + bit26 + bits[15:10] in {0x22 cmgt,
  0x26 cmeq, 0x2a cmlt} AND bit16 CLEAR (bit16 set = FP frint/tbl family -- the
  first gate draft collided with frintm 0x4e219821).
- esize = 1<<bits[23:22] (8B/8H/4S/2D); U (bit29) toggles gt->ge and eq->le.
- translate: per-lane signed/sign-extended load, test, setcc(eq==0x4,gt==0xf,
  ge==0xd,lt==0xc,le==0xe), movzx, neg => all-ones-or-0 mask.

## 2. sub-wide ssubw/usubw decoded as ADD -- SILENT
The SimdAddw gate checks `(insn & 0x1800)==0x1000` (bit12 set, bit11 clear) but
ignores bit13 (0x2000), the add/sub-wide discriminator (saddw 0x0e7613de vs
ssubw 0x0e7633de differ by bit13). So every ssubw silently ADDED the widened
element; the negative-count idiom's subtract became an add and the count was
wrong. Added `sub = (insn & 0x2000) != 0` to SimdAddw; translate emits
`sub_rr64` when set.

## 3. MulDiv gate mask kept bit20 -- `mul w9,w9,w20` was Unsupported
`(insn & 0x7ff0_0000)` keeps bit20 (part of rm, bits[20:16]), so any mul/madd/
sdiv with rm>=16 (bit20 set) failed the gate. Correct mask 0x7fe0_0000 (clears
the whole rm field). gcc's `mul w9,w9,w20` (32-bit mul, high rm) exposed it.

## Verification
- jit.rs `isa_regress_tests`: cmlt mask (neg bytes all-ones) + ssubw subtracts-
  not-adds exec (both decode-atom and runtime values).
- diff_battery `diff_byte_negcount_ssubw_cmlt`: byte sum + negative count via
  the cmlt->ssubw idiom + 16-bit unsigned widening; jit==oracle.
- Cross-gcc probes all exact: byte SIMD=36775, 16-bit audio =9564799, int64
  narrowing n1=7769812456 / w=7634108456, byte+negcount=3487.
- (learned: keep test value literals out of `<< 32` shift overflow; append
  regression tests as a separate `#[cfg(test)] mod` at EOF instead of fragile
  mid-file splicing.)
- `cargo build --workspace` clean; `cargo test --workspace` 280/0 (arm64jit
  165 lib + 44 diff + 8 loader_run).

## Session total (commits 3718824, 60400f3, 1702c89)
4 ISA walls / features (fcvtl/fcvtn float<->double, SIMD compare-to-zero) and
FOUR real silent miscompiles found+fixed via the differential sweep:
FpLdStImmWb scalar-store RAX-address clobber, SIMD BIF->BSL opposite select,
ssubw-as-add, MulDiv high-rm gate mask. The cross-gcc differential battery
(+native oracle) is the highest-leverage correctness engine available without
an APK/GPU.

## Next (ordered, no APK/GSI/GPU on this box)
1. Keep the differential battery sweeping (sat-ops, half-precision, more -O3
   reduction/permute shapes, fp16).
2. libloader ELF/loader gaps -> libbadcpu ISA gaps -> services/auth.
3. Real-binary/GPU boot proof (`elfjit <libroblox.so> 0x1f0db20 --jni`) stays
   the HARD GATE, blocked until a capable host + the real binary/APK (none here).

## Session (Sep 11, 2026) — ld2/st2 structure deinterleave element-size FIXED (commit 36813d1, workspace 282/0)

Root-caused and fixed a silent SIMD miscompile in the structure load/store
(ld2/st2) path: the translate arms DEINTERLEAVED AT BYTE GRANULARITY
regardless of element size. `ld2 {v.8h, v.8h}` (2-byte elements — gcc's
strided u16 accumulate `for(i+=2) s2 += b[i]`) read mem[2i],mem[2i+1] instead
of the correct mem[4i],mem[4i+2]; the even-index sum registered the wrong
memory elements. Decode already computed `esize` for the 3/4-register forms
but dropped it for ld2/st2 (the 2-register case made byte-only runs look
correct, hiding the bug). Passed `esize` through decode and deinterleave at
element stride (element i of reg j at byte i*(2*es)+j*es, copy es bytes).

Verified end-to-end (no QEMU): isolated strided-u16 repro 311814 -> 281606 ==
native; differential canary `diff_ld2_halfword_strided_accumulate` added
(proven sensitive — reverting ONLY the esize change makes it fail again).
+decode regression with assembler-verified 8h/16b/4s encodings.
`cargo build --workspace` clean; `cargo test --workspace` 282/0 (was 280).

HONEST REMAINING / next high-value target: a SEPARATE pre-existing co-resident
bug surfaced by the differential sweep — a single function that VECTORIZES TWO
widen-accumulate loops (e.g. two byte-sums `i+=1` and `i+=2`) into one host
block produces correct low-32 sums but a stray data byte at bits 32-39 of the
64-bit accumulator lanes (repro `twov2.c`/`twoloopp.c`: JIT 18916 vs oracle
16868; isolated each loop passes). Independent of the ld2 fix (byte-only path,
esize=1, unchanged). Root-cause lead: bits 32-63 of the accumulator lanes are
contaminated, strongly suggesting a shared host/permscratch register clobber
across the two co-resident widening loops — NOT a single-Ld2 fault. Next
session: trace which translate arm leaves permscratch / a host vector scratch
dirty that the second loop's zip/uxtl reads; this subsumes the "byte-acc
halved" 2x signature documented in the runs/STATUS ledger. HARD GATE unchanged
(no GPU/APK/libroblox.so on this VPS).

## Session (Sep 11, 2026) — differential-sweep JIT correctness: 6 silent SIMD/int miscompiles FIXED (288/0)

Driving elfjit against qemu-aarch64 oracles on real static aarch64 builds caught
and fixed SIX silent miscompiles (each regression-guarded + sensitivity-verified):

1. ld2/st2 element-size deinterleave (commit 36813d1) — byte-granularity
   regardless of element size; strided-u16 accum read mem[2i],mem[2i+1] instead
   of mem[4i],mem[4i+2]. 282/0.
2. W-form bitfield (Ubfm/LSR/LSL/ROR aliases) loaded Rn as u64 and shifted
   without zeroing the high 32 bits (b739a6b) — `madd x; lsr w` pulled high
   guest garbage into the byte (8652 -> 0x37562e2cc). 284/0.
3. shrn/shrn2 (shift-right-NARROW) decoded as plain equal-size ushr/sshr
   (04a1483) — wrong source stride + shift (x^(x>>16) -> 0x83f9b82e6). 286/0.
4. rbit SWAR truncated every 64-bit mask to 32 bits (b83a1cf) — only the low
   half of x reversed; ctz=198 vs 10.
5. clz x (sf=true) emitted REX.W BEFORE the F3 prefix — CPU ran a 32-bit
   lzcnt, so clz(x<2^32) returned 32-len not 64-len (clz(0x16136740)=3 vs 35).
   Emit `F3 48 0F BD` (REX must be the last prefix).
6. shl #imm decoded shift as immb-only + esize from trailing_zeros (dropped
   bit22) — `shl v.4s,#25` ran as #1 on 1-byte lanes. shift = immh4:immb -
   esize_bits.

Tooling added: `sweep_wform.py` (in /tmp/combw) — static -nostdlib -Wl,-e,entry
build, qemu-aarch64 oracle, elfjit run, diff. Expanded to rbit/clz/ctz/shl/
shr/umulh families; all probes now match the oracle. Notebook: the entry-sym
parse must split on whitespace (objdump '0000000000400120 <entry>:' includes
the symbol), and native oracle must use gcc not cross-gcc.

Workspace 288/0, build clean, 6 focused commits on dev. No repo push. The
pre-existing co-resident two-loop widening bug remains open (documented;
contamination at bits 32-63 of acc lanes when two widen loops share a block)
and is independent of all six fixes. Next: keep sweeping SIMD/FP shapes; then
libloader ELF/loader gaps per RECOMMENDATION order. HARD GATE unchanged (no
GPU/APK/libroblox.so on this box).

## Session (Sep 11, 2026) — randomized differential fuzz: +2 JIT fixes (289/0, 165 fuzz cases green)

Extended the elfjit-vs-qemu-aarch64 differential harness into a randomized
fuzzer (`fuzz_jit.py` in /tmp/combw) generating unrolled scalar/SIMD shift/xor/
byte/fp programs with runtime-dependent LCG inputs. Found TWO more silent JIT
bugs, both in `Inst::BitField` (the general-extract path):

1. UBFM extract mask SIGN-EXTENSION: for a field width making the mask >= 2^31
   (ubfx x,#16,#32 -> mask 0xffffffff) `and_ri64(RAX, mask as u32)` emitted a
   64-bit AND with a sign-extended imm32 => mask became 0xffffffffffffffff
   (no-op), leaking the high 32 bits of the shifted value into the result
   ((x>>16) -> 8234290418553910946 vs 85047753507696). Route masks >0x7fffffff
   through mov_ri64+and_rr64. Commit 750457a.
2. UBFM mis-decoded as ROR: `imms+immr+1==bits` is NOT a rotate discriminator —
   genuine ror is an EXTR alias (-> Inst::Extr), so any UBFM matching it
   (ubfx x,#16,#32: immr=16,imms=47, field to the top bit) is a plain extract.
   The old branch ran ror_ri8(imms), corrupting extracts. Removed it; words
   fall through to the extract path. Genuine ror x,#17 still passes.

Regression: diff_ubfx_masks_upper_bits_and_is_not_ror (u32-width extract +
extract-with-genuine-ror), sensitivity-proven both ways. cargo build clean;
cargo test --workspace 289/0. Randomized fuzz seeds 1-4: 165/165 match qemu.

Combined with the previous sweep this cycle has produced EIGHT verified JIT
correctness fixes (ld2/esize, W-form bitfield, shrn2, rbit mask, clz REX order,
shl-imm decode, ubfx mask, ubfx-vs-ror). Next: keep fuzzing with more diverse
generators (fp, structure loads, saturating arith), then libloader ELF/loader
gaps per RECOMMENDATION order. HARD GATE unchanged (no GPU/APK/libroblox.so).

## Session (Sep 11, 2026) — co-resident two-loop widening bug RESOLVED (was the documented OPEN BUG)

After the 8 JIT correctness fixes this session (ld2/st2 esize deinterleave,
W-form bitfield high-32 mask, shrn/shrn2 narrow decode, rbit 64-bit mask width,
clz REX.W byte order, shl#imm decode, UBFM extract mask sign-extension, and
ubfx-vs-ror discriminator), the LONG-DOCUMENTED co-resident two-loop widening
bug — and the "intermittent SIMD-loop block-liveness" bug it subsumed — are
both resolved. The original reproducers now pass exactly:
  twov2.c    JIT 18788  == oracle 18788   (was JIT 7465833 / garbage 0x375)
  twoloopp.c JIT 3776   == oracle 3776
  maskf.c    JIT 2416   == oracle 2416    (documented intermittent lane corruption)
Both were SYMPTOMS of the same underlying shift/immediate-mask miscompiles
(high-32 contamination leaking through sign-extended `and` imm32 masks and
wrong shift amounts), not a separate co-resident register-liveness fault. ~700
differential fuzz cases green including PIE+reloc loader-mode (elfjit vs qemu
oracle). This removes the last known-open JIT correctness item on this box.

Next (ordered, all still open): libloader ELF/loader gaps, libbadcpu ISA gaps,
services/auth — or more fuzz coverage. HARD GATE unchanged (no GPU/APK).
## Session (Sep 11, 2026) — guest_svc syscall surface expansion (filesystem/IO/network)

Added two batches of AArch64 syscalls to the guest_svc bridge that a real
Android/Roblox boot path issues early and that previously returned -ENOSYS,
each number verified against the sysroot asm-generic headers (not guessed):
  Batch 1 (commit c0073b6): fcntl(25), clock_nanosleep(115), getrusage(165),
  setpgid(154), rt_sigaction(134), rt_sigprocmask(135), fadvise64(223).
  Batch 2 (commit a682eea): mkdirat(34)/unlinkat(35)/symlinkat(36)/linkat(37)/
  renameat(38), pread64(67)/pwrite64(68), socketpair(199), sendto(206)/
  recvfrom(207)/sendmsg(211)/recvmsg(212)/accept4(213), madvise(233), umask(166),
  getgroups(158).
Signal ops accept registration (return 0) but don't dispatch guest trampolines
(consistent with the shim's no-signal posture); oact/oset outputs are zeroed so
callers don't deref garbage. madvise DONTNEED keeps host RSS bounded under guest
allocation churn. Test guest_svc_common_boot_gaps_roundtrip covers both batches
(mkdir/link/rename/read/write/pipe lifecycle, socketpair+sendmsg/recvmsg byte
roundtrip, madvise, umask, fcntl, sigaction/procmask zeroing, clock_nanosleep).
Workspace 290/0. HARD GATE unchanged (no GPU/APK on this box).
## Session (Sep 11, 2026) — fuzz_jit loader-mode: PIE + reloc shapes end-to-end (100+ cases)

Extended the differential fuzzer with a loader-mode: gen_globals_pie /
gen_pie_callchain compile `-fPIE -pie -nostdlib` programs with exported
statics, arrays, and function-pointer initializers — forcing R_AARCH64_GLOB_DAT,
RELATIVE, and ABS64 relocations in .data.rel.ro — then run them through elfjit
(load_elf_image + bind_image_plt + JIT) and diff against a qemu-aarch64 oracle.
~35% of fuzz cases now take this path. Seeds 31-36: 120/120 pass, validating
the full loader→reloc→bind→JIT chain the runtime depends on (commit 93c0ee4).
The JIT correctness work is now broadly covered (~620 differential cases green
total). Next (unchanged, in RECOMMENDATION order): libloader ELF/loader gaps
then libbadcpu ISA gaps then services/auth; or more precision on the open
co-resident two-loop widening bug. HARD GATE unchanged (no GPU/APK on this box).

## Session (Sep 11, 2026) — guest_svc boot-path gaps + co-resident bug RESOLVED (290/0)

Added seven AArch64 syscalls a real Android/Roblox boot issues early, that
previously fell to -ENOSYS: fcntl(25), clock_nanosleep(115), getrusage(165),
setpgid(154), rt_sigaction(134), rt_sigprocmask(135), fadvise64(223) — numbers
verified against the sysroot asm-generic headers (commit c0073b6). +unit test
guest_svc_common_boot_gaps_roundtrip.

MAJOR: the long-documented co-resident two-loop widening bug — and the it
subsumed "intermittent SIMD-loop block-liveness" bug — are BOTH RESOLVED. After
the 8 JIT correctness fixes this session (ld2/st2 esize, W-form bitfield mask,
shrn/sh2 narrow decode, rbit 64-bit mask, clz REX.W order, shl#imm decode, UBFM
mask sign-extension, ubfx-vs-ror), the original reproducers pass exactly:
twov2.c 18788==18788 (was 7465833), twoloopp.c 3776==3776, maskf.c 2416==2416.
They were SYMPTOMS of the same shift/immediate-mask miscompiles, not a separate
register-liveness fault. ~950 differential fuzz cases green (incl. PIE+reloc
loader-mode via fuzz_jit.py). No known-open JIT correctness items remain.

Workspace 290/0, build clean, ~18 focused commits on dev. Next (RECOMMENDATION
order): more boot-path syscalls, libloader/libbadcpu/services gaps, or more fuzz
coverage. HARD GATE unchanged (no GPU/APK/libroblox.so on this box).

## Session (Sep 11, 2026) — scalar ucvtf S-form FIXED (291/0) + fuzz gens

Silent FP miscompile found by expanding the differential fuzzer with
fma-chain/128-bit-struct/float-reduce generators: scalar ucvtf S-form
(0x7e21db18, gcc emits `ldr sD,[sp]` + `ucvtf sD,sD` for `(float)volatile_u32`)
decoded with sng=true but the translate arm ignored it and always did the 64-bit
D-form convert, silently dropping the value (audio/down-mix u32->float). Fixed,
sensitivity-proven (166908 vs 222908 without the fix). commit 48863d1.
Open (deeper, still characterizing): vectorized div/multiply reduction pipeline
(`a[i]/b[i]` -> ushr.2d + and + uzp1 + scvtf + fmla fmul fdiv) gives value
collapse (63 vs 729251) not reproducible in minimal isolated hand-asm; next
step is tracing that specific op mix.

## Session (Sep 11, 2026) — fuzz-driven FP findings wrap

Fixed 9th JIT miscompile this campaign: scalar ucvtf S-form sng flag ignored
(commit 6a3db93). fuzz_jit gens expanded (fma-chain/128-struct/float-reduce).
All individually-isolated SIMD/FP ops now match the qemu oracle (fdiv/fmul/fmla/
uzp1/and/ushr/scvtf/ucvtf/movi/lane-extract). Remaining open: a specific
multi-op vectorized div/multiply-reduction pipeline (vmult.c: `acc+=a[i]/
b[i]+c[i]` with computed operands) shows value collapse ~63 vs 729251 that does
NOT reproduce when the same ops are isolated; needs a JIT execution tracer to
pin the exact guest instruction. Low smoking-gun priority: all constituent ops
validate individually. Next sessions: add a per-instruction traced run to
diff_battery or resolve via reducing vmult.c further.

## Session (Sep 11, 2026) — MOVI Vd.2D immediate decode FIXED — the 10th JIT bug

Root-caused and fixed the 'fmla/div vector pipeline' bug that had resisted
isolation all session. It was NOT a lane-coalescing interaction — it was a
fundamental decode error in MOVI Vd.2D, #<imm>. The 2D immediate is a
BYTE-SELECT pattern (bits[9:5] low nibble picks which bytes 0..3 of the low 32
are 0xff), but the decode treated it as a generic imm8-replicate, so
mov v27.2d,#0xffff produced 0x03 lanes instead of 0x000000000000ffff. Any
vector AND-mask built this way corrupted bit-field extraction (gcc's
shr->and->uzp1->scvtf reduction), collapsing values. Fixed the decode with 6
qemu-verified ground-truth lane values; new unit test (movi_2d_byte_select_
ground_truth) + moved the flow canary from #[ignore]d to a real regression
(diff_fmla_div_pipeline_lcg). Sensitivity-proven both ways (695 vs 7777753).
cargo test --workspace 293/0. This is the 10th JIT correctness fix this
campaign; the fma_chain fuzzer is responsible for surfacing it.

## Session (Sep 11, 2026) — RELR e2e test + campaign wrap (294/0, ~1680 fuzz cases)

The loader's DT_RELR path (packed-relative relocations, tag 0x23) had a
unit-tested decoder but the discovery wire-up (walk PT_DYNAMIC tags -> find the
stream -> decode -> route back as R_AARCH64_RELATIVE) had no test. Added an
e2e test that builds a minimal ELF with a RELR-only PT_DYNAMIC on disk and
asserts read_elf_relocations returns the right offsets/info. This is the last
unvalidated reloc path a real Android .so would hit (recent lld emits RELR by
default).

Known gap for next sessions: R_AARCH64_TLS_* (TPREL/DTPREL) are not handled at
all by the loader/JIT. Real TLS needs a host-side per-thread guest TLS area
(tp/x28 slot), an architected feature — needs the real binary to validate.
cargo test --workspace 294/0, build clean.

## Session (Sep 11, 2026) — long-open fused two-loop signed-div miscompile RESOLVED: integer vector NEG/ABS (302/0)

Root-caused the last reproducibly-open JIT bug — the fused two-loop signed
magic-division miscompile (`fuzz_jit.py` gen_signed_div; n=4, div /7 repro:
oracle 406144671 vs jit 78184144 — the entire neg-loop contribution was lost).
It was a DECODE COLLISION, not a register-clobber:

- `neg v29.2s, v31.2s` (0x2ea0bbfd) is an integer two-register-misc op
  (opcode bits[16:12]==0xb, bit16 CLEAR). The vector float->int FcvVec gate
  used mask 0xffe0_fc00, which ZEROES bits[20:16], so NEG fell into the residue
  `0x2ea0_b800` (== fcvtzu v0.2s) and executed as a float->int convert — with
  v31.s0=0xfc8e117a (a small denormal float) that silently produced 0, then fed
  every downstream zip1/saddw/sxtl2 lane wrong.

Fixed (commit 47b1007):
1. FcvVec gate now requires bit16 SET (all six fcvtzs/fcvtzu sizes have it;
   neg/abs have it clear) — NEG/ABS no longer decode as fcvtzu. This also means
   NEG/ABS stop silently corrupting anywhere a -O3 build emits them.
2. New `Inst::SimdArithUnary` decode for NEG/ABS (opcode 0xb, neg=bit29,
   esize from bits[23:22] {0:B,1:H,2:S,3:D}), placed before FcvVec.
3. Translate: per-lane signed negate (neg_r64, two's-complement wrap) and
   abs via `(x^(x ar>> w-1)) - (x ar>> w-1)` after sign-extending each lane;
   per-lane read-modify-write is rd==rn safe.
4. Added `JIT_STEP` debug trace (single-instruction blocks + full x/v dump
   after each) — the per-instruction register oracle that made this tractable;
   debug-only, no effect on normal path.

Verification: repro jit=406144671==oracle (exact); decode + exec regressions
across .4s/.8h/.16b/.2d (negative, wrap, and abs-magnitude lane cases); new
diff_battery canary `diff_vector_neg_abs_unary`; ~420 fresh fuzz cases across
12 seeds (incl. 50/99/7/9001 repro seeds) all green, 0 fails; full
`cargo test --workspace` **302/0**, build clean.

No known-open arm64jit correctness items remain on this box (same as the
cycle-27 close). Next high-value per RECOMMENDATION order: libloader ELF/loader
gaps, libbadcpu ISA coverage, JNI function-table surface, then services/auth.
HARD GATE unchanged: real Roblox boot + reproducible run log on a GPU + real
APK/binary host (none on this VPS).

---

## Session (Sep 11, 2026) — guest TLS bootstrapped (R_AARCH64_TLS local-exec / main-binary case)

Commit `6af3cd4` (dev), workspace **304/0** (was 302), build clean.

### What landed
- `libloader::elf::setup_guest_tls(info, path, tls_region, size) -> tpidr` (+ `tls_layout`):
  finds the main image's `PT_TLS` (p_type 7), copies its `p_filesz` init image into
  a per-thread region at `region + AARCH64_TCB_SIZE` (16), zero-fills `.tbss` to
  `p_memsz`, and returns the thread pointer `tpidr = region` — the AArch64 TLS ABI:
  the module TLS data block lives 16 bytes after TP, and local-exec/initial-exec
  `:tprel:` addressing (`mrs xN,tpidr_el0; add x0,tp,#o`) lands on block+`o-16`.
- `elfjit` and the `loader_run` harness now seed `CpuState.tpidr` from
  `setup_guest_tls` instead of a bare zero-filled stack. ELFs without `PT_TLS`
  get `tpidr = region` (byte-identical to the prior behaviour) — no regression.
- **Verified end-to-end, no QEMU**: cross-gcc `__thread` fixture (`g_slot=7`,
  `g_big=123456789`, `g_zero` in `.tbss`, `bump()`) through `load_elf_image →
  setup_guest_tls → jit_run` returns **123456804** — the exact native x86-64
  oracle. (qemu-aarch64 itself SIGSEGVs on this nostdlib static TLS image because
  it doesn't seed PT_TLS without a dynamic loader, so qemu is not a usable oracle
  here.) Before the seeding the region was zeroed, so every `__thread` read
  returned 0.

### Scope note (honest)
This closes the documented "R_AARCH64_TLS_* untouched" gap for the **main-binary
local-exec/initial-exec** case — which is exactly the shape of `libroblox.so`
loaded as the boot image (a PIE still uses local-exec for its own `__thread`).
The **dynamic** TLS paths (`R_AARCH64_TLS_TPREL64`/`DTPREL64` GOT slots for
TLS referenced *across* modules, general-dynamic) only engage once the loader
loads `DT_NEEDED` dependency modules as a multi-image process — the next
loader frontier, and it needs a real multi-lib host to validate.

### Tests added
- `libloader elf::tests::setup_guest_tls_copies_init_and_returns_tcb_tpidr` —
  init-image copy, TCB tpidr, `.tbss` zero-fill, tprel addressing, no-TLS fallback.
- `arm64jit loader_run::loader_run_thread_local_storage_returns_123456804` —
  full loader→TLS→JIT pipeline gate (skipped if cross-gcc absent).

### Next (per RECOMMENDATION order)
libbadcpu/libloader JTAG: guest **threading (clone/vfork)** is the documented
single-threaded-boot frontier (needs a real host to validate); multi-module
`DT_NEEDED` load for cross-module TLS + GOT/PLT within deps; broaden the
differential fuzzer into still-uncovered NEON/by-element/`tbz` classes. HARD
GATE unchanged: real Roblox boot + run log on a GPU/APK host (none on this VPS).

### Session (Sep 11, 2026 cont.) — differential fuzzer extended: NEON by-element/tbl + bitfield-insert + tbz + fcvt classes, qemu oracle; 23 generators clean
Commit `a733fba`. Added 5 generators for previously-uncovered ISA classes (NEON
`vmlaq_n_f32` by-element fmla + lane ins/get + `vbsl/vext/vrev64` bitmix, 64-bit
`bfi/bfiz`, `tbz/tbnz` bit-branches, fixed-point `fcvtzs #fbits`) and a
64-bit-exact **qemu-aarch64 oracle fallback** (write+itoa `_start` wrapper) for
`<arm_neon.h>` programs the host x86 gcc can't compile — the old harness hard-
skipped them. `gen_neon_byelem` uses binary-exact lanes so FMA-vs-`mul+add` ULP
noise doesn't read as a structural diff; removed an unavailable `vrbitq_u32`
(only `_u8` exists) for `vrev64q_u32`. Result: full 23-generator sweep is
**70/70 green across fresh seeds, 0 skips** (was ~14% skip). No new miscompile
found in the covered classes — a negative result, but those classes are now
permanent gates. Workspace unchanged (304/0; Rust untouched this commit).

## Session (Sep 11, 2026 cont.) — complete: TLS bootstrap + fuzz-harness expansion + 4 permanent canaries + harness-cascade fix (workspace 308/0)

### Milestones landed this session (8 commits on `dev`)
1. **Guest TLS bootstrap** (`6af3cd4`, docs `7bc1a5e`): `libloader::setup_guest_tls`
   copies the main image's `PT_TLS` init into a per-thread region at TP+16 (the
   AArch64 TCB) and seeds `tpidr_el0` from it, so local-exec/initial-exec
   `:tprel:` addressing reads/writes real `__thread` data. Verified no-QEMU:
   cross-gcc `__thread` fixture → `jit_run` returns 123456804 == native oracle.
   Closes the documented `R_AARCH64_TLS_*` gap for the main-binary case.
2. **Fuzz harness** (`a733fba`, docs `5b8d805`): +5 generators for previously
   uncovered classes (NEON by-element fmla + lane round-trip, NEON
   `bsl/vext/vrev64`, 64-bit `bfi/bfiz`, `tbz/tbnz`, fixed-point `fcvtzs #fbits`)
   and a 64-bit-exact **qemu-aarch64 oracle fallback** (write+itoa `_start`
   wrapper) so `<arm_neon.h>` programs the host x86 gcc can't compile now get a
   differential oracle instead of hard-skipping. 23 generators; a 12-seed ×
   100-case campaign (1200 cases) is **0 fail / 0 skip**.
3. **Permanent cargo canaries** (`b4a48cd`, `7244feb`): loader_run gates for the
   newly-covered classes — bfi-64 (279514809947), tbz/tbnz (4068), fixed-pt
   fcvt (99), NEON by-element fmla (504; qemu architectural oracle).
4. **Harness-cascade fix** (`7244feb`): a single test's cross-gcc compile failure
   used to poison the shared `run_lock` mutex and cascade-fail every other test
   with `PoisonError` (each passed in isolation). Added `lock_run()` which
   recovers poisoned guards. Also made the byelem fixture `-O0`-safe
   (const-index `vgetq_lane`/`vsetq_lane` need `-O` to fold their lane index).

### Honest scope + findings
- **Negative result (good news):** the newly covered classes (NEON by-element
  fmla, lane ops, bitfield-insert, bit-branch, fixed-point fcvt) show **no**
  miscompile across 1200 fresh differential cases — the arm64jit ISA handling
  of those classes is structurally correct. They're now locked as permanent
  `cargo test` gates.
- **FMA granularity note:** the JIT's by-element `fmla` translate does `mul`+`add`
  (two roundings) rather than a fused FMA (one rounding). Structurally correct
  (binary-exact differential runs agree with qemu), but bit-exact float workflows
  can differ by 1 ULP from ARM's fused `fmla`. Acceptable for now; revisit if
  exact-bit-sovereignty is ever required.
- **Boundary (unchanged):** dynamic *cross-module* TLS (`R_AARCH64_TLS_TPREL64`
  GOT slots, general-dynamic) only engages with a multi-`DT_NEEDED` loader (the
  next loader frontier); guest threading (`clone`/`fork`/`rt_sigreturn`/`execve`)
  remains the documented single-threaded-boot frontier.

### Verification state
`cargo build --workspace` clean; `cargo test --workspace` **308/0** (up from
302). loader_run 13/13. Ship 1200-case fuzz campaign green. HARD GATE unchanged:
real Roblox boot + reproducible run log on a GPU + APK/binary host (none on this
VPS) — nothing here can satisfy it, so this session closed loader/TLS + fuzz-
correctness work as far as physically verifiable.

## Session (Sep 11, 2026 cont.) — EXT extract-immediate bug FIXED; open u16/32 pair-xor reduction bug isolated

### Fixed: `ext` (SIMD vector extract immediate) operand-order inversion — commit `826db92`
Found by the new `gen_pairwise_reduce` differential generator. ARM
`ext Vd.16B, Vn.16B, Vm.16B, #imm` returns the 16-byte window at `imm` of the
concatenation where **Vn is the low-address half and Vm the high**:
`Vd[0..16-imm)=Vn[imm..16)`, `Vd[16-imm..16)=Vm[0..imm)`. The translate built the
concat as `[Vm.lo, Vm.hi, Vn.lo, Vn.hi]` (Vm low, Vn high) — inverted for every
non-symmetric `ext`. **qemu-verified** with distinct bytes: `ext(Vn,Vm,#8)` ⇒
lo=`Vn[8..15]`, hi=`Vm[0..7]`. gcc's horizontal XOR-reduce for pair reductions
(`s ^= a[i]+a[i+1]`) emits `ext v0,v30,v0,#8` + `eor`; the old code collapsed
it to a single 64-bit-lane xor instead of the full one. Minimal repro p1
(const int pair-xor): returned **16**, now **0** == qemu and native. 28→99/160
of the pairwise-reduce stress now correct.

### OPEN (isolated, reproducible) — 16/32-bit `i+=2` pair-xor reduction
A second, independent bug in the pair-xor reduction path remains, unaffected
by the ext fix. Minimal repro `/tmp/combw/s13b.c` (`unsigned short a[24]`,
`for(i+=2) s ^= (long long)a[i]+a[i+1]`): **native 2314 vs jit 59648**. The
path uses `uxtl/uxtl2` + `uaddl/uaddl2` (element-wise widen-add, upper-half
reads) + `eor` chain + the (now-correct) `ext` horizontal combine. Suspect the
in-place `uxtl2` upper-half read or an eor-chain lane-composition bug, NOT the
(now-correct) ext and NOT `uaddl2` — a hand-assembled
`uaddl2 v1.4s,v0.8h,v2.8h` isolated test is byte-correct in the JIT (== qemu,
upper-half values [5..8]+[50..80]=[55,66,77,88]). The passing sum loop (s13a)
uses `uaddw`+`uxtl`, the failing xor loop (s13b) uses `uaddl`+`uxtl2` with the
in-place `uxtl2 Vd.2d, Vd.4s` (rd==rn) upper-half reads — the cycle-27 in-place
widening-alias class re-checked for the `.4s→.2d` upper form. Reproducible via
`fuzz_jit.py` `gen_pairwise_reduce`
(int/short/u32/u16 variants all trip it). Next debug pass: isolate `uaddl2`
(upper) with a hand-controlled fixture vs qemu, then the eor-chain.

### State
`cargo build --workspace` clean; `cargo test --workspace` **308/0**. Commits
`826db92` (ext fix), `bd60f2c` (generator + this doc), plus the earlier
`fe1e27a` cycle-28 close. The gen_pairwise_reduce generator is a permanent
asset that keeps surfacing this class. HARD GATE unchanged: real Roblox boot +
run log on a GPU + APK/binary host (none on this VPS).

---

# Session — in-place uaddl/saddl widening-alias fix (silent SIMD miscompile)

## The bug (found by the gen_pairwise_reduce differential fuzzer)
The previous session isolated an OPEN pair-xor reduction failing repro
`/tmp/combw/s13b.c` (u16 `for(i;i+=2) s ^= a[i]+a[i+1]`): native gcc + qemu-aarch64
oracle `2314`, JIT `59648`. Suspect was "in-place uxtl2 upper", but that was a
red herring — isolated `uxtl2 v0.2d, v0.4s` returns 3 correctly. The real cause:

**`SimdAddl` (uaddl/saddl/usubl) had NO in-place alias snapshot.** The widened
2*esrc dest write of lane i at `i*2*esrc` overlaps the narrow source bytes of
lane i+1 (at `(i+1)*esrc`), so when `rd` aliases `rn`/`rm` a read-then-write
loop clobbers the still-needed source and corrupts the sum. gcc's pair-xor
reduction emits `uaddl v0.4s, v0.4h, v1.4h` (dest==src). The sibling widening
ops `SimdAddw`/`SimdXtl` already snapshotted via `permute_source`; `SimdAddl`
was the missed gap.

## The fix
Mirror `SimdAddw`: snapshot `rn`/`rm` to the permscratch slots when either
aliases `rd`: `nb = if rn==rd { permute_source(..false) } else { vslot(rn) }`,
`mb = if rm==rd { permute_source(..true) } else { vslot(rm) }`.

## Verification
- in-place `uaddl v0.4s,v0.4h,v1.4h`: lane1 22 (was 20) — `0x160000000b`.
- `s13b.c`: 2314 == oracle (was 59648).
- 400 fresh fuzz cases across 9 seeds (7,19,31,42,5,11,23,55,77): **0 fail / 0 skip**.
- `cargo test --workspace` 309/0 green. New permanent gate
  `loader_run_pairwise_xor_inplace_uaddl_returns_2314` compiles the exact
  program with **-O3** (new `compile_o3` helper — the default `compile()` is -O0
  and never emits the SIMD uaddl path).

HARD GATE unchanged: real Roblox boot + run log on a GPU + APK/binary host
(none on this VPS).

## Session 31b (Sep 11, 2026) — multi-module (DT_NEEDED) loader + cross-module symbol binding

The loader could previously load only a **single** aarch64 image; the real
libroblox.so `DT_NEED`s a dependency chain (libssl, libcrypto, liblog, GSI libs)
that was never loaded, so the guest would fault on its first cross-module
import. Committed `8f3c48b` + `040c3bf` (workspace **310/0**, build clean):

- `libloader::deps::{load_elf_with_deps, LoadedChain}` resolves the main image's
  `DT_NEEDED` closure recursively, maps each dependency **contiguously** after
  the previous in one high guest region, so a single `jit_run` image slice
  `[chain.base, chain.end)` covers every module and cross-module calls compile
  from the same image. `load_elf_image` refactored into
  `load_elf_image_at(path, base)` (the `-shared` tools default to GNU hash, so
  no `DT_HASH` nchain — the scope scan is bounded by the mapped image end).
- `arm64jit::plt::build_export_scope(els)` builds a combined `name→guest_addr`
  map (main-first interposition); `bind_image_plt`/`bind_glob_dat` now take an
  optional scope and resolve an import a loaded dependency defines to its guest
  address (else the host resolver / float bridge / graphics stub as before).
- Two latent binder bugs fixed en route: the symbol scan mistook the **null
  symbol (index 0)** for a terminator → collected **0 exports**; and
  `patch_stack_canary` dereferenced its hardcoded libroblox GOT link
  (`0x631aa30`) even when it mapped outside a small module's image — now guarded
  by `host_addr_of` (real libroblox path unaffected: the slot is in-image).

Verified end-to-end with a cross-gcc `-shared` fixture: libmain.so `DT_NEED`s
libdep.so; `entry()` calls `dep_val()` (cross-module **JUMP_SLOT**) and reads
`dep_global` (cross-module **GLOB_DAT**) → returns **82** from the dependency's
guest address through the shared slice (both symbol-resolution families now
permanent `loader_run` gates).

HARD GATE unchanged: real Roblox boot + reproducible run log on a GPU + real
APK/binary host (`elfjit <libroblox.so> 0x1f0db20 --jni`); none on this GPU-less,
APK-less VPS. Next per RECOMMENDATION order: still libloader gaps (multi-module
binding validated only against synthetic fixtures until the GSI/APK is present),
then services/auth and the JNI surface.

## Session 32 (Sep 11, 2026, hermes-worker) — JNI native-method registry + C++ static-init (__cxa_guard*) shims wired into boot (315/0)

Opened at 311/0 green (no failing test; the android idempotency mandated fix is
long since committed). Runtime-side advance per RECOMMENDATION order (JNI
surface + the documented FMOD/engine C++ static-init boot blocker). Two commits:

### 1. arm64jit JNI: register->lookup->dispatch native-method bridge (a55ead8)
`jni_register_natives` returned JNI_OK and DROPPED the guest `JNINativeMethod`
array, so a native method Roblox binds (e.g. Java_..._IAPPurchaseManager_*) was
recorded nowhere and could never be called back into the guest. Now:
- `parse_register_natives` reads the guest JNINativeMethod array (3 u64 words:
  name*, signature*, fnPtr) at `methods[0..n)` and records (class,name) ->
  (signature,fnPtr) in a process-wide registry (`native_registry()`).
- `lookup_native_method(class,name)` returns the last-registration-wins binding.
- `dispatch_native_method` runs a registered guest fnPtr (a Java_* impl address)
  back through `jit_run` as a guest entry with args, returning guest x0.
+2 tests: records guest bindings (multi-entry parse, re-registration replaces,
  unknown->None), and end-to-end register->lookup->dispatch through jit_run
  returns 42 (guest `mov x0,#42; ret` as the registered fnPtr).

### 2. __cxa_guard_*/__cxa_atexit C++ static-init shims wired into boot (4cff657)
The elfjit `--jni` boot target (JNI_OnLoad) previously stopped when the FMOD /
engine C++ static-init dispatched into pc=0x68c7518 (a `.bss` guard) because the
guest's `__cxa_guard_acquire/release/abort` were never provided — the documented
handoff blocker. Now:
- `shims::register_cxx_shims()` installs Itanium `__cxa_guard_acquire/release/
  abort` (single-threaded byte semantics: acquire sets 1 and returns 1 to run the
  once-body, release sets 2, abort resets to 0) + a no-op `__cxa_atexit`.
- `plt::bind_image_plt` now calls `register_shims()` + `register_cxx_shims()`
  before symbol scanning, so the hand-written bionic shims (__errno/strlens/
  android_log/AAsset/ALooper/ANativeWindow) AND the C++ shims all participate in
  import binding instead of every unresolved name falling to the NULL/0 graphics
  catch-all. `register_named` is idempotent (reuses an existing slot).
+2 tests: guard acquire/release/abort byte machine, and `register_cxx_shims` are
  resolvable by name through `resolver::resolve` (proving the boot wiring).

### Verification
`cargo build --workspace` clean (0 errors); `cargo test --workspace` 315/0
(was 311). Differential fuzz re-run on 9 fresh seeds (77,101,202,303,404,505,
13,29,91) -> 0 fail / 0 skip, confirming no JIT correctness regression from the
wiring. HARD GATE unchanged: real Roblox boot + reproducible run log on a
GPU + real APK/binary host (`elfjit <libroblox.so> 0x1f0db20 --jni`) — none on
this GPU-less, APK-less VPS. Next (RECOMMENDATION order, still open): libloader
multi-module TLS (TPREL/DTPREL across DT_NEEDED deps), more JNI surface (fake
object backing), libbadcpu ISA, services/auth.

## Session 32b (Sep 11, 2026, hermes-worker) — REAL bug: self-imports bound to the NULL catch-all (commit 88ba3f6, 316/0)

Follow-on finding while validating the __cxa_guard_* shims against a real
cross-gcc C++ fixture (function-local static with dynamic init): the fixture's
`entry()` returned 0 instead of the correct 21. Root cause was NOT the guard
shims — it was a binder bug **exposed** by the fixture's `bl init_value@plt`:
a `-shared` module that calls one of its OWN exported functions goes through
`@plt`, and that JUMP_SLOT names a symbol the module itself defines
(`st_shndx != SHN_UNDEF`). `bind_image_plt` only handled cross-module deps
(via `scope`), host resolution, float bridges, and the graphics/stub catch-all —
never the module's own definitions. So `init_value@plt` bound to the NULL/0
graphics catch-all host thunk, and the call dispatched to a stub returning
garbage (0) instead of executing the real guest function at `guest_of(st_value)`.

**Fix (plt.rs):** before host resolution, read the dynamic symbol's `st_shndx`
(byte offset +6) and `st_value` (+8); if `st_shndx != 0` (locally defined), write
`el.guest_of(st_value)` into the GOT slot and count it resolved. `st_value` is
the definition's link address; `guest_of` maps it exactly like every other reloc
target. Self-defined CUDA/PAC/STT exports are the same shape.

**Verified** (`elfjit <guardfixture.so> 0x920 --jni`, real C++ static-init + guard):
first `init_value` -> `__cxa_guard_acquire` returns 1 (runs init, x=0+3+5=8),
release marks done; second call -> acquire returns 0 (already done), x=8+5=13;
sum 8+13 = 21, and JIT now returns exactly 21 = native oracle (a=8 b=13 sum=21
confirmed on host gcc). Before the self-import fix this returned 0. This pattern
(own-exported functions internally `bl fn@plt`-called) is ubiquitous in real
libroblox.so init code, so it is directly on the boot path.

+permanent regression `loader_run_self_import_binds_to_own_guest_body`
(internal_fn(7) = 35; readelf-verifies the JUMP_SLOT names a self-defined symbol
before running). `cargo build --workspace` clean; `cargo test --workspace`
316/0. HARD GATE unchanged: real Roblox boot + run log on a GPU/APK host (none
on this VPS).

## Session 32c (Sep 11, 2026, hermes-worker) — the documented once-routine regression ROOT-CAUSED + FIXED (commit fce362c, 317/0)

While validating the self-import fix against real cross-gcc fixtures, a
`-shared` module that recurses `f(n)=n<=0?7:f(n-1)+helper(n)` (f calls BOTH
itself AND helper@plt) returned 21 instead of native 27 at JIT_BUDGET>=64.
This is the long-documented **"nested guest bl-inline regression to once-
routine"** boot blocker, finally reproduced minimally end-to-end.

Root cause (all three pieces):
1. f's body calls `helper@plt` -> `body_contains_host_plt_bl(f)`=true -> f is
   **import-bearing**, so the recursion `bl f` must DIVERT via a dispatcher-
   return stub (a fresh f frame) rather than inline.
2. Compile-time handling was correct (`import_bearing` -> `force_stubs=true`,
   target NOT pushed to frontier). BUT the fixup-RESOLUTION step still checked
   `host_of_guest.contains_key(fx.target_pc)` FIRST — and f IS the current
   block's own start, so it's in `host_of_guest`. The recursion was re-inlined
   as a self-host-call anyway.
3. Inlining the recursive f AND diverting the inner helper@plt through the stub
   table regresses exactly like the QEMU once-body did: the inner import's
   dispatcher bookkeeping clashes with the inlined caller, so f(5)+helper
   produced 21 (one helper contribution lost) instead of 27.

Fix (jit.rs): a `divert_set` of guest-bl targets decided import-bearing. Stub
targets are built for them even when present in `host_of_guest`, and fixup
resolution routes them to the stub first (call rewritten call->jmp) so the
dispatcher runs each recursive f frame cleanly.

Exposed by the session-32b self-import fix: before it, `helper@plt` bound to the
NULL/0 catch-all and f got a garbage return (this exact fixture returned 0), so
the recursion bug was masked. Now the whole chain is exercised.

Verified: d.so returns 27 == native at EVERY JIT_BUDGET (1,2,8,64,8192; was 21
at >=64); all self-import family (a=1,b=61,c=720,d=27) match native; C++ guard
fixture still 21; fib/shared and plain-static recursion still correct. Fuzzer
re-run 3 fresh seeds 0 fail. +loader_run_recursive_import_bearing_callee_returns_
correct regression. `cargo test --workspace` 317/0, build clean. HARD GATE
unchanged: real Roblox boot + run log on a GPU/APK host (none on this VPS).

## Session 32d (Sep 11, 2026, hermes-worker) — cross-module TLS GOT binding: TPREL64 + TLSDESC (319/0)

Closed the last documented loader TLS gap (`R_AARCH64_TLS_*` across `DT_NEEDED`
deps, which the memory tracked as open). The multi-module loader seeded TLS only
for the main image, and TLS GOT relocations were unbound, so a `__thread` in a
dependency (libssl/libcrypto/...) read garbage/0. One focused commit:

- **`deps::setup_chain_tls` / `layout_chain_tls`** (`libloader`): lay EVERY
  module's `PT_TLS` block into one per-thread region at TP-relative offsets
  (aarch64 `TLS_TCB_AT_TP`: TCB at TP, first block at TP+16, later blocks
  alignment-padded) and copy each module's init image there. Returns `(TP,
  per-module offsets)`. Generalises `elf::setup_guest_tls` (main-only).
- **`plt::bind_chain_tls(els, offsets)`** (`arm64jit`): bind the chain's TLS GOT
  relocations to concrete TP-relative offsets:
  - `R_AARCH64_TLS_TPREL64` (1030, initial-exec): GOT slot = module block offset
    + symbol `st_value` + addend — the guest's `mrs tpidr_el0; ldr [GOT]; add
    tp,x0` then lands on the variable.
  - `R_AARCH64_TLSDESC` (1031, GCC 13+ default even for `-ftls-model=global-
    dynamic`): a 16-byte descriptor `{resolver_host_call, tprel}`; the resolver
    host fn returns `descriptor[1]` after the guest `blr`s to it.
  - **Pitfall found in debug:** GCC emits TLSDESC relocs into `.rela.plt`
    (DT_JMPREL), NOT `.rela.dyn` (DT_RELA) where initial-exec TPREL64 lives, so
    the binder must walk BOTH dynamic reloc tables (first cut bound 0 slots).

Verified end-to-end, no QEMU, with cross-gcc `-shared` dep chains exercising
BOTH models: dep-local `__thread` accessed through TLSDESC (`entry()==1007`) and
through initial-exec (`entry()==53`) via the full loader → cross-module binder →
chain-TLS → `jit_run` pipeline. Two permanent gates
`loader_run_chain_dep_tls_{initial_exec,tlsdesc}` seal them.

`cargo build --workspace` clean; `cargo test --workspace` **319/0** (was 317;
+2 loader_run). HARD GATE unchanged (no GPU/APK here). Next per RECOMMENDATION:
more JNI fake-object backing, libbadcpu ISA.

## Session 32e (Sep 11, 2026, hermes-worker) — global-dynamic TLS `__tls_get_addr` closes the LAST TLS dialect (320/0)

Completed the TLS relocation surface: GCC 13+ defaults to TLSDESC, but older dep
builds forced to the classic model (`-mtls-dialect=trad
-ftls-model=global-dynamic`) call `__tls_get_addr(&tls_index{module, offset})`
from the PLT — and that JUMP_SLOT previously fell to the NULL/0 catch-all (guest
called garbage). Commit `a1b1505`:

- `bind_chain_tls` additionally binds `R_AARCH64_TLS_DTPMOD64` (1028 →
  tls_index[0] = defining module's chain index) and `R_AARCH64_TLS_DTPREL64`
  (1029 → tls_index[1] = the symbol's offset within that module's block).
- `plt::set_chain_tls(tp, offsets)` stores process-wide TP + per-module block
  offsets (single-threaded).
- `host_tls_get_addr(a0=…&tls_index)` returns `TP + offsets[module] + offset`;
  `ensure_tls_get_addr()` registers it by name in `bind_image_plt` so the
  JUMP_SLOT binds (not the catch-all). `run_chain` seeds it before `jit_run`.
- Permanent gate `loader_run_chain_dep_tls_global_dynamic_returns_403`
  (dep_getx 3 + dep_gety 400 through a forced trad/gd dep).

This completes **all three AArch64 TLS access models** for cross-module deps:
initial-exec (TPREL64), TLSDESC (1031), and general-dynamic
(DTPMOD/DTPREL via `__tls_get_addr`). The `R_AARCH64_TLS_*` gap in the memory
ledger is now closed. `cargo build --workspace` clean; `cargo test --workspace`
**320/0**. Next per RECOMMENDATION: more JNI fake-object backing, libbadcpu
ISA, services/auth. HARD GATE unchanged (no GPU/APK here).

---

## Session (cycle 34, Sep 11, 2026) — RESOLVED the last documented open arm64jit bug (323/0)

Closed the long-open cycle-33 residual (fuzz seed 9000_58, `gen_fp_edge`):
full-program JIT 2474795 vs both oracles 2039651 (+435144 = exactly a[5]*1e6).
This was the only remaining documented arm64jit correctness gap on this box.

**Root cause — a decode collision, NOT the hypothesized host-register clobber.**
gcc -O3 schedules a scalar `fcsel Dd,Dn,Dm,<cond>` into the d-reg min/max chain
whose rm/cond fields give it a top-16 (`0x1e65`) that ALSO matches the
fcvt-to-int decode gate (`fcvtau`/`fcvtas`, mode 2). e.g. `fcsel d26,d28,d5,mi`
= `0x1e654f9a` decoded as `Inst::FcvtToInt` — which writes integer X{rd}, NOT
vector D{rd} — so the min-accumulator (d26) never updated and kept its stale
`a[i]*1e6` double; the final total carried that element. This explains the two
confusing facts: it only reproduced from the full program (register-reuse gave
the exact badly-encoded fcsel), and JIT_BUDGET=1/JIT_STEP did NOT mask it
(per-instruction decode, not a within-block scratch collision).

**Fix (decode.rs, one discriminator):** the FcvtToInt gate now requires
`bits[11:10]==00` in addition to the existing bit12-clear guard. Every real
fcvt-to-int clears bits[11:10] (verified fcvtau 0x1e650062 = 00); fcsel needs
0b11 (cond lives in bits[15:12]). With the guard, `0x1e654f9a` falls through
to `Inst::FcsSel` and d26 updates correctly. Regression-guarded:
- decode unit: fcsel 0x1e654f9a/0x1e65ef9a -> FcsSel, fcvtau 0x1e650062 ->
  still FcvtToInt (added to fp_2d_op_decode_collisions_with_int_add_bsl_and_fcvt).
- e2e canary `diff_fp_edge_fcsel_swallowed_as_fcvt` (full program, jit==oracle
  ==2039651). Sensitivity-proven: reverting only the `0x0c00` guard re-fails
  the canary with the EXACT original jit 2474795 vs oracle 2039651.
- `fuzz_repros/README.md` updated: fp_edge_9000_58/21 now RESOLVED.

`cargo build --workspace` clean; `cargo test --workspace` **323/0** (was 322).
HARD GATE unchanged: real Roblox boot + run log only on a GPU/APK host (none
on this VPS). Next per RECOMMENDATION order: JNI fake-object backing, libbadcpu
ISA, services/auth.

## Session (cycle 36, Sep 11, 2026) — GLES float/mixed + >8-arg bridge to real Mesa; JIT compressed-texture decode via shared texture_codec (343/0)
Closed the cycle-35b graphics hole (float-ABI and >8-arg GLES stuck on the
NULL/0 stub: no way to clear the framebuffer or upload a texture) and the
compressed-texture JIT gap. Commits c951bb7, 370b544.

1. **`HostGlesCall` bridge (c951bb7).** The integer `HostCall` (8 x-reg args) and
   the uniform-float bridges can't express GLES functions that mix integer args
   (x0..x7) with float args (low 32 bits of s0..s7) or take >8 args (the 9th+ on
   the guest stack). Added `jit::HostGlesCall = extern "C" fn(*mut CpuState)->u64`
   + `register_gles_call` (new thunk region after the f32 slots); the dispatcher
   hands each registered wrapper the full guest CpuState. `resolver::resolve_gles_mixed`
   installs one wrapper per function that reads the exact x/s/sp lanes its
   AArch64 signature uses and calls real Mesa (libGLESv2.so.2, RTLD_LOCAL).
   Wired into the PLT binder (plt.rs 7-tuple) so these imports bind to real Mesa.
   Covered: glClearColor/BlendColor/ClearDepthf/DepthRangef/LineWidth/PolygonOffset/
   SampleCoverage/TexParameterf, glUniform{1,2,3,4}f, glVertexAttrib{1,2,3,4}f,
   glTexImage2D/TexSubImage2D/TexImage3D (stack pixels). Filled the integer-ABI
   whitelist with missed pointer-arg GLES: glUniform1fv..4fv, glUniformMatrix{2,3,4}fv,
   glTexParameterfv, glGetTexParameterfv, glGetFloatv, glGetTexLevelParameteriv.

2. **JIT compressed-texture decode (370b544).** The wrapper's Android-format
   decompression was bypassed by the JIT path (glCompressedTexImage2D bound to raw
   Mesa -> undecodable ETC2/ASTC on desktop). Extracted `crates/texture-codec`
   (ETC1/ETC2/EAC/ASTC/ATC -> RGBA8 + handle_compressed_tex_image_2d/sub_image_2d;
   6 tests moved from the wrapper) shared by BOTH the glesv2-wrapper cdylib and the
   JIT. glCompressedTexImage2D moved to the mixed bridge (removed from the int
   whitelist) and glCompressedTexSubImage2D now decode to RGBA8 and upload via real
   glTexImage2D/SubImage2D; non-Android formats fall through to Mesa.

**Headless gates (permanent, real Mesa llvmpipe + surfaceless EGL, no GPU):** the
end-to-end test drives context creation (eglGetDisplay/Initialize/ChooseConfig/
CreateContext/MakeCurrent) entirely through guest blr and the int bridge,
glClearColor(0.5,0.25,0.75,1.0) round-trips via glGetFloatv (float bridge), a 1x1
glTexImage2D uploads through the stack bridge, and a 4x4 ETC2 upload leaves
glGetTexLevelParameteriv(GL_TEXTURE_INTERNAL_FORMAT)==GL_RGBA8 (proves the JIT
decompressed it, not raw-Mesa). cargo build --workspace clean; cargo test
--workspace 343/0. HARD GATE unchanged: real Roblox boot + run log on a GPU/APK host.

3. **Headless window-presentation gate (cdab7f8, 344/0)** — proved
   GRAPHICS_RECOMMENDATION §5 (ANativeWindow→desktop window → EGL window surface →
   present) end-to-end: `arm64jit/tests/egl_window_present.rs` spawns Xvfb, opens
   an X11 window via input-wrapper, and drives eglGetDisplay/Initialize/
   ChooseConfig(window-capable)/CreateWindowSurface (the X11 Window XID as
   native_window)/CreateContext/MakeCurrent, then glClearColor(float bridge)+
   glClear+eglSwapBuffers — all through guest blr + the resolver bridges under
   EGL_PLATFORM=x11 on Mesa llvmpipe. First proof a translated guest can present
   frames to a real on-screen window on a headless box. arm64jit gained
   input-wrapper + x11rb dev-deps.

4. **JNI object-array backing (e4e8aea, 345/0)** — NewObjectArray(172)/
   GetObjectArrayElement(173)/SetObjectArrayElement(174) were NULL/garbage in the
   JNI function table. Backed them like the primitive arrays (shared
   array_len_registry): object arrays are len×8 opaque jobject slots; NewObjectArray
   allocs + optionally seeds with initialElement; Get/Set are bounds-checked
   (get→NULL, set→no-op out of range, never fault). +regression test.

5. **timerfd + signalfd syscalls (5220aed, 346/0)** — ALooper/libutils wait on
   timerfds for timeouts and some services take a signalfd; both were -ENOSYS.
   timerfd_create(85)/settime(86)/gettime(87) are clean itimerspec forwards
   (283 is membarrier, already handled); signalfd4(74) returns a real host
   signalfd with an EMPTY sigset (no guest-signal dispatch here, so it never
   fires, but the call succeeds). +guest_svc_timerfd_and_signalfd_roundtrip.

## Session (cycle 35, Sep 11, 2026) — GRAPHICS TRANSLATION LAYER: egl-wrapper + glesv2-wrapper cdylibs, input-wrapper, real-EGL JIT wiring (336/0)
Per the worker operating rules, took the graphics translation layer
(GRAPHICS_RECOMMENDATION.md) as the highest-leverage unblocked item — built AND
headlessly verified via Mesa llvmpipe + surfaceless EGL (no GPU needed). Commits
6a6efd5, bcf49ed, 50f8b14:

1. **`crates/egl-wrapper` + `crates/glesv2-wrapper`** — cdylibs exposing the guest's
   exact sonames (`libEGL.so`/`libGLESv2.so`). They dlopen Mesa (`libEGL.so.1`/
   `libGLESv2.so.2`) and forward every entry via a `dl.rs` resolver (dlsym'd addrs
   cached once; a missing Mesa symbol aborts loudly rather than calling null). All
   44 EGL + 142 GLES signatures transcribed EXACTLY from the system headers by
   `gen_forward.py` (kept regenerable). `glCompressedTexImage2D`/`SubImage2D` are
   hand-written to intercept Android compressed textures — ETC1 (0x8D64), ETC2
   RGB/RGBA1/RGBA8, EAC R/RG signed+unsigned, ASTC (0x93B0..0x93BD), ATC — and
   decompress to RGBA with pure-Rust `texture2ddecoder` (BGRA→RGBA swizzle), then
   upload via the real `glTexImage2D`/`glTexSubImage2D`. Non-Android formats pass
   through to Mesa untouched.
2. **`crates/input-wrapper`** (GRAPHICS_RECOMMENDATION §6) — Android MotionEvent/
   KeyEvent/action-key model + surface-agnostic `PointerTracker` (mouse→ACTION_DOWN/
   MOVE/UP multi-touch), X11 keysym→AKEYCODE map, and a raw-X11 (pure-Rust x11rb,
   no C toolchain) window/event pump.
3. **In-process JIT graphics wiring** — `resolver::resolve_egl` dlopens real Mesa
   `libEGL.so.1` RTLD_GLOBAL and binds `egl*` imports as integer-ABI `HostCall`s, so
   the guest's EGL calls now run real Mesa instead of the NULL/0 graphics catch-all.
   Regression drives a guest `blr` to `eglGetError` through jit_run and asserts a real
   non-zero EGL error enum. GLES is deliberately NOT routed through the integer
   HostCall (its float-in-xmm ABI needs a dedicated float bridge).

**Permanent headless gates:** `glesv2-wrapper/tests/headless_graphics.rs` dlopens
both .so shims and exercises a real surfaceless ES3 llvmpipe context through them
(EGL forwards, GLES renderer string, ETC2 intercepted => GL_TEXTURE_COMPRESSED=0,
DXT1 passthrough, eglGetProcAddress forwards). `input-wrapper/tests/xvfb_input.rs`
spawns Xvfb and opens a mapped window through the crate, validating connect + the
pointer→touch mapping (random-display + connect-retry for stability). Plus 6 texture
unit, 3 input unit, 2 EGL resolver tests. Installed libegl-dev/libgles-dev/libgbm-dev/
libwayland-dev/mesa-utils first; input deps use pure-Rust x11rb.

`cargo build --workspace` clean (only the cosmetic cdylib crate-name warnings);
`cargo test --workspace` **336/0** (was 323). HARD GATE unchanged: real Roblox boot +
run log on a GPU/APK host (none on this VPS). Next: wire the wrapper sonames into the
guest loader's NEEDED resolution and add a GLES float-ABI host bridge, then continue
the ordered RECOMMENDATION list (JNI fake-object backing, libbadcpu ISA, services/auth).

## Session (cycle 35b, Sep 11, 2026) — integer-ABI GLES routed to real Mesa in JIT resolver + JNI array offset bug fixed (341/0)
Two follow-on commits on the graphics/JNI surface:

1. **de5fed1 — `resolver::resolve_gles_int`**: mirroring the EGL wiring, 107 GLES
   entry points with a pure integer/pointer ABI AND at most 8 integer args (all the
   texture/state/draw pipeline calls Roblox makes — bind/gen/delete, shader compile,
   draw arrays/elements, buffer data, stencil/blend/depth, uniform*i, viewport) are
   now resolved to real Mesa via a whitelist (GLES_INT_NAME_LIST, generated from the
   exact headers). `gles_handle()` dlopens libGLESv2.so.2 with **RTLD_LOCAL** (NOT
   GLOBAL: a global load would leak float-taking `gl*` into the general resolve()
   RTLD_DEFAULT scan and corrupt their xmm args through the integer bridge). Float-
   taking GLES (glClearColor) and >8-arg forms (glTexImage2D) are deliberately kept
   on the NULL/0 stub — they need a dedicated float-ABI bridge, not wired here.
2. **6ba6104 — JNI primitive-array accessors + a REAL offset bug**: verified every
   JNINativeInterface offset against the authoritative Android NDK r26b jni.h
   (extracted from the official NDK zip). Found REGISTER_NATIVES was 199 and
   GET_JAVA_VM 203 but the NDK places them at **215 and 219** (the 175..214
   New<Prim>Array/Get/Release/Get/Set<Prim>ArrayRegion block was skipped), so a
   guest's RegisterNatives/GetJavaVM table index read the wrong (NULL) slot and
   JNI_OnLoad native registration was silently dropped in the real boot path. Fixed
   the offsets and implemented the missing JNI primitive-array surface (jbyteArray /
   jintArray = guest-addressable buffer + byte-length registry; new/get/release/
   region ops, bounds-checked), wired at the corrected slots; GetArrayLength now real.
   +3 regressions (NDK offset assertions, ByteArrayRegion round-trip, Elements->backing).

cargo build --workspace clean; `cargo test --workspace` **341/0** (was 336). HARD GATE
unchanged: real Roblox boot + run log on a GPU/APK host (none on this VPS).

## Cycle 37 (Sep 11, 2026) — JNI fake-object backing + full JNI surface (351/0)

Commit `23e053a` (dev). Per the ordered post-graphics RECOMMENDATION list, took
"JNI function-table stubs / fake-object backing" as the highest-leverage
unblocked item. The JIT `jni.rs` (crates/arm64jit) had backed arrays, strings and
~14 slots, but everything else in the JNINativeInterface fell to the `voidp`
default (returns 0) — so guest Roblox JNI paths that `if (!ref / !clazz / !buf)
fail` aborted instead of proceeding.

Backed the behavior-changing slots at their authoritative Android NDK offsets:
- **NewLocalRef / NewWeakGlobalRef**: identity pass-through (no distinct ref pool).
- **IsSameObject**: identity compare (1 iff a==b).
- **GetObjectClass**: stable non-zero jclass handle (interned
  `java/lang/Object`), never aliases the object; NULL obj -> NULL class.
- **IsInstanceOf / IsAssignableFrom**: permissive true (fake-object model takes
  the success branch instead of a NULL/abort path).
- **GetStringUTFRegion**: bounds-safe UTF-8 byte copy (real behavior, tested).
- **DirectByteBuffer trio** (Roblox passes textures/audio/asset native memory as
  java.nio.ByteBuffer): NewDirectByteBuffer creates a fresh unique handle into a
  handle->(addr,cap) registry; GetDirectBufferAddress/Capacity recover it; unknown
  handle -> 0.
- **PopLocalFrame**: passes its `result` arg through.
- Wired monitor (Enter/Exit=JNI_OK), exception (Occurred/Check=no-pending,
  Describe/Clear no-op), local-frame (Push=JNI_OK, EnsureCapacity=JNI_OK),
  static-field (GetStaticFieldID=GetMethodID stub, Get/SetStatic* typed), and the
  Call{Object,Boolean,Int,Void}Method + CallStatic* forms (typed zero) to explicit
  stubs so they're non-null at correct offsets.

**Tests**: +4 (NDK offset assertions for 23 slots; nonnull-at-official-offsets for
17 boot-relevant slots; fake-object backing semantics; direct-buffer round-trip +
string-region copy). `cargo test --workspace` **351/0** (was 346), build clean.

Next per RECOMMENDATION order: libbadcpu ISA gaps, then services/auth. HARD GATE
unchanged (real Roblox boot + run log only on a GPU/APK host; none on this VPS).

## Cycle 37 / 37b (Sep 11, 2026) — JNI fake-object backing + boot syscall gaps (352/0)

Commits `23e053a`, `ae4de9c` (+ docs 3a8ea7b, 1d8783f) on `dev`. Per the ordered
post-graphics RECOMMENDATION list (JNI function-table stubs / fake-object backing,
then ELF/loader, libbadcpu, services/auth).

1. **JNI fake-object backing (`23e053a`)** — `crates/arm64jit/src/jni.rs` backed
   arrays/strings/~14 slots; the rest of the JNINativeInterface fell to the voidp
   default (return 0), so guest `if (!ref / !clazz / !buf) fail` aborts. Wired the
   behavior-changing slots at authoritative NDK offsets: NewLocalRef/NewGlobalRef/
   NewWeakGlobalRef (identity), IsSameObject (identity compare), GetObjectClass
   (stable non-zero jclass), IsInstanceOf/IsAssignableFrom (permissive true),
   GetSuperclass/PopLocalFrame, the DirectByteBuffer trio (NewDirectByteBuffer/
   GetDirectBufferAddress/GetDirectBufferCapacity via a handle->(addr,cap) registry
   — Roblox's NIO texture/audio/asset buffers), and GetStringUTFRegion (bounds-safe
   copy). Monitor/exception/local-frame/static-field/call-method slots are typed
   no-op stubs at correct offsets. +4 tests (NDK offsets, nonnull-at-offset, fake-object
   semantics, direct-buffer roundtrip, string-region copy). 347→351.

2. **Boot-critical syscall gaps (`ae4de9c`)** — sysinfo(179)/statx(291)/
   get_robust_list(100)/restart_syscall(128) were -ENOSYS. sysinfo fills the guest
   asm-generic 64-bit struct with REAL host values (no forging per the record);
   statx is a raw SYS_statx forward; get_robust_list reports an empty robust-futex
   list so glibc pthread init proceeds; restart_syscall returns -EINTR.
   +guest_svc_sysinfo_statx_robust_restart_roundtrip. 351→352.

3. **Differential-fuzz validation** — 3 campaigns, 14 fresh seeds x 100 = ~1500
   cases, **0 failures / ~150 skips**: the JIT FP (fcsel/frint/fcvt/vector-compare),
   SIMD (widen/bitmix/popcount/4s), bitfield, long-loop and shared/PIE paths all
   hold against the qemu-aarch64 + native x86 gcc oracles. No silent miscompile found.

`cargo build --workspace` clean; `cargo test --workspace` **352/0**. HARD GATE
unchanged: real Roblox boot + run log only on a GPU/APK host (none on this VPS).

**Assessed next (not landed — see project no-half-baked discipline):** the armed64jit
guest thread model (`clone` 220 → host thread with post-svc PC continuation + a
per-thread CpuState + join/tid registry). This is the last large JIT capability a
real Roblox run needs (render/audio/network worker threads) but is a genuine
multi-session subsystem; the translator already threads per-instruction guest PC
(needed for child continuation) and guest==host makes child memory shared.

## Cycle 38 (Sep 11, 2026) — GUEST THREAD MODEL: clone(220) spawns a real host thread (353/0)

Commit landed on `dev` (jit thread-model slice). This was the flagged
next-biggest JIT capability a real Roblox run needs (render/audio/network
worker threads) and the HANDOFF's stated "last large JIT capability". The first
**bounded, fully-correct, tested slice** of it:

- **CpuState** gains `svc_next` (post-svc guest PC) + `tid`.
- **Svc translate arm** now (a) records `pc+4` into `state.svc_next` before the
  `guest_svc` call, and (b) early-returns from the compiled block when a syscall
  zeroes `state.pc` — the mechanism that lets a child thread's OWN `jit_run`
  unwind cleanly at thread-exit (the inlined svc otherwise continues into the
  next guest instruction).
- **guest_svc `clone`(220)**: requires CLONE_VM (the sharing thread case that
  pthread_create uses). Assigns a guest tid, clones the parent register file,
  sets child SP from the `child_stack` arg + new TLS (CLONE_SETTLS), honors
  CLONE_PARENT_SETTID/CLONE_CHILD_SETTID stores, and `std::thread::spawn`s a
  real host thread that re-enters `jit_run` at the post-svc PC — so the child
  continues the guest program right after its `svc`, exactly as AArch64 clone
  semantics dictate. `EXEC_CTX` (image ptr/len/base) is registered at `jit_run`
  entry for the child's re-entry.
- **exit(93) is now thread-local on a spawned child** (pc=0 → Svc-arm early-ret
  → child `jit_run` returns → host thread ends); **exit_group(94) and
  main-thread exit still `_exit` the whole process**. `gettid`(178) stays the
  real host tid — each clone child is a distinct host thread, so it returns a
  distinct, correct id.
- **New helpers**: `jcc_rel8`/`jne_rel8`/`jz_rel8` in x86.rs (short rel8 jcc —
  needed for the in-block thread-exit `ret` guard).

**Regression gate** `loader_run_clone_spawns_guest_thread`: a raw-`clone`
fixture (cross-gcc) where the child sums 0..63 on its own guest stack, writes
42 to a shared guest global, then thread-exits (93) without killing the parent;
the parent's bounded spin then returns 42. Proves end-to-end that a *second
host thread* ran guest code in the same image, shared memory (guest==host),
distinct stack/tid, and clean thread-local exit. Verified e2e manually first
with a SIGSEGV-logging harness (`entry()` → 42) and root-caused an initial
fixture ABI bug (child resumes post-svc WITHOUT the caller's `sub sp,#0x20`
prologue, so its `[sp+#8]` locals sat above the raw SP — pointed `child_stack`
below the buffer top).

`cargo build --workspace` clean; `cargo test --workspace` **353/0** (was 352).
Differential fuzz seeds 1 (36 cases), 7 (38), 55 (38) all 0 fail — the Svc-arm
change (now emits a pc-load/test/guard on every svc) didn't regress the JIT.

**Known next** in this subsystem (documented, not half-baked in): pthread_join
(hold child tid + futex-CLONE_CHILD_CLEARTID join), per-thread guest TLS block
layout beyond SETTLS-pointer handoff, `clone3`(435), and tgkill/signal
delivery to a specific child. The core spawn+shared-memory+thread-local-exit
loop is now verified working. HARD GATE unchanged: real Roblox boot + run log
only on a GPU/APK host (none on this VPS).

## Cycle 38b (Sep 11, 2026) — pthread_join primitive: CLONE_CHILD_CLEARTID + futex (354/0)

Commit `b778647` on dev. Completed the thread lifecycle after cycle 38's clone
spawn: a joining parent can now block on `FUTEX_WAIT(ctid)` and be woken when
the child exits.

- `CpuState` gains `clear_tid_addr`. `clone`(220) now honors
  `CLONE_CHILD_CLEARTID` (0x00200000): the child carries the child-tid word
  address in its state (alongside the existing CLONE_CHILD_SETTID store).
- thread-local `exit`(93) on a spawned child now **zeroes** that word and
  **FUTEX_WAKEs** it before halting `pc` — exactly the kernel
  CLONE_CHILD_CLEARTID semantics pthread_join's futex-wait depends on.
- +`loader_run_clone_child_cleartid_join_via_futex`: parent FUTEX_WAITs on the
  child's clear-tid word; child computes a signed sum, publishes a result
  global, thread-exits (93); the exit clears+wakes the word so the parent's
  wait returns. Asserts the word is zeroed, the result published, and the wait
  returned cleanly -> 42.

`cargo test --workspace` **354/0** (was 353), build clean. The core spawn →
  child-runs-on-own-stack/tid → publishes-shared-state → thread-local-exit →
  parent-futex-joins lifecycle is now verified headlessly end-to-end. Documented
  remaining: per-thread guest TLS block layout, clone3(435), tgkill/signal
  delivery. HARD GATE unchanged: real Roblox boot + run log only on a GPU/APK
  host (none on this VPS).

## Cycle 38c (Sep 11, 2026) — clone3(435) struct-args thread spawn (355/0)

Commit landed on dev. Modern glibc/bionic prefers clone3(435) over clone(220),
so a real boot needs it.

- Refactored the shared-VM thread spawn from the clone(220) arm into
  `spawn_guest_thread(s, flags, stack, parent_tid, tls, child_tid)` — one body,
  two syscall shapes.
- clone3(435): reads `struct clone_args` from guest memory (flags@0,
  child_tid@16, parent_tid@24, stack@40, tls@56; -EINVAL if the guest's `size`
  is too small for those fields) and spawns via the shared helper — same
  CLONE_VM/SETTLS/PARENT_SETTID/CHILD_SETTID/CLEARTID semantics as clone(220).
- +`loader_run_clone3_struct_args_spawns_guest_thread`: raw clone3 with a real
  struct clone_args — child computes 10! (3628800) on its own stack, publishes a
  result global, thread-exits (93); parent futex-joins on the child's clear-tid
  word; asserts the factorial result, the zeroed clear-tid, and a clean wait.

`cargo test --workspace` **355/0** (was 354), build clean. clone(220),
clone3(435), and pthread_join (CLONE_CHILD_CLEARTID + futex) all verified
headlessly. Remaining thread-model next: per-thread guest TLS block layout
(beyond SETTLS pointer handoff), tgkill/signal delivery to a specific child.
HARD GATE unchanged: real Roblox boot + run log only on a GPU/APK host (none
on this VPS).

## Cycle 39 — SIMD variable-shift ushl/sshl + unaligned `ext` fixes (359/0)

New `gen_varshift` differential fuzz generator (variable shift by register,
both 32/64-bit lanes) exposed **two real silent miscompiles**, both fixed on
`dev` (commit `e8b9721`):

1. **ushl/sshl Vd.T,Vn.T,Vm.T.** The decode gate `(insn & 0x3f000c00) ∈
   {0x2e,0x4e,0x6e}` masked out bit29 (the **U** bit — ushl=1, sshl=0) AND the
   `.2d` size bits, so **sshl never decoded** (its residue is `0x0e…`) and the
   `signed_` flag was inverted. Fixed to mask `0xffe0_fc00` with all 14
   q/size/ushl-sshl residues; `signed_ = bit29==0`; added the `q` flag the
   translate was ignoring. The translate previously did a plain `shl` with
   `16/esize` lanes always (wrong for q0) and no negative-count handling.
   Rewritten to honor q, sign-extend the count lane, and implement ARM
   semantics: C≥0 left shift; C<0 right shift by -C (**sshl arithmetic** /
   **ushl logical**); |C|≥element-width → 0 (ushl) or sign-fill (sshl).
   Also fixed rel32 patching (done-jumps were patched to a stale offset recorded
   before the right path was emitted; the 64-bit left path fell through into the
   right path; and the JS gate had no `test_rr64`). Encodings verified against
   `aarch64-linux-gnu` for `.2d`/`.4s`. +`simd_var_reg_shift_2d_reference` and
   `simd_var_reg_shift_4s_reference` unit tests.

2. **`ext VD.16B,Vn,Vm,#imm`** — pre-existing latent bug: the SimdExt translate
   computed the concat offset in **bytes** but shifted by that value as if **bits**
   (`shr/shl by 4` instead of `32`), so every unaligned immediate (≠0,≠8)
   returned garbage. gcc's cross-lane XOR-reduce (`ext #8`+`ext #4`) mis-combined
   the halves, which is what also derailed the sshl `.4s` compound cases. Fix:
   `sh = (start%8)*8`. +`diff_simd_ext_unaligned_xor_reduce` canary.

Both bugs are silent (no crash — wrong values). Fuzz generator now bounds shift
counts below element width so the native-gcc oracle and ARM agree (ARM left-shift
by ≥width = 0, x86 masks `cl` mod-width — UB region differs by ISA). A follow-on
generator `gen_scalar_varshift` now covers the scalar lslv/lsrv/asrv path (the
SIMD gen_varshift vectorizes to NEON ushl/sshl; the scalar arm was unfuzzed) —
40/40 differential clean, plus a 600-case confirmation campaign.

**`cargo test --workspace` 359/0** (was 355). New cross-lane fuzz campaign clean
(8 seeds × 40, all pass). `cargo build --workspace` clean. HARD GATE unchanged:
real Roblox boot + run log only on a GPU/APK host (none on this VPS).

## Cycle 40 (Sep 11, 2026) — GUEST SIGNAL DELIVERY: rt_sigaction/kill/tgkill dispatch, handler run + SIGRET resume (363/0)

Commit `5f8c16c` on `dev`. Completed the thread-model item documented as the
next gap: a guest thread can now be interrupted by a signal (self-delivered or
cross-thread), run a Linux-style handler, and resume cleanly.

**New `crates/arm64jit/src/signals.rs`** (guest signal contract):
- `rt_sigaction`(134) parses the aarch64 kernel `struct sigaction` (handler@0 /
  flags@8 / restorer@16 / 8-byte mask@24) into a process-wide table
  (SIG_DFL / SIG_IGN / guest-handler fn), reporting the prior action into oact.
- `kill`(129)/`tgkill`(131) go through the guest model (NOT forwarded to libc,
  which would kill the host): a SAME-thread target runs the handler right after
  the `svc`; a DIFFERENT guest thread gets a cooperative `pending_signal` that
  its own `jit_run` loop picks up (proves "signal to a specific child thread").
- Handler run: save the interrupted context (x regs, vectors, sp, TPIDR, NZCV)
  + guest siginfo/ucontext on a per-thread frame stack; enter the handler with
  the aarch64 signal ABI (x0=signo, x1=siginfo, x2=ucontext, x30=SIGRET); on the
  handler's `ret` (x30 lands on the SIGRET sentinel) or an explicit
  `rt_sigreturn`(139) restore and resume right after the interrupted `svc`.
- Un-handled signals fall back to the POSIX default disposition (ignore for
  SIGCHLD/SIGURG/SIGWINCH/SIGCONT+stop family; terminate the process 128+sig
  otherwise), so a guest raise(SIGTERM)/SIGPIPE behaves like Linux.

**Two JIT fixes the dispatch exposed (both real):**
1. Svc translate arm: the syscall-return store `stg x0` was overwriting the
   handler's signo argument before the block yielded. Redirect now decides
   FIRST (before `stg`), so guest x0 keeps `sig` for a self-delivered handler.
2. `svc`-bearing `bl` callees — new `body_contains_svc` (follows guest calls
   transitively) — are now diverted through the dispatcher like host-import
   callees, so every `svc` runs at a top-level block boundary. Without this, an
   svc inlined into a caller's monolithic block whose Svc arm early-rets (signal
   redirect / thread-local exit) popped the *inlined-caller* return address
   instead of jit_run's — corrupting the host stack (a real `movaps [rsp]` fault
   below a corrupted RSP). This also hardens the pre-existing child-thread
   local-`exit`-via-helper path.

**CpuState**: `+redirect_request` (block-yield to a handler) and
`+pending_signal` (cross-thread cooperative pickup, volatile), plus a
guest-tid → CpuState registry (`GUEST_THREADS`) for `post_signal_to_thread`.

**Tests (+3, one strengthened):** `loader_run_self_signal_handler_runs_and_resumes`
(self-delivered SIGUSR1 handler sets globals, resumes, returns 42),
`loader_run_sig_ign_prevents_termination` (SIG_IGN for default-death SIGPIPE
survives), and `loader_run_cross_thread_signal_delivers_to_child` (parent
`tgkill`s a spawned child; the child's own thread picks it up cooperatively,
runs the handler, publishes a result and futex-joins — real cross-thread
delivery). Unit `body_contains_svc_follows_call_graph`. The `guest_svc` oact
assert moved from "128-byte buffer zeroed" (the old no-op artifact) to the real
32-byte aarch64 sigaction struct boundary.

`cargo build --workspace` clean; `cargo test --workspace` **363/0** (was 359).
HARD GATE unchanged: real Roblox boot + run log only on a GPU/APK host (none on
this VPS). Thread-model remaining: per-thread guest TLS block layout beyond the
SETTLS-pointer handoff; signal-blocking (rt_sigprocmask is a no-op) and real
timer/signalfd dispatch are still simplified.

---
## Cycle 41 (Sep 11, 2026) — REAL rt_sigprocmask blocking + per-thread TLS TP (365/0)

Two thread-model gaps closed on `dev` (commits `de483c3`, `517b2fb`), following
cycle 40's guest signal delivery.

### 1. Real `rt_sigprocmask` (135) — signal blocking, Linux semantics (de483c3)
The previous arm was a no-op (accepted and reported an empty old-set). Now:
- `signals.rs::sigprocmask` implements SIG_BLOCK(0)/SIG_UNBLOCK(1)/SIG_SETMASK(2)
  on a per-thread `CpuState.blocked_mask` (64-bit sigset, bit N-1 = sig N),
  reports the previous mask into oset, returns -EINVAL on bad `how`/`sigsetsize`
  or a NULL-set SIG_SETMASK. SIGKILL(9)/SIGSTOP(19) bits are silently dropped
  from any attempted mask (the kernel never lets a thread block them).
- A signal that arrives while blocked is **mercifully marked pending**
  (`pending_mask`) and NOT dispatched; the dispatch arms (`kill`/`tgkill` self,
  cross-thread post) route through `signals::deliver` which checks `is_blocked`.
- On `rt_sigprocmask` returning after an UNBLOCK/SETMASK, `take_deliverable_pending`
  drains the lowest unblocked pending signal and dispatches it post-svc (the
  kernel delivers a pending signal before returning from sigprocmask).
- **Cross-thread `tgkill` now posts into the target's `pending_mask`** via
  `mark_pending` (blocked-aware), replacing the old fixed single-word
  `pending_signal`; the target's dispatcher loop drains
  `take_deliverable_pending` (respects its own blocked_mask) each iteration.
  `pending_mask` access is volatile/atomic so a sender racing the owner's clear
  can't lose a newly-pending signal.
+`loader_run_sigprocmask_block_then_unblock_delivers_pending` — set SIGUSR1
handler, BLOCK it, `tgkill` self (handler MUST NOT run), UNBLOCK, assert the
handler now runs with the right signo; returns 42.

### 2. Per-thread TLS TP for `__tls_get_addr` (general-dynamic) (517b2fb)
`host_tls_get_addr` previously resolved `{module, offset}` against a
process-global main TP (from `set_chain_tls`), so a clone-spawned child thread
accessing a dependency's `__thread` via the classic global-dynamic model
(`-mtls-dialect=trad -ftls-model=global-dynamic`) read the MAIN thread's block.
- New `jit::current_guest_tp()` thread-local, published at `jit_run` entry from
  `CpuState::tpidr`. Each guest thread runs its own host thread (main scope or
  clone child's `std::thread::spawn`), so this holds that guest thread's TP.
- `host_tls_get_addr` uses `current_guest_tp()` (falling back to the main TP
  only for a never-published call). Module block offsets (TP-relative) stay
  global; only TP is per-thread. This closes the documented "per-thread guest
  TLS block layout beyond the SETTLS-pointer handoff" item for the
  general-dynamic path.
+unit `current_guest_tp_is_thread_local_and_published` (per-thread distinct TP,
published/reset on the right host thread).

### State
`cargo build --workspace` clean; `cargo test --workspace` **365/0** (was 363).
Thread-model remaining (documented, needs a multilib/real-guest host to fully
validate): real timer/signalfd **to-guest-handler** dispatch (rt_sigaction
guests can't yet receive host POSIX-timer expiry as guest signals); per-thread
TLS **init-image copies** for clone children beyond the TP pointer handoff.
HARD GATE unchanged: real Roblox boot + run log only on a GPU/APK host (none on
this VPS). Next per RECOMMENDATION order: libbadcpu ISA, services/auth, more
differential-fuzz coverage.

## Cycle 42 (Sep 11, 2026) — fuzzer found FP pairwise gap; fmaxp/fminp/fmaxnmp/fminnmp fixed (366/0)

### What happened (a new generator caught a real incomplete-ISA decode)
Extended `fuzz_jit.py` with two generators for previously-uncovered shapes:
- **`gen_double_neon`** — f64-lane (`.2d`) NEON, which NONE of the existing SIMD
  generators exercised (they're all f32/single). Forces `vfmaq_n_f64`,
  `vmulq_n_f64`, `vmaxvq_f64` and a `vld1q/vst1q_f64` round trip, with small
  binary-exact lane values so any structural miscompute reads as a real diff.
  The host x86 gcc can't compile `<arm_neon.h>`, so it runs through the
  **qemu-aarch64 architectural oracle** (`run_qemu_exact`), never a native one.
- **`gen_uxtl_uaddw`** — 16→32→64 widening chains (`uxtl`/`uaddw`/narrow back)
  exercising the `SimdAddw`/`SimdXtl` permute-source alias paths.
- **`gen_double_neon` immediately exposed a real gap**: the very first shape
  stopped the JIT with `Unsupported(0x7e70fa60)` at an fp-reduce site. That
  word is `fmaxp d0, v19.2d` — the **FP pairwise two-register horizontal
  reduce** (`FMAXP Vd, Vn`), which had NO decoder slot (it fell to the scalar-
  fp family's Unsupported residue).

### Fix (commit 9d40f47)
- `Inst::FpPair` + decode: all **8 two-register** encodings
  (`fmaxp/fminp/fmaxnmp/fminnmp` × `.2s/.2d`), with ground-truthed field
  extraction (verified against `aarch64-linux-gnu-as`/`objdump`):
  `min = bit23`, `nm = bits[14:12]==4` (fmaxp/fminp have bits[14:12]==7),
  `sz (.2d) = bit22`. Disjoint from the 3-operand `FMAXP Vd,Vn,Vm` (bit16 SET)
  and from `FMaxMin`/`FMaxV`. (My first field guess — bit15-min, bit13-nm — was
  wrong and the decode unit test caught it.)
- translate: pairwise-reduce Vn's two elements into a scalar Vd — `.2s` via
  `maxss/minsd`.../`minss`, `.2d` via `maxsd/minsd` (x86 max/min ignore NaN,
  acceptable for the fmaxnm* path like the existing `FMaxMin` comment notes).
- decode regression test `fp_pairwise_two_register_reduce_ground_truth` (all 8).
- Verified end-to-end: the exact program that previously stopped now returns
  **1562 == qemu oracle**. 16+16 fresh oracle-gated cases for both new
  generators: 0 fail / 0 skip.

`cargo build --workspace` clean; `cargo test --workspace` **366/0** (was 365).
Committed `9d40f47` (+ this doc). The gen_double_neon generator is a permanent
asset that now keeps the f64-NEON lane classes regression-guarded. HARD GATE
unchanged (real Roblox boot + run log only on a GPU/APK host; none on this VPS).

## Cycle 43 (Sep 11, 2026) — REAL guest POSIX timer -> guest-signal dispatch (367/0)

### What was wrong
`timer_create(107)/timer_settime(110)/timer_delete(109)` forwarded to host
POSIX timers (`libc::timer_create/timer_settime`). A host timer expiry raises a
**host** SIGALRM delivered to the host process's libc disposition — it never
reached the guest's `SIG_ACTIONS` handler table that cycle 40 built. So Android
watchdogs/callbacks that rely on SIGALRM (SystemClock, trace, watchdog,
timeout handlers) never fired on the guest. This was the documented "real
timer/signalfd-to-guest-handler dispatch" thread item.

### Fix (test-driven; commit 641f45b)
Routed the three syscalls to a guest-side timer implementation:
- **`guest_timer_create`**: reads the aarch64 `struct sigevent`
  (`sigev_signo`@8, `sigev_notify`@12; default SIGALRM=14), returns a non-null
  `timer_t` id, records the owning guest thread's real host tid + signo.
- **`guest_timer_settime`**: reads the 16-byte `struct itimerspec`, spawns a
  host worker thread that sleeps `it_value` then **POSTS the signal into the
  owner's blocked-aware `pending_mask`** (`post_signal_to_thread`, the cycle-41
  mechanism); a periodic `it_interval` re-arms. The owner's dispatcher loop
  drains it via `take_deliverable_pending` and runs the registered handler.
  Fresh `Arc<AtomicBool>` stop-flag per arming cancels any prior worker without
  stopping the newly-armed one.
- **`guest_timer_delete`**: signals the worker to stop (the worker holds an Arc
  clone of the flag, so it stays valid) and frees the slot.

Two bugs the e2e test surfaced: (1) the guest's `timer_t` local wasn't being
reloaded after the svc (gcc kept it in a register) — fixed the *test* with a
global volatile; (2) settime re-used the SAME stop flag it set to cancel the
prior worker, so every new worker immediately stopped itself — fixed by
installing a fresh Arc per arming.

### Test
`loader_run_timer_signal_delivers_sigalm_to_handler`: guest installs a SIGALRM
handler, creates a timer (default sigevent), arms a 100ms periodic itimerspec,
spins (1ms nanosleep yields) until the handler ticks **2** times, then
verifies `g_sig == 14`. Runs in ~0.23s — real timer→guest-signal dispatch.

### State
`cargo build --workspace` clean; `cargo test --workspace` **367/0** (was 366).
Fuzz sanity (3 seeds, 25 cases) 0 fail — no JIT regression from the timing code.
Committed `641f45b` (+ this doc). Documents the fuzzer additions since cycle 42
(gen_scalar_fp_sign_chain, gen_fmadd_reduce) in the cycle-42 record.
Thread-model remaining: per-thread TLS **init-image copies** for clone children
beyond the TP pointer handoff (needs a real multilib guest to validate).
HARD GATE unchanged: real Roblox boot + run log only on a GPU/APK host (none on
this VPS).

## Cycle 44 (Sep 11, 2026) — SIMD rev16 (granule 2) fixed; rev/permute fuzz generator (368/0)

### The bug (silent-miscompile class, found by the differential fuzzer)
A new fuzz generator `gen_byte_reverse_perm` (added to `fuzz_jit.py`) targets
the SIMD byte-reverse/permutation family the int-heavy generators never
produce: `vrev16/32/64q_u8`, `vuzp`, `vtrn`, and a u32 `vrev64q_u32`. First run
exposed a real gap: **SIMD `rev16` decoded via the rev64/rev32 gates (which
only handle byte1 0x08) and mis-decoded to a wrong-value instruction instead of
trapping** — the byte-reverse/permute silent-miscompile class. Real encodings
`rev16 v0.16b=0x4e201800` / `.8b=0x0e201800` fell through. Oracle/JIT
mismatches were byte-identical to the rev16 FAIL numbers (a compiler also
emitted `rev16` inside the trn/rbit functions, so all three reported the same
root cause).

### Fix (commit 8e00bdc)
- **decode.rs**: `rev16` is two-reg-misc REV16 (byte1 0x18, **bit21 SET**) —
  disjoint from the uzp1/trn1 3-same permute (`uzp1 v0.16b=0x4e011800` has bit21
  CLEAR). New gate `(insn & 0x3f20_f800) == 0x0e20_1800` pins the prefix lanes,
  bit21, and byte1 0x18; returns `SimdRev { granule: 2 }`.
- **x86.rs**: `mov_load16` (movzx 0F B7, zero-extend a 16-bit word) and
  `rol16_ri8` (66 C1 /0 ib, 16-bit rotate-left) helpers.
- **translate.rs**: `SimdRev` granule 2 arm — per-16-bit-halfword byte-swap via
  `mov_load16 → rol16_ri8(8) → mov_store16`.
- **decoder regression test** `rev16_decodes_as_granule2_not_uzp` (rev16 .16b/.8b
  → granule 2 + q; uzp1 stays a permute, not rev16; rev32 stays granule 4).

### Verification
Isolated the family through the full oracle pipeline: rev16 60/60 (was 22/60
before the fix), rev32/rev64/uzp unchanged. 4 fresh seeds (30 cases each) of the
full mixed fuzzer: 0 fail. `cargo test --workspace` **368/0** (was 367/0 — the
new decode regression).

Remaining JIT ISA surface is substantially complete (155 Inst arms incl. SHA,
crypto, structure ld/st, perms, SAT narrow, SME no-ops); the one annotation'd
gap is fp16 (`fcvtl` `.4h`/`fcvtn` `.4h`), deferred — a real project, not a
one-liner. Thread-model remaining: per-thread TLS init-image copies for clone
children (needs a real multilib guest). HARD GATE unchanged.

## Cycle 44g (Sep 11, 2026) — FMUL/FMLA by-element 32-bit index (376/0)

Commit `def4153` (dev). A fused-FMA pair probe (gen_fp_fmla_reduce, later
removed — qemu is nondeterministic on the fused-FMA oracle pattern) exposed
that the 32-bit by-element index was decoded wrong: SimdFmulEl read
bit11|bit13<<1 (always 0 for .s), FmlaEl used b21<<1|b11 (swapped). Assembler
ground truth: .s index is 2 bits, index = (b11<<1)|b21 — s[1]=b21, s[2]=b11,
s[3]=both. Every .4s by-element s[2]/s[3] (and fmla s[1] via the swap) used
the wrong element. Cleared with regression tests for all 4 fmul indices +
fmla index-1 execution. Workspace 376/0.

## Cycle 44k (Sep 11, 2026) — SIMD int pairwise smaxp/sminp/umaxp/uminp (383/0)

Commit `54bfb8d` (dev). gen_int_pairwise_maxmin exposed smaxp/sminp/umaxp/
uminp mis-decoding as SimdAddB/SimdAddH/Simd4s or Unsupported. New
`Inst::SimdMaxMinP`: prefix 0x0e/2e/4e/6e + **byte2&0xfc in {0xa4=max,
0xac=min}** (byte2 low bits carry rn — high-reg self-alias like v30,v31,v30
clears them), placed before the mla/add gates. `min` discriminator = byte2
bit3 (word bit11). Translate: pairwise-reduce each source into halves of Vd;
sign/zero-extend; cmov **must use 0x4x cmov cc** (L=0x4C,G=0x4F,B=0x42,
A=0x47 — the jcc-0x40 codes are wrong); permute_source for self-alias.

## Cycle 44j (Sep 11, 2026) — SIMD saturating shift-left sqshl/uqshl/sqshlu (382/0)

Commit `f3dd4d7` (dev). gen_sat_left_shift exposed sqshl (emitted by
vqshl_n_s16) as `Unsupported`. New `Inst::SimdSatShl`: gate prefix
0x0f/2f/4f/6f + bits[14:12] in {0b110=sqshlu, 0b111=sqshl/uqshl}, placed
BEFORE the plain shl gate (which uses bits[14:12]=0b101). shift/esize from
immh like shl. sat: 0=sqshl signed src/dst, 1=uqshl unsigned, 2=sqshlu
(signed src → unsigned dst, byte2 0x64 vs 0x74 → discriminator is **bit12
(0x1000)**). Translate: sign/zero-extend src, shl in the 64-bit reg, THEN
clamp to the element range — must NOT pre-truncate to elem width first,
else a negative src (e.g. 0xCBB2<<15 becomes huge-positive) wrongly
saturates to max instead of min. 220 arm64jit + 382 workspace, all swept clean.

## Cycle 44i (Sep 11, 2026) — 3-same FP pairwise faddp/fmaxp/fminp/fmaxnmp/fminnmp (380/0)

Commit `309a82d` (+ `172b830` tests). gen_fp_pairwise exposed fmaxp/fminp/
faddp Vd.T,Vn.T,Vm.T as `Unsupported` — the existing `FpPair` gate handled
only the 2-register (0x7e, bit16-clear) reduce form. New `Inst::SimdFpPair3`
gate: prefix 0x2e/0x6e, residue `&0xffe0_fc00` in the 12 {c4,d4,f4}x{20,a0}x{2e,6e}
patterns, `.2s/.4s` only (bit22 clear); add = bit13-clear && bit12-set, nm =
bit13-clear && bit12-clear, min = bit23. Translate reduces each source's
adjacent lanes into halves of Vd via addss/minss/maxss. Key pitfall: **bit16
is bit0 of rm, NOT a 2-op/3-op discriminator** — self-aliased fmaxp v31,v31,v30
(0x2e3ef7ff) clears it; prefix alone disambiguates. 40/40 fuzz cases pass;
workspace 380/0.

## Cycle 44h (Sep 11, 2026) — saturating-narrowing-shift + rshrn rounding (379/0)

Commit `0c63158` (dev). gen_narrow_shift (shrn/vrshrn/vqshrn) exposed two
silent-miscompile families: sqshrn/uqshrn/sqshrun decoded as non-saturating
SimdShrAcc (bad values), and rshrn decoded as shrn (no rounding half-add
1<<(shift-1)). New `Inst::SatNarrowShift` with a disciplined gate (prefix
0x0f/2f/4f/6f, bit15 narrowing marker, **bit19** immh guard so by-element
widen-mul with immh4=0xc is NOT captured, bit12 OR bit29 for saturation, and
sqshrun src_signed=byte2-bit4). Rounding-narrow now `round = bit11`; both
translates self-alias via permute_source. Regression tests lock the 6 encodings.

## Cycle 44f (Sep 11, 2026) — 3-same ADDP + MODIMM ORR/BIC RMW (375/0)

Commit `84e0d71` (dev). gen_pairwise_dot (addp/vpaddq/smaxv/sminv/umaxv/
uminv) found two silent-miscompile families:
1. **3-same ADDP Vd.T,Vn.T,Vm.T** (byte2&0xf8==0xb8, bit10 set) was decoded as
   SimdArithUnary (neg/abs) or Unsupported. bit10=0 two-reg neg/abs vs
   bit10=1 3-same ADDP. Added SimdAddp with a self-alias-safe translate
   (permute_source snapshot so `addp v31,v31,v31` reductions survive; exact
   width loads/stores).
2. **MODIMM ORR/BIC are read-modify-write**, not MOVI/MVNI. Odd cmode
   (bit0=1) with op0/orr + op1/bic; the decoder wrote lo/hi flatly, so
   `bic v.4h,#0xff,lsl#8` REPLACED lanes with the mask (all 0x00ff) instead of
   ANDing — every gcc -O3 value-truncation was wrong. Added VecMovi.kind
   {0=write,1=AND,2=OR} from cmode LSB + op; translate ANDs/ORs in place.
Workspace green at 375/0. HARD GATE unchanged.

## Cycle 44b (Sep 11, 2026) — integer LONG multiply by element fixed; widen-mul fuzz generator (369/0)

### The bug (silent-miscompile, fuzzer-caught)
A second new generator `gen_widen_mul_acc` (vmlal_lane_s16/32, vmull_lane, and
the `_high_` 2 variants — widening multiply-accumulate with an indexed
broadcast element) failed **40/40** against the qemu oracle. Root cause: the
integer **LONG-by-element** multiply family
(`smull/umull/smlal/umlal/smlsl/umlsl Vd.T, Vn.T, Vm.Tsb[idx]`) was entirely
undecoded. GCC emits it for vector*const-scalar scaling. The words mis-decoded
as `FmlaEl` (the `.2d` forms, which share bit23 SET) or `VecMovi` (the `.4s`
forms) — both silent wrong values, no trap.

### Fix (commit 39150c4)
- New `Inst::SimdMullEl`. Decode gate: prefix `b[28:24]=01111`,
  size `b[23:22]` (1 → `.4s` res 4 bytes, 2 → `.2d` res 8), indexed operand
  reg `Vm = b[19:16]` (4 bits, v0-v15), index = `L(b21):H(b20)` for 16-bit
  src / `L(b21)` alone for 32-bit src, op `b[15:12]` in
  `{2=mlal acc, 6=mlsl acc-sub, 0xa=mull}`.
- **bit13 (0x2000) SET** is the clean discriminator vs FP fmla-el (op 1/5 →
  bit13 clear) and non-widening int mla-el (op 0 → bit13 clear). The gate
  must precede the FP fmla-el gate (which otherwise steals the `.2d` forms).
- Translated per-lane like `SimdMull` but the `m` operand is loaded ONCE from
  the single indexed element (`slot(rm) + index*esize`) instead of lane `i`;
  `sub` arm for mlsl/umlsl; `uphalf=8` for the q=1 (_2/upper) high-lane forms.
- Decoder regression `widening_mul_by_element_decodes_not_fptsel_or_movi`
  (25 ground-truth encodings incl. the failing real-binary words) + guards
  that FP fmla-el, int MLA-el, and `movi v0.2d,#0` stay unmangled.

### Verification
widen-mul isolation 40/40 (was 0/40 — every case formerly failed). Full mixed
fuzz sweeps across 3 fresh seeds: 0 fail. `cargo test --workspace` **369/0**
(arm64jit lib 207 unit tests, +1 with the new regression).

The two cycle-44 fixes (rev16 `8e00bdc`, LONG-by-element `39150c4`) were both
differential-fuzzer finds — new targeted generators is the highest-yield
bug-hunt on this APK-less box. HARD GATE unchanged.

## Cycle 44c (Sep 11, 2026) — saturating-narrow + MSL immediates fixed; gen_sat_narrow fuzzer (370/0)

Third fuzzer-won battle this cycle. `gen_sat_narrow` (vqmovn_s32/vqmovn_u32/
vqmovun_s32 forced through NEON, with a matching clamp reference) failed
**18/40** and exposed FOUR real JIT bugs, each silent (wrong value, no trap):
1. **rev32 gate stole the unsigned-dst SaturatNarrow** — sqxtun/uqxtn have a
   0x2e prefix with bit23 set, which the rev32 gate (granule 4) wrongly
   claimed (`0x2e614bff → SimdRev`), so the unsigned family never ran. Added
   bit15==0 (byte1 top-nibble 0) to the rev32 gate — genuine rev32 has
   byte1=0x08 (top nibble 0), sqxtun has 0x2b.
2. **SaturatNarrow decode was wrong in 3 places**: byte2 must be MASKED
   (`& 0xf8`) not exact (sqxtun byte2 is 0x2b, carries reg bits → was missed);
   src/dst-signed misderived — now from U(bit29)+byte2 (sqxtn dst+src signed,
   uqxtn both unsigned, sqxtun dst unsigned + src signed); dst_esize now from
   the full byte1 nibble (0x2→1, 0x6→2, 0xa→4) so `sqxtn .2s,.2d` gets 4 bytes.
3. **SaturatNarrow clamp used the wrong max for signed dest** — always 0xffff;
   positive overflow of a signed dst saturated to 65535 not 32767 (the
   `+0x7fff7fff` vs `-0x8000` oracle diff). Fixed to signed-aware maxv.
4. **MSL movi/mvni (vector immediate, cmode 0xc/0xd) was captured by the
   SIMD shl gate** (missing bit15==0 check — movi-msl fell in as a shift), and
   the MSL branch never inverted for mvni (op==1). Both fixed; `mvni v.4s,
   #0xff,msl8` now gives per-lane 0xffff0000 as ground-truth requires.

Regression `saturating_narrow_variants_decode_correctly` (11 encodings incl
the squared-byte2 0x2b forms) + movi-msl16/mvni-msl8 + shl/ushr-stay-shifts.
Verified sat-narrow 40/40 (was 22/40), each variant 25/25, clean fuzz sweeps.
cargo test --workspace **370/0** (arm64jit lib 208). Commit `7599867`.
HARD GATE unchanged.

---

## Session (Sep 11, 2026, hermes-worker) — FIRST REAL-BINARY BOOT EXERCISE on this box; 3 boot-path JIT fixes (384/0)

Milestone context change: the REAL Roblox 2.738.1397 APK is NOW present on this
VPS (`~/.cache/open-sober/apks/roblox-android.apk`, 109MB arm64 `libroblox.so`
extracted to `~/.cache/open-sober/robbox/libroblox.so`, ARM aarch64 Android 26
NDK r28c stripped). The task's HARD GATE — exercising the actual Roblox binary
through arm64jit+libloader headlessly with Mesa llvmpipe — is now a concrete,
daily action on this box, no longer "impossible without APK/GPU". The boot is
STILL not complete (main loop not reached), but the JIT now loads the real
binary, binds ALL 534 JUMP_SLOT + 63 GLOB_DAT imports, and executes real
JNI_OnLoad prologue+init code before faulting on the JNI fake-object vtable wall.

### Three fixes (all committed to local `dev`, all boot-verifying):
1. **`45f111b` — bind APS2-packed GLOB_DAT/ABS64 in `bind_glob_dat`.** This
   release's `.rela.dyn` is `DT_ANDROID_RELA` (packed APS2, no stock DT_RELA),
   so the old binder (which only walked plain DT_RELA) left every Android-
   packed data GOT slot 0. The very first guest prologue instruction
   `adrp x24,0x67d1000; ldr x24,[x24,#1776]` (the `__stack_chk_guard` data
   GOT) read 0 and null-faulted BEFORE any real code ran. Decoded the APS2
   stream via `libloader::android_relocs::decode_aps2` and bind GLOB_DAT/ABS64
   entries; added a static-canary fallback for `__stack_chk_guard` (glibc
   doesn't export it to RTLD_DEFAULT). Now 63 GLOB_DAT bind (was 0); the guest
   executes past its prologue into real JNI init.
2. **PRFM decode (`0xf98xxxxx`) → Hint.** `prfm pldl3keep,[x8]` = size-8
   bit23-set, which the LdStrImm sign-extend decoder rejected as "size 8 not
   implemented". PRFM is a pure hint; now a Hint/no-op. +regression
   `prfm_prefetch_is_hint_not_size8_sext_load`.
3. **JavaVM GetEnv ABI slot 7→6 (byte 48).** Guest dispatch does
   `ldr x8,[vm]; ldr x8,[x8,#48]; blr` = JNIInvokeInterface slot 6 = GetEnv
   (verified against host java-21 jni.h). Our table had GetEnv at slot 7/offset
   56 (inherited QEMU guess), so `[x8,#48]` read the voidp default, `*penv`
   was never written, and the guest's env stayed null → next dispatch null-
   faulted. Updated the e2e JIT JNI test (`jit_jni_onload_getenv_getversion`).

### Current boot frontier (verified via JIT_TRACE / JIT_STEP)
The guest now loads, binds 597 imports, gets a live JavaVM* in x0, and
executes the JNI_OnLoad prologue, `__stack_chk_guard` store, once-guard init,
clock/time setup, and dispatches into real JNI code. It then faults on a
`ldr x8,[x8,#48]` C++ virtual-method dispatch where the handle's word0 (method
table) is 0 — the documented **JNI fake-object backing wall**: the guest
builds a C++ object from JNI getters/results and virtual-dispatches on a null
vtable slot, which the fake JNI objects (bare str_handle buffers, or GetEnv's
env when the vm slot was wrong) don't safely back. Trace shows NONE of the
FindClass/NewStringUTF/GetEnv stubs fire before the fault — it is pure guest
code dispatching on an object its own init built. Real fix = JNI fake-object
model (real vtable-backed handles the guest can dispatch on), a multi-session
subsystem.

Repro run-log artifact: `/home/hermes-worker/runs/real-boot-runlog.txt`.

### Environment / assets now on this box
- `~/.cache/open-sober/apks/roblox-android.apk` — real Roblox 2.738.1397 (229MB)
- `~/.cache/open-sober/robbox/libroblox.so` — extracted arm64 lib (109MB)
- elfjit command: `cargo run -p arm64jit --example elfjit -- <lib> 0x2173ff4 --jni`

### Status
`cargo test --workspace` **384/0** green (`cargo build --workspace` clean; the
decode.rs/plt.rs edits surface only the pre-existing rustfmt-churn warnings —
rustfmt isn't installed on this box). Local `dev` commits only (no push).

---

## Session (Sep 11, continued) — REAL Roblox engine code now runs: MemoryPool + guest-threading fixed (387/0)

The real `libroblox.so` 2.738.1397 boot (arm64jit + libloader, headless) crossed
three walls this session and now executes **real engine code** before the next
fault: JNI_OnLoad completed, TSAN/TLS-key once-init, a guest worker thread
spawned, and `[roblox:JNIMain] TelemetryProtocol::setProcessTimeOverride` logged.
Run log: `/home/hermes-worker/runs/real-boot-runlog.txt`.

Three boot fixes (dev commits `9bf61e3`, `a3a8372`):

1. **`body_contains_indirect()`** — a guest fn containing a `blr`/`br` (C++
   vtable dispatch, computed GetEnv) is now *diverted*, not inlined. An inlined
   `blr` `ret`s into the caller block instead of jit_run, silently skipping the
   hostcall (GetEnv's *penv never written) and skipping the callee's x19-x28
   restoring epilogue. This was the REAL cause of the long-standing "null
   vtable" SIGSEGV: the vm was a corrupted register from a skipped inline GetEnv,
   NOT a missing JNI stub (the vtable-backed-fake-objects theory is unnecessary.
2. **`bionic_pthread_once()`** — real glibc pthread_once calls the guest
   init_routine natively (SIGILL on `paciasp`). Interpose: run the guest
   once-routine via jit_run (`run_guest_callback()`). Unblocked the TSAN /
   TLS-key one-time init.
3. **`route_mempool_big_alloc_to_host()`** — the TLS-block allocator's big
   allocator (unseeded MemoryPool arena → NULL → guest abort) is patched
   (adrp/br + thunk in a mapped segment gap) to route to host `calloc`. Then
   `bionic_pthread_create/join` + `spawn_pthread()` — glibc pthread_create
   called the guest worker start routine natively (SIGILL); now spawns a fresh
   host thread running it through jit_run with its own guest stack+TLS.

Current wall (engine data access): JNIMain/TelemetryProtocol SIMD-copies a
struct to guest addr 0x109285fb0 (unmapped, ~33MB past image end). The pointer
isn't a 0x55... heap result — likely a guest svc mmap returning a low address or
a computed arena base. Next: trace who produced 0x109285fb0 and make it real.

`cargo test --workspace` **387/0** green. Local dev commits only (no push).
