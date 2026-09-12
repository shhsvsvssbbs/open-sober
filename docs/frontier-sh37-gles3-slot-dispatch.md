# Frontier: SH37 — the SH35-sealed GLES3 pipeline slots are FUNCTIONAL (dispached live)

SH35 shut the last raw-Mesa-dispatch-gap by adding the ten GLES3-pipeline function names
to `resolver::GLES_INT_NAME_LIST`, so the engine's OWN GLES dispatch table (BSS
`0x106d3b2f0 + 8*N`) auto-heals to bridge slots. But that only proved **resolvability**.
This cycle proves they actually **run** — dispatched through the engine's own slot stubs.

## The engine's real dispatch mechanism

The engine's frame clear/draw code `bl`s to per-slot stub trampolines, each a bare
`adrp x8,6d3b000; ldr x3,[x8,#752+8N]; br x3` — i.e. it loads slot N's function pointer
and `br`-dispatches through it. A guest `br` to stub `x` re-enters the JIT at the bridge
slot. These stubs are at GUEST vaddr `0x105b3a1c0 + 0xc*N` (file vaddr `0x5b3a1c0` plus
the `0x100000000` image base) — a `jit_run` into the guest stub is the faithful way to
exercise a sealed slot exactly as a real session would.

```
slot N  stub guest addr     slot N  stub guest addr
0       0x105b3a1c0        8       0x105b3a220
1       0x105b3a1cc        9       0x105b3a22c
2       0x105b3a1d8       10       0x105b3a238
3       0x105b3a1e4       11       0x105b3a244
4       0x105b3a1f0       12       0x105b3a250
5       0x105b3a1fc       13       0x105b3a25c
6       0x105b3a208       14       0x105b3a268
7       0x105b3a214       15       0x105b3a274
```

## New lever: `--renderframe-progbin`

Runs after the standard render sequence on the live Mesa-llvmpipe context (StartApp +
render-init + render-bind warm-up). Drives the sealed slots via `jit_run` into the stub
addresses, asserting `glGetError` stays NO_ERROR throughout:

- **slot15 glProgramParameteri**(prog, GL_PROGRAM_BINARY_RETRIEVABLE_HINT=0x8257, 1)
  before link — Mesa emits a retrievable binary only when the hint is set.
- **slot13 glGetProgramBinary**(prog, 4096, &len, &format, &bin) → **len=3498,
  format=0x875f** — a real Mesa program binary produced THROUGH the sealed slot. A
  mis-bridged slot would return len=0/format=0 or GL_INVALID_OPERATION.
- **slot14 glProgramBinary**(prog, 0x875f, bin, 3498) re-upload → accepted (err 0x0).
- **slot5 glBindBufferBase**(GL_UNIFORM_BUFFER=0x8A11, 0, real_gen_buffer) → err 0x0.
- **slot10 glDrawArraysInstanced**(GL_TRIANGLES, 0, 0, 3) — re-pointed to the sealed
  instanced fn for the probe, then restored → dispatches clean, err 0x0.

Full run (runs/sh37-progbin-full.txt, exit 124 stable) also shows the standard render
path intact in the same process: geometry wrapper `0x5b35288 Ok(0x0)`, triangle
`glDrawElements` drawn, textured-quad exact texel readbacks
(BL=RED/BR=GREEN/TR=WHITE/TL=BLUE), all swaps Ok(0x1).

## Bug found + fixed

The slot-stub constants were first written as the .so FILE vaddrs (`0x5b3a…`). `jit_run`
expects GUEST vaddrs → the first attempt failed with `pc 0x5b3a274 outside image
[0x100000000, …)`. Constants corrected to `0x105b3a…` (+0x100000000). The slot **table**
address (`0x106d3b2f0+8*N`) was always guest-correct (it lives in guest BSS).

## New hermetic regression

`sealed_gles3_ubo_and_instanced_slots_dispatch_real_mesa_clean` — makes a real
surfaceless ES3 context via `resolve_egl`, then drives `glBindBufferBase` (SLOT5) and
`glDrawArraysInstanced` (SLOT10) through `resolve_gles_int` with `blr`, asserting
`glGetError` == GL_NO_ERROR after each. A mis-bridged slot (wrong fn or ABI) would raise
GL_INVALID_ENUM/OPERATION or crash the test process. Gates softly (skips the functional
part, still asserts resolvability) if Mesa's surfaceless has no ES3 pbuffer config.

## Status

- `cargo build --workspace`: clean. `cargo test --workspace`: **479/0** (was 478/0).
- Baselines unchanged: `--jni` exit 0 (JNI_OnLoad), stable idle exit 124.
- Commits: `a0ba81c` (SH37), `8f57` (ledger).

## What this closes

The harness has now proved every GLES dispatch slot 0-15 is BOTH bridged (SH19/SH35) AND
functionally dispatchable on real Mesa (SH37), and that the engine's own geometry
wrapper/swap render real frames (solid, triangle, textured quad/grid, ETC1/ETC2/ASTC).
The remaining structural frontier is unchanged since SH14: the engine's own main-loop
producer still never enqueues a render task, so frames are harness-driven on a time base.