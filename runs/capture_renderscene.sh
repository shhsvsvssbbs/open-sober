#!/bin/bash
# SH63: reproducible artifact — the ENGINE's OWN scene renderer (0x105b2ead4)
# now walks a POPULATED scene list: 0x28-stride scene nodes laid into R+0x180
# (head) / R+0x188 (tail) each carry a render-obj (+0x08) + view (+0x18), and
# the engine builds ONE real 0x98 frame-desc PER NODE (per-node loop at file
# 0x5b2eb9c, linking each at container node+0x18 via 0x5b2d9e0) — in addition
# to the base frame at R+0x170. This is the per-node engine-detail frame plane
# SH62's empty-scene proof left as the next frontier.
#
# RENDERSCENE_NODES=N lays N 0x28-stride nodes into R (default 3 in the live
# run; 0 = the legacy empty-scene fast path, still verified below).
set -u
cd "$(dirname "$0")/.."
LOG=/home/hermes-worker/runs/sh63-renderscene-populated.txt
rm -f "$LOG"
timeout 70 env JIT_DRIVE_LIFECYCLE=1 RENDERINIT_WARMUP_MS=1000 RENDERSCENE_NODES=3 \
  RENDERSCENE_MAX_FRAMES=2 RENDERSCENE_WINDOW_MS=2000 \
  ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 \
  --jni --startapp 0x258b144 \
  --renderinit 0x105b3a280 --renderthunk --renderframe --renderscene \
  --deque-node-live 0x106829f00 --drain-poll 8 \
  --persist-roundtrip --kicker 0x106863af8 \
  > "$LOG" 2>&1
EXIT=$?
echo "EXIT=$EXIT"
echo "=== engine scene renderer built base + per-node frames ===
      (expect per_node_frames=[3 guest ptrs] per_node_ok=true scene_nodes=3)"
grep -E "renderscene\] frame #0" "$LOG"
echo "=== presented (all swap Ok(0x1)) ==="
grep -oE "renderscene\] present #[0-9]+ swap Ok\(0x1\)" "$LOG" | wc -l
echo "=== persist roundtrip byte-exact ==="
grep -E "persist\] live datastore roundtrip" "$LOG"
echo "=== crash/json-overflow (must be 0) ==="
grep -icE "SIGSEGV|SIGABRT|string length overflow" "$LOG"