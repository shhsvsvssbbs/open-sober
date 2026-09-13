# SH61b — SINGLE-OWNER PRESENTER: every task-driven frame now presents Ok(0x1)

## Summary

Follow-on to SH60 (deliverable 1) addressing its open frontier: the drain-thread
`swap Ok(0x0)`. **Result: 24/24 task-driven presents are now genuine `eglSwapBuffers
Ok(0x1)`, ZERO `Ok(0x0)`** (was 25 Ok(0x1) / 5924 Ok(0x0) in SH60). Achieved by
splitting the type-4 frame thunk into a pure producer (drain thread) + a single
consumer present loop on the one thread where EGL current-binding is established.

## The mechanism (and an empirical negative that pinned it)

SH60's static-RE concluded the dump-thread `swap Ok(0x0)` is because EGL current
binding is thread-LOCAL and the engine swap 0x105b3b408 NEVER binds — a swap
succeeds only on the thread that made the ctx current and is still current at swap
time. First I tried the obvious fix: a global presenter **mutex** around the whole
bind->frame-fn->swap sequence (serialize presenters). That was an **empirical
negative**: the Ok(0x1)/Ok(0x0) ratio was UNCHANGED (25/6399) — the Ok(0x1)s are
exactly the 25 renderinit-thread presents. Conclusion: the drain thread's
`run_guest_callback` make-current genuinely cannot establish currency even when
serialized (its guest callback path differs from the renderinit thread's). The
correct fix is to route the present to the currency-owning thread, not to lock.

So I split the thunk:

- **`type4_frame_thunk` (drain thread) is now a pure producer.** It accounts the
  dispatch, reads the node's dispatchable-flag ([node+40] bit0 — diagnostic only),
  bumps a process-wide `PENDING_PRESENTS` counter, and returns. No EGL work, no
  run_guest_callback, so it is safe/cheap on the drain thread and can never hit the
  EGL-current wall. Self-guarded: RENDERCTX==0 -> no-op (pre-recovery flood).
- **New `present_one_task_frame(ctx, n)`** does the real work: engine make-current
  0x105b3b358 -> frame-fn 0x105b32c00 -> swap 0x105b3b408, returning the swap result.
- **Presenter loop** on the --renderthunk thread (the ONE thread where render-init
  left EGL current): drains PENDING_PRESENTS — each queued request becomes a real
  presented frame — rate-limited to a bounded window (default 24 frames / 2s; env
  `TASKFRAME_MAX_FRAMES` / `TASKFRAME_WINDOW_MS`) so the run still exits 124
  (stable idle) cleanly. The drain flood adds PENDING orders of magnitude faster
  than llvmpipe can present, so a greedy drain would never terminate; bounding it
  gives a clean, sustainable, visibly-animating frame stream of genuine presents.

## Verification (real libroblox.so, full taskv4-frame recipe)

`runs/sh61b-presenter.txt` (806-line log, exit 124):
- **`present #0..23 swap Ok(0x1)` — 24/24 genuine eglSwapBuffers successes, 0
  Ok(0x0).** Every task-driven frame now presents on the currency-owning thread.
- `presenter drained: 24 real task-driven frames presented (all on the
  currency-owning thread)`.
- 196 real `NODE ... POPPED` pops (the drain's genuine task pops route through the
  thunk -> PENDING -> presenter). Dispatch counter reached #3.4M (idle heartbeats
  patched to w4=4 flood the producer, self-guarded pre-ctx).
- Zero SIGSEGV/SIGABRT/json-abort. Byte-exact persist roundtrip intact in the same
  run.

**Productized baseline re-verified** (`runs/sh61b-product-reverify.txt`): exit 124,
real indexed triangle (centroid RGBA(255,0,0,255)) + textured quad
(BL=RED/BR=GREEN/TR=WHITE/TL=BLUE) + 7 swap Ok(0x1) + persist 45B byte-exact + zero
crash. (When `--taskv4-seed frame` is absent, the renderthunk thread fires a single
deterministic present on this currency-owning thread — the SH60 `present swap
Ok(0x1)` marker path is preserved.)

Workspace **506/0** (no test count change; example-only diagnostic binary).

## Honest scope

This closes SH60's cross-thread swap wall: task-driven frames now PRESENT genuinely
every time, on the correct thread. It does not yet change WHAT is rendered — the
present still drives the engine's clear-path frame-fn (0x105b32c00) with the
fabricated coherent renderer; it is not yet the engine detail-rendering its own
login/home UI (that needs a populated render-manager, SH60's standing frontier:
engine frame-plane driver 0x105b2ead4 with R+0x160/170/180 populated by game/UI
setup). And the drain's PENDING counter can grow large during the flood (bounded
only by what the presenter drains) — harmless (it is a u64 count, re-queued frames
are not stored), but a future cycle could coalesce or throttle the producer.

## Next frontier

From SH60/HANDOFF: bridge the w4=4 dispatch rate into the post-ctx window
(now largely done — the presenter keeps consuming into the post-ctx window), and
feed a real engine session producer. The engine's REAL frame driver is guest
0x105b2ead4 (scene renderer) which needs a populated render-manager (engine
game/UI setup, Lua/textures/login scene). That remains the standing structural
wall for "engine renders its own real screens".