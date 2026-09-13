#!/bin/bash
# SH62: reproducible artifact — the ENGINE's OWN scene renderer (0x105b2ead4)
# constructs + registers a real frame-desc into a fabricated-but-engine-native
# render-manager R (its own operator-new 0x1d96768 / frame ctor 0x5b34de8 /
# linker 0x5b2d9e0), and that engine-built frame-desc is PRESENTED via the
# engine swap 0x105b3b408 on the recovered live EGL ctx.
#
# --renderscene drives guest 0x105b2ead4(R) on the currency-owning renderinit
# thread after RENDERCTX is recovered. The renderer reads R+0x160=ctx,
# R+0x170=view (W/H), R+0x180/0x188=scene list, binds ctx, and constructs a
# 0x98-byte frame-desc (vtable 0x106731b00) EVEN with an empty scene array.
# Our view W/H is set to a sentinel != the live surface dims so the engine
# takes the BUILD branch (== the surface size would make it skip as
# "already built").
set -u
cd "$(dirname "$0")/.."
LOG=/home/hermes-worker/runs/sh62-renderscene.txt
rm -f "$LOG"
timeout 60 env JIT_DRIVE_LIFECYCLE=1 RENDERINIT_WARMUP_MS=1000 \
  ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 \
  --jni --startapp 0x258b144 \
  --renderinit 0x105b3a280 --renderthunk --renderframe --renderscene \
  --deque-node-live 0x106829f00 --drain-poll 8 \
  --persist-roundtrip --kicker 0x106863af8 \
  > "$LOG" 2>&1
EXIT=$?
echo "EXIT=$EXIT"
echo "=== engine scene renderer constructed+registered a real frame-desc ==="
grep -E "renderscene\] frame #0" "$LOG"
echo "=== presented engine-built frames ==="
grep -E "renderscene\] present #" "$LOG"
echo "=== present count ==="
grep -cE "renderscene\] present #" "$LOG"
echo "=== swap-results (must be all Ok(0x1)) ==="
grep -oE "renderscene\] present #[0-9]+ swap Ok\(0x1\)" "$LOG" | wc -l
echo "=== crash/json-overflow ==="
grep -icE "SIGSEGV|SIGABRT|string length overflow" "$LOG"