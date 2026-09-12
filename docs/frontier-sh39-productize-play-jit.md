# Frontier SH39 — Productize the proven JIT boot+render into `open-sober play --jit`

The SH15–SH38 frontier proved, through the **elfjit debug harness**, that the real
Roblox client boots headlessly and renders its OWN frames (real EGL context, engine
geometry wrapper, GLES bridge) onto a live Mesa-llvmpipe X11 window. But that
capability lived only in an example binary; the actual product entry point
(`open-sober play --jit`) still ran a crude `jit::run_elf_entry` stub that could not
bootstrap the real client at all. The two were disconnected.

This cycle **productizes** it: `open-sober play --apk <real-roblox.apk> --jit` now
reproduces the proven boot+render automatically, through the product command. The
client's own render pipeline (engine dispatch slots, geometry wrapper, swap) renders
real frames on the live engine context, with the guest-persistence root armed so a
future session's `/data` datastore lands on persistent host disk.

## What landed

`crates/sober-core/src/jitlaunch.rs` (new, ~190 LOC incl. unit tests):
- `invocation_proven(lib, frames)` builds the canonical elfjit recipe:
  `JNI_OnLoad(0x2173ff4) → StartApp(0x258b144) → render-init thunk (0x105b3a280) →
  renderthunk → renderframe(+drive+seedgles) → drawprobe → triangle → quad →
  quad-loop N → kicker(0x106863af8)`, with `JIT_DRIVE_LIFECYCLE=1` +
  `RENDERINIT_WARMUP_MS=5000`.
- `resolve_elfjit_bin()` finds/builds the `elfjit` example binary
  (`target/debug/examples/elfjit` — the same executable the `runs/capture_*.sh`
  harnesses drive), so the product stays in sync with the harness source.
- `launch_jit(lib)` spawns it against the extracted `libroblox.so`, inheriting
  stdio, and waits. elfjit self-contains: Xvfb bring-up, ANativeWindow→X11 XID
  wiring, and `SOBER_ANDROID_ROOT` arming (SH38/SH38b).

`crates/sober-core/src/main.rs`: the `--jit` branch now `apk::extract_libs` →
`qemu::find_main_binary` → `jitlaunch::launch_jit`. The superseded
`mod jit;` (`jit.rs` `run_elf_entry`) is removed — it was a pre-productization stub
that could not bootstrap the real client.

## Proof (real client through the product command)

`open-sober play --apk ~/.cache/open-sober/apks/roblox-android.apk --jit`
(runs/capture_jit_play.sh, log runs/sh39-play-jit.txt, exit 124 = timeout ⇒ stable
idle main loop after the render prove). The **extraction from the real APK** resolved
`librobblox.so` (so the whole chain — product command → APK → real binary → JIT →
engine render — is exercised), then:

```
running entry guest=0x102173ff4          (JNI_OnLoad)
JIT(no-QEMU) entry() -> 65542 (0x10006)  (JNI_OnLoad Ok)
driving StartApp @ guest 0x10258b144
[elfjit:anativewindow] wired real X11 window XID=0x200000 on :263
[elfjit:renderinit] returned Ok(...)     (REAL ctx 0x7fdba0001d90 recovered)
[elfjit:renderframe-triangle] readback: centroid(640,360)=RGBA(255,0,0,255)
[elfjit:renderframe-quad]     readback: BL=RED BR=GREEN TR=WHITE TL=BLUE  (exact texels)
quad-loop iter 0..5 drew+swap Ok(0x1) bg=[5 distinct cycling colors]  (6 fresh frames)
```

Triangle = real indexed `glDrawElements` through the bridge (centroid red); textured
quad = real interpolated-UV sampling (BL/BR/TR/TL exact texel colors); 6 fresh
textured frames prove sustainability. Workspace **484/0** (was 482/0; +2 jitlaunch
regression tests pin the proven recipe and the bounded frame count).

## Honest scope

The render is still **harness-driven** on the live engine context — the engine's own
main-loop producer still never enqueues a render task (SH14's standing structural
wall). What changed is *where* the harness is driven from: the actual product command
(`open-sober play --jit`), not a debug example. This is the product-integration step
the frontier docs named ("wire open-sober play --apk to reproduce the elfjit
boot+render automatically"), and it arms the persistence root so a real session's
datastore can persist once the boot reaches a session that reads/writes it.

## Next (closest unblocked)

With `play --jit` now the single product driver of the proven boot+render, the
producer/deque wall (get the engine's own main loop to enqueue a render task) is the
sole remaining block to a self-driven session; re-opening it means driving the ALooper
app-command lifecycle so a real producer posts work (SH14's flagged lever) rather than
more deque-node surgery.