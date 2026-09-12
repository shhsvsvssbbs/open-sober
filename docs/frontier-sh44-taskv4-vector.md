# SH44 — The task-deque "maintenance wall" precisely collapsed: the type-4 task-dispatch plane is FUNCTIONAL when its vector is populated

## Summary

The ~30-cycle "engine never self-produces a render task" wall has a precise,
now-empirically-proven mechanism: the engine's per-CPU task-deque drain has a
**type-4 popped-task dispatch vector** at guest BSS `0x6829EA8` that is **0 on
any headless boot** (including mid-render), and is only ever populated by a
real *framework* task-producer that is absent headlessly. When we **seed that
vector with a live handler**, injected task nodes dispatch through the engine's
REAL dispatcher to it with the exact ABI we predicted from disassembly. The
wall is therefore NOT an unreachable dispatch plane — it is simply that
*nothing installs the type-4 producer vector* on this box.

This is SH13's "next lever: identify a real render/tick vtable" taken the rest
of the way to a mechanical, observable answer: the dispatch plane works; the
missing-installed-vector is the sole reason no foreign/task node can drive the
engine toward a frame. The remaining work (SH45+) is to reverse what the real
framework installs into `0x6829EA8` and what task node/arg reaches a
frame/session producer.

## The drain dispatch contract (fresh disassembly, file vaddrs)

`drain` = 0x2856e40, dispatcher `[vt+40]` = 0x10285371c. Four dispatch sites,
all routing through `[node+112]&~0x3f -> [vt+40]` with a **hardcoded w4 type**:

| site | w4 | source | what the handler runs |
|------|-----|--------|-----------------------|
| 0x2856f24 | 2 | `[x19+104]` sentinel (heartbeat) | globals `0x68262E8` (`0x10620db24`) + `0x6826300` (`0x102176bfc`) |
| 0x2856f68 | 3 | sentinel (heartbeat) | global `0x6826308` (`0x1022199e0`) |
| 0x2856ffc / 0x285703c | 4 | popped node `[x22]` | **global `0x6829EA8` — the type-4 task vector** (`adrp 6829000; ldr x3,[x8,#3752]; br x3`, file 0x2853788/0x28537b8) |

`0x6829EA8` is real `.bss` (readelf section 29, addr 0x6829e80) — zero-initialized,
no static reloc, runtime-written. The four *populated* maintenance vectors at
`0x68262E8/0x6826300/0x6826308/0x6826320` are all thin telemetry/atrace event
emitters: each does `adrp x,67d1000; ldr x,[x,#1776]` (the render-ctx/monitor
global) then `mov w1,#<eventid>; bl 1e0b0a8` (trace-enter helper) — event ids
1/4/5/8. **None touches a render/session producer.**

## Empirical proof (runs/sh44-taskv4-plane.txt)

New `--taskv4-seed <probe|guest-hex>` elfjit lever populates `0x6829EA8`.
With `--taskv4-seed probe --deque-node-live 0x106829f00 --drain-poll 8`, the
drain pops 39 injected task nodes (`NODE ... POPPED`, re-injecting each time)
and **3 clean dispatches reach the seeded host-thunk handler** through the real
dispatcher w4=4 plane:

```
[elfjit:taskv4] seeded dispatcher type-4 vector [0x106829ea8] = 0x7f00000001b8
[elfjit:taskv4] type-4 task handler #1: fnarg0(x0)=0x107334000 arg1=0 arg2=0x7f32829a48c0 node=0x7f00000001b8 w4=4 x5=0
[elfjit:taskv4] type-4 task handler #2: ... #3: ...
```

ABI confirmed exactly as disassembled — `handler(node=x0, [node+32]&~1=x1,
consumer=x2)` with `w4=4, x5=0`. (A subsequent SIGSEGV at `guestpc=0x102856f7c`
is the known force-pop + re-inject + patched-drain-recompile race, SH11/SH13 —
the 3 clean firings are the conclusive signal.)

## Diagnostic improvements

- `JIT_FRAMEWORK_DUMP` now also dumps the **task-v4 vector** `[0x106829ea8]` and
  the backup **v0/2 vector** `[0x106826320]` at both sampler sites. Fresh dumps:
  `task-v4 [0x106829ea8]=0x0 v0/2 [0x106826320]=0x10620dddc` — task-v4 stays 0
  even mid-render; the maintenance vectors are populated.
- New `--taskv4-seed` harness lever (documents the type-4 plane + lets a future
  cycle point it at a real guest producer once reversed).

## Frontier (next)

Reverse what the real Android framework installs into `0x6829EA8` and what task
node type/arg (`[node+32]&~1`, the arg1 the dispatcher passes as x1) selects a
real frame/session producer — then seed the vector with that guest handler so a
task node drives the engine's OWN render loop natively. Also still open: the
bare-StartApp `RBX::json::Writer string length overflow` abort (a harness LSM
host-pointer seed leaking into a string-length read on the no-render bootstrap
path; the productized `play --jit` recipe already bypasses it).