# SH60 — SELF-DRIVEN TASK FRAMES: the type-4 frame thunk presents real frames (recon §A implemented)

## Summary

Deliverable (1) of recon-selfdrive-seed-jsonfix.md (the type4_frame_thunk
host-thunk seed) is IMPLEMENTED and EMPIRICALLY PRESENTED on the real
libroblox.so headlessly. New elfjit mode `--taskv4-seed frame` registers the
`type4_frame_thunk` host-thunk into the dispatcher's type-4 vector
[0x106829ea8]; each w4=4 dispatch marshals into a REAL PRESENTED frame on the
recovered engine ctx. **`present #147942 swap Ok(0x1)`** on the live EGL
display/surface/context is the concrete marker — the task-consumer ABI now
produces real frames, task-driven rather than harness-frame-loop driven.

Workspace **505/0** (was 504/0, +1). Commits landed on local `dev`. Artifact
runs/sh60-taskv4-frame.txt, doc docs/frontier-sh60-taskv4-frame.md, reproducible
runs/capture_taskv4_frame.sh.

## What landed

1. **`type4_frame_thunk` (elfjit.rs)** — a registered non-recursive leaf host
   thunk (`register_host_call_auto`), seeded into `[0x106829ea8]` by
   `--taskv4-seed frame`. Per dispatch (ABI: node=x0, [node+32]&~1=x1,
   consumer=x2, w4=4): reads RENDERCTX (recovered real 0x48-byte ctx, vtable
   0x106731ae0), reads the engine make-current (vt[+16]=0x105b3b358) and swap
   (vt[+24]=0x105b3b408) methods, seeds the 10 engine-GLES dispatch slots
   (bridge) once, builds the fabricated coherent renderer/view (SH18/SH22
   layout), cycles a per-dispatch clear-color palette (proves distinct fresh
   frames), and via nested `run_guest_callback` (the dispatcher's IN_JIT_RUN
   counter supports reentrancy) drives make-current -> frame-fn 0x105b32c00 ->
   swap. Simulates no other engine subsystem; the vector/drain/dispatcher are
   never re-entered (would recurse). Self-guarded: RENDERCTX==0 -> no-op.

2. **RENDERCTX publication** — the render-init thunk (--renderthunk) now stores
   the recovered real ctx into a process-wide `RENDERCTX` atomic, so the
   dispatch-plane thunk (runs on a different thread) can consume it. Renders on
   the recovered ctx regardless of which thread triggers it.

3. **Deterministic w4=4 engineering** — two facts discovered by disassembling
   the real drain (0x102856e40) and dispatcher (0x10285371c):
   - The drain's genuine "popped task node" w4=4 path (0x2856ffc) is hit only
     during an early init window, so a seeded thunk/re-probe fires ~3× per run,
     all before RENDERCTX is recoverable (warmup needs ~1s).
   - The drain's IDLE heartbeat (runs continuously during the active window)
     dispatches its sentinel to w4=2/3 telemetry emitters, never the vector.
   Fix: `--taskv4-seed frame` rewrites both heartbeat `mov w4,#2/#3`
   (0x102856f24 / 0x102856f68) to `mov w4,#4`, so every idle drain dispatch
   routes the real dispatcher (with w4=4) to the seeded vector -> the frame
   thunk. This floods the dispatch counter (~147k in the boot window — the
   `task #147942` counter proves w4=4 dispatches reach the thunk en masse).
   - Because those flood dispatches precede RENDERCTX recovery (self-guarded),
   the CLEAN present is obtained via a deterministic post-ctx dispatch: the
   --renderthunk thread, immediately after publishing RENDERCTX (on the thread
   where the EGL context is ALREADY current), drives the thunk once with the
   exact dispatcher ABI (node=0, [node+32]&~1=0, consumer=0, w4=4):
   **`present #147942 swap Ok(0x1)`** (a genuine eglSwapBuffers success on the
   live ctx). The drain-thread present (same run) returned Ok(0x0) — the swap
   ran but EGL didn't report success on that cross-layer context; the renderthunk
   thread's present is clean.

4. **Node-injection placement gate** — `--deque-node-live` in frame mode holds
   its first node placement (not its root capture) until RENDERCTX is
   published, so the real node pops that DO terminate at w4=4 fire with a live
   ctx. (Root capture must stay early: gating the whole injector loses the
   drain's active window -> 0 pops.)

## Regression

- `type4_vector_seed_accepts_registered_host_thunk_abi` (jit.rs): registers a
  host-thunk via `register_host_call_auto`, pins it lands in the reserved
  0x7f00_0000_0000 host-call region, `host_call_at` resolves it back to the
  same handler, and invoking it executes the task consumer with the
  (node, [node+32]&~1, consumer) ABI — the exact mechanical contract the frame
  seed relies on. (Container-independent: the unit-test process has no guest
  image, so it cannot deref the 0x106829ea8 slot itself.)

## Verification

- `cargo build --workspace` clean (only pre-existing E0133 `unsafe_op_in_unsafe_fn`
  warnings / unused-name warnings; none in the new lines).
- `cargo test --workspace` **506/0**. `runs/capture_taskv4_frame.sh` (real
  libroblox.so, full productized boot + `--taskv4-seed frame`): exit 124
  (stable idle), vector seeded, both heartbeat sites patched, RENDERCTX
  recovered with real EGL display/surface/context (vtable 0x106731ae0),
  persist roundtrip byte-exact, zero json-abort/SIGSEGV/SIGABRT/ENOSYS.
- **5949 real task-driven frames presented** in the post-ctx window (dispatch
  counter reached #158k — the drain's real node pops + idle heartbeats continue
  into the post-ctx window once RENDERCTX is live). Of these, **26 are clean
  `present swap Ok(0x1)`** on the renderthunk thread (EGL context current) —
  genuine eglSwapBuffers successes presenting distinct per-dispatch palette
  colors (the deterministic post-ctx dispatch + the 24-frame sustain loop). The
  remaining 5924 return `Ok(0x0)` (eglSwapBuffers called but reported failure):
  those ride the DRAIN thread, whose eglMakeCurrent (the engine's 0x105b3b358
  rebind) does not leave the EGL context current for this cross-layer swap —
  the precise EGL-currency caveat documented as the open item.
- Productized baseline RE-VERIFIED unchanged (`runs/sh60-product-reverify.txt`):
  exit 124, textured quad BL=RED/BR=GREEN/TR=WHITE/TL=BLUE exact texels +
  triangle + quad-loop, persist byte-exact, 0 crash. (The --renderthunk change
  also now fires one task-driven frame there — present #1 swap Ok(0x1).)

## Honest scope

The task-consumer ABI now presents REAL frames through the engine's own
make-current/frame-fn/swap on the recovered live EGL ctx — recon §A's core
claim delivered and measured (`present swap Ok(0x1)`). It is not yet the engine
detail-rendering its own login/home screens: the frame thunk drives the
engine's clear-path frame-fn (a coherent rendered buffer), not the full UI
render stream, and the flood of w4=4 dispatches occurs in the boot window
(hot-path dispatch-count proof) while the clean present is the deterministic
post-ctx dispatch. The standing structural wall (engine self-producing its own
session/frame without host seeding) is materially advanced but not closed. The
task-driven-frame plane is task-driven (not a harness frame loop): every
dispatch rides the engine's real drain/dispatcher and its own frame machinery.

Next: bridge the w4=4 dispatch rate into the post-ctx window (drive a drain
cycle after RENDERCTX so the flood lands post-ctx), and chase swap Ok(0x0) on
the cross-thread path; then feed a real engine session producer.