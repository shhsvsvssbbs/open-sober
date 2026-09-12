# Frontier: SH35 — the engine's GLES3 pipeline dispatch slots (UBO / instanced / program-binary) now resolve through the bridge

`arm64jit::resolver::GLES_INT_NAME_LIST` gains the ten real-engine GLES3 pipeline
functions that the SH28 live slot snapshot showed the engine's own GL-init leaves
in its dispatch table (BSS `0x106d3b2f0 + 8*N`) as **raw-Mesa host addresses** —
the same SH19/SH24 crash class (a guest `br` through the `0x5b3a1c0 + 0xc*N` stub
jumps out-of-image). A real engine-driven frame that dispatches through these
slots would have crashed; now they resolve to host-thunk bridge slots.

## What changed (resolver only — no harness/none/GLES-codec change)

Five patches to `crates/arm64jit/src/resolver.rs` `GLES_INT_NAME_LIST` (all pure
integer/pointer ABI, ≤8 args — safe through the integer HostCall, and correctly
REJECTED by `resolve_gles_mixed`):

- `glBindBufferBase`, `glBindBufferRange`            (SH28 slot 5 / 6)
- `glUniformBlockBinding`, `glGetUniformBlockIndex`, `glGetActiveUniformBlockiv`
                                                    (slots 4 / 7 / 8)
- `glDrawElementsInstanced`, `glDrawArraysInstanced` (slots 9 / 10 at init)
- `glGetProgramBinary`, `glProgramBinary`, `glProgramParameteri`
                                                    (slots 13 / 14 / 15)

Because `w_eglGetProcAddress` (SH3's interception, which the engine's real GL-init
uses to build its dispatch table) checks `resolve_gles_int` after mixed, these names
now return OUR bridge slots when the engine resolves them — the engine's table is
auto-healed without any harness re-seed.

## Verification (real libroblox.so, Mesa llvmpipe + Xvfb)

`--renderframe-seedgles` PRE-SEED snapshot now shows every GLES3 pipeline slot as a
`0x7f000000…` bridge slot (was raw `0x7f44…` Mesa in the SH28 capture):

```
SH28 (before)                          SH35 (after)
slot 4 = Mesa glUniformBlockBinding    slot  4 = 0x7f0000003028  (bridge)
slot 5 = Mesa glBindBufferBase         slot  5 = 0x7f0000003030  (bridge)
slot 6 = Mesa glBindBufferRange        slot  6 = 0x7f0000003038  (bridge)
slot 7 = Mesa glGetUniformBlockIndex   slot  7 = 0x7f0000003040  (bridge)
slot 8 = Mesa glGetActiveUniformBlockiv slot 8 = 0x7f0000003048  (bridge)
slot 9 = Mesa glDrawElementsInstanced  slot  9 = 0x7f0000003050  (bridge)
slot10 = Mesa glDrawArraysInstanced    slot  10 = 0x7f0000003058 (bridge)
slot13 = Mesa glGetProgramBinary       slot  13 = 0x7f0000003060 (bridge)
slot14 = Mesa glProgramBinary          slot  14 = 0x7f0000003068 (bridge)
slot15 = Mesa glProgramParameteri      slot  15 = 0x7f0000003070 (bridge)
```

Render path unchanged (regression-gated): geometry wrapper `0x5b35288 Ok(0x0)`,
post-draw swap `Ok(0x1)`, 4×4 grid 16/16 cell-center readbacks all match their
texel color, exit 124 stable. Run-log: `runs/sh35-pipeline-slots.txt`.

## Baselines unchanged
- `--jni` boot returns `Ok(0x10006)` (JNI_OnLoad reached), then idles to exit 124.
- Single-quad / triangle / clear / compressed-texture levers intact.
- `cargo build --workspace` + `cargo test --workspace` = **477/0** (was 476/0).

## New regression
`gles3_pipeline_names_resolve_via_int_bridge_for_engine_draw_slots` pins all ten
names resolve through the int bridge (trailing NUL) and are rejected by mixed.

## Honest framing
Still harness-driven on a time base (the engine's own main-loop producer still
never enqueues a render task). But this closes the last raw-Mesa-dispatch gap in
the engine's OWN GLES3 render table: if/when a real frame dispatches UBO binding,
instanced draws, or program-binary uploads through its dispatch table, each routes
through our bridge instead of jumping out-of-image. This is the SH19/SH24 bug
class applied to the modern GLES3 pipeline SH28 identified as the real renderer.