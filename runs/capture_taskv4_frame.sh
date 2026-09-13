#!/bin/bash
# SH60: reproducible artifact for the SELF-DRIVEN task frame plane
# (recon-selfdrive-seed-jsonfix.md §A). --taskv4-seed frame registers the
# type4_frame_thunk host-thunk into the dispatcher's type-4 vector
# [0x106829ea8]; a w4=4 dispatch marshals into a REAL presented frame (engine
# make-current 0x105b3b358 -> frame-fn 0x105b32c00 -> swap 0x105b3b408 on the
# recovered real ctx, vtable 0x106731ae0). Engine task-driving is sustained by
# (a) --deque-node-live real node pops (w4=4 through the drain's real pop-loop)
# and (b) patching the drain's two idle-heartbeat `mov w4,#2/#3` to `mov w4,#4`
# so EVERY idle drain dispatch routes the real dispatcher 0x10285371c to the
# seeded vector. A deterministic post-ctx w4=4 dispatch on the --renderthunk
# thread (EGL context already current) yields the clean `present swap Ok(0x1)`.
# RENDERCTX self-guard no-ops pre-recovery dispatches.
set -u
cd "$(dirname "$0")/.."
LOG=/home/hermes-worker/runs/sh60-taskv4-frame.txt
rm -f "$LOG"
timeout 50 env JIT_DRIVE_LIFECYCLE=1 RENDERINIT_WARMUP_MS=1000 \
  ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 \
  --jni --startapp 0x258b144 \
  --renderinit 0x105b3a280 --renderthunk --renderframe \
  --taskv4-seed frame \
  --deque-node-live 0x106829f00 --drain-poll 8 \
  --persist-roundtrip --kicker 0x106863af8 \
  > "$LOG" 2>&1
EXIT=$?
echo "EXIT=$EXIT"
echo "=== seed + heartbeat patch + ctx recovery ==="
grep -E "taskv4\] (type4_frame_thunk|seeded)|patched heartbeat|renderthunk\] published" "$LOG"
echo "=== TASK-DRIVEN FRAME PRESENTED (the deliverable marker) ==="
grep -E "taskv4-frame\] present #" "$LOG" | tail -5
echo "=== present count ==="
grep -cE "taskv4-frame\] present #" "$LOG"
echo "=== total w4=4 dispatches reaching the thunk ==="
grep -oE "task #[0-9]+" "$LOG" | tail -1
echo "=== node pops (real task-driven drain activity) ==="
grep -cE "NODE .* POPPED" "$LOG"
echo "=== json-overflow / crash summary ==="
grep -iE "string length overflow|RBX::json" "$LOG" || echo "(no json abort)"
grep -icE "SIGSEGV|SIGABRT" "$LOG"