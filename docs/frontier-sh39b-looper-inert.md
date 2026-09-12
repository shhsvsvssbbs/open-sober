# SH39b — negative diagnostic: the ALooper app-command lever is INERT for this client

SH14 flagged "drive the ALooper app-command lifecycle so a real producer posts the work
item" as the next productive lever behind the engine's idle-main-loop wall. SH39 (and
SH38 before it) POST a full lifecycle sequence (APP_CMD_START=1, APP_CMD_RESUME=2,
APP_CMD_INIT_WINDOW=11) via `arm64jit::shims::post_app_command` under
`JIT_DRIVE_LIFECYCLE` — yet the engine still idles (exit 124), and render-init is only
ever reached by the harness's direct host-drive (`--renderinit` warm-up), never by the
guest's own main loop.

This probe pins WHY, and it is conclusive:

```
timeout 30 env JIT_DRIVE_LIFECYCLE=1 JIT_TRACE=1 \
  JIT_REGION_WATCH=0x102bcd5d0-0x102bcd650 \
  target/debug/examples/elfjit ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 \
  --jni --startapp 0x258b144 --kicker 0x106863af8
```

Run log runs/sh39-looper-probe.txt, exit 124 (stable idle). Results:

- `[alooper] ALooper_pollOnce` call count across the whole stable boot: **0**.
  The guest never invokes the ALooper shim (whose symbol is bound via
  `plt::bind_image_plt`), so the posted APP_CMD_* values sit in the host FIFO
  undrained forever.
- `JIT_REGION_WATCH` on the app-glue main loop (guest 0x102bcd5d0): **0 hits**.
  The GameActivity glue loop is never entered on this box.
- `ALooper_addFd` (which registers the command-pipe poll source): **0** — consistent
  with the guest never driving an app command/looper path at all.

## What that means for the frontier

The engine's *idle* is NOT the android_native_app_glue/GameActivity main loop that the
ALooper shim was built to feed — it is the **per-CPU task-deque consumer** (SH4–SH7,
SH14: generic futex wait at 0x10284d018, era-drain pop-loop dispatching `[vt+40]` with
the w4=4 type baked in). So "drive the ALooper app-command lifecycle" is a **dead-end
for this client** exactly as the deque-injection path was (SH14): neither host knob
makes the engine enqueue its own render work.

That closes the last of SH14's listed "productive levers." The single remaining wall to
a self-driven session is the framework **task-producer** enqueue into the per-CPU deque
(Document):

- the consumers park in the generic `Q'` wait on the circular intrusive deque (sentinel
  = the drain struct; `[struct+112]=vt=0x106829f00`, `[vt+40]`=0x10285371c),
- a real task node inserted there is dispatched by the maintenance handler
  0x10285371c, whose forward edge `blr`s through framework-owned BSS globals
  0x1068262e8/300/308 — all statically 0 on this box.

So the next productive direction is NOT more lifecycle/kick/futex poking. It is either
(a) reverse-engineering what those three BSS dispatch globals must point at (what a real
"maintenance" task's handler is), so a working node + seeded globals can advance the
engine into a real frame/session task, or (b) accepting the current architecture — the
harness (now productized as `open-sober play --jit`, SH39) drives the engine's OWN
context/geometry/swap — as the stable render path until a GPU host makes the framework
producer reachable.

## Honest scope

This is a pure negative/diagnostic result — no new capability. Its value is redirecting
effort: it disproves the last flagged "easy" lever and re-pins the exact wall (task-deque
producer + three empty BSS dispatch globals), so no future cycle burns time re-posting
app commands or re-investigating the looper. Workspace unchanged (484/0).