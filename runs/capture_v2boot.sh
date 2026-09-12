#!/bin/bash
# SH54: reproducible artifact for --v2boot on the STABLE productized boot. The
# bare StartApp path is the documented pre-existing harness-bootstrap abort, so
# --v2boot is exercised on the SAME recipe that keeps the real client alive to
# exit 124 (full render chain + lifecycle + kicker + persist roundtrip). The
# v2boot thread sleeps V2BOOT_WARMUP_MS then drives the ordered ladder, dumping
# the type-4 producer vector [0x106829ea8] after EVERY rung — the SH53-open
# cross-module/runtime test of the recon's reframe.
set -u
cd "$(dirname "$0")/.."
LOG=/home/hermes-worker/runs/sh54-v2boot-ladder.txt
rm -f "$LOG"
timeout 45 env JIT_DRIVE_LIFECYCLE=1 RENDERINIT_WARMUP_MS=5000 V2BOOT_WARMUP_MS=4500 \
  ./target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 \
  --jni --startapp 0x258b144 --v2boot \
  --renderinit 0x105b3a280 --renderthunk --renderframe \
  --renderframe-drive --renderframe-seedgles --renderframe-drawprobe \
  --renderframe-triangle --renderframe-quad --renderframe-quad-loop 3 \
  --persist-roundtrip --kicker 0x106863af8 \
  > "$LOG" 2>&1
EXIT=$?
echo "EXIT=$EXIT"
echo "=== v2boot rungs (order + per-rung vector) ==="
grep -E "v2boot\] (driving|after|returned|stopped|ladder done)" "$LOG"
echo "=== json-overflow / string-length-overflow (the documented abort)? ==="
grep -iE "string length overflow|RBX::json" "$LOG" || echo "(none)"
echo "=== any non-zero [0x106829ea8] ==="
grep -oE "after .*: \[0x106829ea8\] = 0x[1-9a-f][0-9a-f]*" "$LOG" | head -3 || true
echo "=== final vector ==="
grep -E "ladder done" "$LOG"
echo "=== render/persist baseline (proves product still functional) ==="
grep -E "persist\] live datastore|triangle|quad-loop\] iter" "$LOG" | head -5
echo "=== abort/segv summary (count) ==="
grep -icE "SIGSEGV|SIGABRT" "$LOG"