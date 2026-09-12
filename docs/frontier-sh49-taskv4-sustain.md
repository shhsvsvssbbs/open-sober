# SH49 — the type-4 task-dispatch plane is now SUSTAINABLE: force-pop arming + block-cache eviction run exactly once

## Summary

SH44 proved the drain's type-4 popped-task dispatch plane (`[0x6829ea8]`) is
functional when its vector is populated, but every sustained attempt crashed
after ~39 injected nodes (SIGSEGV at guest `0x102856f7c`). This cycle fixes the
crash mechanism and demonstrates a **sustained** type-4 inject+pop+dispatch
loop: **197 consecutive deque node pops with zero SIGSEGV / abort / terminate,
stable idle exit 124** (SH44 faulted at pop #39). The fix is the precondition
the frontier has named for seeding the vector with a *real guest producer*:
once foreign nodes can be dispatched sustainably, a future seed of
`[0x6829ea8]` with a real frame/session producer has a stable loop to run in.

## Root cause (runs/sh44-taskv4-plane.txt)

The `--deque-node-live` injector loop, on *every* re-injection (i.e. after every
pop, ~every 50 ms), did BOTH:

1. re-patched the drain's force-pop sites `0x102856f4c` / `0x102856f7c`, and
2. re-dropped the **already-recompiled** drain blocks from the block cache
   (`block_cache_drop_region(0x102856e40, 0x1028570c0)`).

The patches are idempotent (`was d503201f -> d503201f` in the log — a NOP that
was already in place), and because the guest bytes at those sites are
permanently patched, *any* future recompile of the region already yields the
force-pop behavior. So the per-iteration cache drop was pure churn: it forced
the pop-loop's translated block to be recompiled while the drain was
**mid-execution** of that same block, and after ~39 churns the translation
desynced — the popped-node register loaded an instruction word
(`x22=0x7bfdd503233f`, low-32 `0xd503233f` = a NOP) and faulted.

## Fix (crates/arm64jit/examples/elfjit.rs, `--deque-node-live` injector)

A process-static `ARMED: AtomicBool` guards the force-pop patch + cache eviction
so they run **exactly once** (the first real placement). Subsequent
re-injections only `write_volatile` the fresh node into the head-cell — no code
patch, no eviction. This removes the recompile-while-running desync while
preserving the SH11 sequencing lever (stable drain during placement, force-pop
after the first placement). Because the guest bytes stay patched, correctness is
unaffected — only the pathological re-eviction is gone.

## Verification

- `runs/sh49-taskv4-sustain.txt`: full productized render recipe +
  `--taskv4-seed probe --deque-node-live 0x106829f00 --drain-poll 8`. **197**
  continuous `NODE ... POPPED` (vs SH44's crash at #39), 3 clean type-4
  vector dispatches through the real dispatcher w4=4 plane, `ARMED ...` logged
  exactly once, `dropped cached drain blocks ...` exactly once, **zero**
  SIGSEGV / abort / `string length overflow`, **exit 124** (stable, hit the
  timeout — a live idle loop, not a crash).
- `runs/sh49-product-reverify.txt`: the productized `open-sober play --apk
  roblox-android.apk --jit` path re-verified on the edited elfjit — exit 124,
  real indexed triangle (centroid RGBA(255,0,0,255)) + textured quad
  (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE) + quad-loop, every post-draw swap Ok(0x1),
  zero ENOSYS/unhandled. The product path never passes `--deque-node-live`, so
  the injector change cannot regress it; re-verified anyway.
- `cargo test --workspace`: **495 passed / 0 failed** (unchanged).
- `cargo build --workspace`: clean (only pre-existing warnings).

## Honest scope / frontier (unchanged structural wall)

This makes the injection *sustainable*, it does not yet drive a self-produced
frame. The 3 clean type-4 dispatches per run reflect the real dispatch
selection (only some injected nodes carry `w4=4` through the sentinel-derived
vtable), and the type-4 **producer vector** `[0x6829ea8]` is still populated by
the harness probe here, not by a real engine producer (SH46 proved no in-code
install site exists). Next per the standing frontier: feed `--deque-node-live` +
`--taskv4-seed 0x<guest-handler>` a real engine frame/session handler so a
sustainably-dispatched task node advances the engine toward its own frame,
now that the loop it runs in no longer self-destructs.