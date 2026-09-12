# Open-Sober Status — Ongoing Autonomous Development

## SH35 (Sep 12, 2026): the engine's REAL GLES3 dispatch-slot table is no longer raw-Mesa — UBO / instanced / program-binary slots now resolve through the JIT bridge. Workspace 477/0 (was 476/0).

SH28's live snapshot showed the engine's own GL-init fills GLES dispatch-table
slots 4-8 (glUniformBlockBinding / glBindBufferBase / glBindBufferRange /
glGetUniformBlockIndex / glGetActiveUniformBlockiv), 9/10
(glDrawElementsInstanced / glDrawArraysInstanced) and 13-15 (glGetProgramBinary /
glProgramBinary / glProgramParameteri) with **raw-Mesa addresses** (the SH19/SH24
crash class). Added all ten to `resolver::GLES_INT_NAME_LIST` (pure int/ptr ABI).
Since the engine builds its table via `eglGetProcAddress` (SH3 → resolve_gles_int),
the table now auto-heals to bridge slots — verified live (runs/sh35-pipeline-slots.txt):
PRE-SEED snapshot shows all ten as `0x7f000000…` bridges (was raw Mesa). Render path
unchanged (wrapper Ok(0x0), swap Ok(0x1), 4×4 grid 16/16 readbacks, exit 124).
New regression `gles3_pipeline_names_resolve_via_int_bridge_for_engine_draw_slots`.
Doc docs/frontier-sh35-gles3-pipeline-slots.md. Commit 6a49574.

## SH34 (Sep 12, 2026): the coherent renderer scales to a REAL LARGER MESH. New `--renderframe-grid <N>` fabricates an N×N grid of textured quads (independent per-cell, each a distinct texel color at the interpolated vertex UV) driven through the REAL libroblox.so's OWN geometry wrapper 0x5b35288. Verified N=3 (9/9), N=4 (16/16), N=6 (36/36) cell-center glReadPixels readbacks ALL match each cell's exact texel color (±1): 6×6 = 144 verts / 216 idx in one call through engine primitive-setup + indexed glDrawElements, wrapper Ok(0x0), swap Ok(0x1), exit 124. Sustainable (quad-loop 20 iters all Ok(0x1)). Captures runs/sh34-grid.{txt,mp4}; capture_grid.sh. Fixed grid/VBO/tex buffer-overlap bugs (relocated to 0x2000/0x4000/0x6000). Harness-only; single-quad mode (4 distinct checkerboard readbacks) + --jni baseline + baselines unchanged. Workspace 476/0. Doc docs/frontier-sh34-grid.md.

## SH33 (Sep 12, 2026): SUSTAINABLE TEXTURED real-geometry rendering — `--renderframe-quad-loop <N>` re-drives clear(cycling bg) -> engine geometry wrapper 0x5b35288 -> swap N times on the detached host thread AFTER the single textured-quad proof frame. Verified 6 iterations all drew+swap Ok(0x1) with 5 distinct cycling backgrounds (red/green/blue/yellow/magenta) and the textured readback intact in the same run (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE): a recording proves a fresh textured render every frame. 8-frame x11grab at runs/sh33-quad-loop.mp4. This is the textured/mesh analog of SH25b's triangle-loop — closes the last "sustainable" property for the textured path a real main-loop frame drive needs (textured recipe now both renders correctly AND sustains). Doc docs/frontier-sh33-quad-loop.md; reproducible runs/capture_quad_loop.sh. Harness-only (no codec/resolver change): workspace 476/0; baselines unchanged.

## SH32 (Sep 12, 2026): the engine's OWN geometry path renders a REAL ASTC texture (`--renderframe-astc`, GL_COMPRESSED_RGBA8_ASTC_4x4=0x93B0 — the load-bearing Android format desktop GL can't native-decode, so our interception is REQUIRED). 8x8 = 4 x 16-byte Khronos LDR void-extent blocks (buf[0]=0xFC, bit8, bit9=0 Dynamic Range; color = UNORM16 at bytes 8/10/12/14, high-byte = 8-bit channel). Uploaded via glCompressedTexImage2D; bridge decodes via decode_astc. FS maps DECODED ALPHA to RGB gray-scale: readback BL=RGBA(255,255,255,255) BR=190 TR=64 TL=128 = the 4 exact ASTC block alphas. Captured runs/sh32-astc.{rgb,png} = 4 distinct gray quadrants on clear-blue, exit 124. New regression astc_ldr_void_extent_blocks_decode_expected_color_and_alpha. Doc docs/frontier-sh32-astc.md. Compressed live-prove now ETC1+ETC2-RGB+ETC2-RGBA8/EAC+ASTC. Workspace 476/0; baselines unchanged.

## SH31 (Sep 12, 2026): the engine's OWN geometry path renders a REAL ETC2-RGBA8/EAC texture (`--renderframe-etc2a`, GL_COMPRESSED_RGBA8_ETC2_EAC=0x9278 — the real Android RGBA-EAC format). The 8x8 texture = 4 x 16-byte blocks (EAC-alpha sub-block + the SH29-proven ETC2-RGB sub-block) uploaded via glCompressedTexImage2D; the bridge decompresses via decode_etc2_rgba8. FS maps the DECODED ALPHA to RGB gray-scale, so readback proves the EAC alpha sub-block decodes live: BL=RGBA(255,255,255,255) BR=RGBA(190,190,190,255) TR=RGBA(64,64,64,255) TL=RGBA(128,128,128,255). Captured runs/sh31-etc2a.{rgb,png} = 4 distinct gray quadrants on clear-blue, exit 124. New regression etc2_rgba8_eac_solid_blocks_decode_expected_alpha. Doc docs/frontier-sh31-etc2a.md. Compressed-texture live-prove coverage now ETC1+ETC2-RGB+ETC2-RGBA8/EAC. Workspace 475/0; baselines unchanged.

## SH30 (Sep 12, 2026): REAL TWO-ATTRIB TEXTURED QUAD renders through the engine's OWN geometry wrapper (`--renderframe-quad`). primitive-setup's multi-primitive loop sets up TWO vertex attribs (aPos @format[3]{4,GL_FLOAT} off0, aUV @format[1]{2,GL_FLOAT} off16) on an interleaved [pos.xyzw, uv.xy]x4 VBO (stride 24, 6-idx EBO). FS samples a 2x2 checkerboard at the REAL interpolated vertex UV -> BL=RED/BR=GREEN/TR=WHITE/TL=BLUE. Captured runs/sh30-quad.{rgb,png} = 4 near-equal color quadrants, exit 124. Doc docs/frontier-sh30-quad.md. Workspace 474/0; baselines unchanged.

## SH29 (Sep 12, 2026): the engine's OWN geometry wrapper renders a REAL ETC2 texture (`--renderframe-etc2`, GL_COMPRESSED_RGB8_ETC2=0x9274 — the actual Android Roblox format). Same hand-crafted blocks as SH27 relabeled ETC2; bridge decodes via decode_etc2_rgb and re-uploads. Readback pixel-identical to ETC1 (centroid WHITE / GREEN / RED distinct; capture runs/sh29-etc2.{rgb,png} = 4-colored triangle interior). Regression extended to assert ETC2 decode equals ETC1. Workspace 474/0; baselines unchanged.

## SH28 (Sep 12, 2026): captured the real engine GLES dispatch-table content live. --renderframe-seedgles dumps all 16 raw slots (BSS 0x106d3b2f0+8*N) before seeding and dladdr-resolves each. The engine's REAL renderer dispatch is a modern GLES3 pipeline (slots 4-8 = UBO/buffer, 9/10 = instanced draws, 13-15 = program binary), NOT the simple clear/draw map. Slots 0-2 read as our bridge slots, confirming SH3's eglGetProcAddress interception reaches the engine's own table. Diagnostic-only. Workspace 474/0; baselines unchanged.

## SH27 (Sep 12, 2026): the engine's OWN geometry wrapper renders a REAL COMPRESSED-ETC1 texture. `--renderframe-etc` uploads a hand-crafted 8x8 ETC1 texture (4 solid blocks) via glCompressedTexImage2D (GL_ETC1_RGB8_OES); the GLES bridge decompresses ETC1->RGBA (texture-codec) and re-uploads. Three on-triangle probes read back distinct decoded colors (WHITE/GREEN/RED), and the rounded channels match the hand-computed (c*0x11)+2 prediction exactly, proving the ETC1 decode ran. Captured runs/sh27-etc.{png,rgb}: triangle interior 4-colored on clear-blue, exit 124. New regression `crafted_etc1_solid_blocks_decode_to_expected_colors` (texture-codec). Workspace 474/0 (was 473). Baselines unchanged (--jni exit 0, idle 124; --renderframe-tex intact).

## SH26 (Sep 12, 2026): the engine's OWN geometry wrapper now renders a REAL TEXTURED triangle — a 2x2 RGBA checkerboard sampled by a textured fragment shader, with every texture/uniform/shader call (glGenTextures/glBindTexture/glActiveTexture/glTexImage2D/glTexParameteri/glGetUniformLocation/glUniform1i) dispatching through the JIT GLES bridge. Workspace 473/0; HEAD (this commit).

Follows SH25's solid-red triangle. New `--renderframe-tex` lever: textured FS
(`precision mediump float;` REQUIRED in GLSL ES 1.00 for a local `vec2` — without
it Mesa errors "No precision specified ... for type 'vec2'") samples a 2x2 RGBA
checkerboard via a UV derived from `gl_FragCoord`. Three on-triangle quadrant
probes read back three DIFFERENT colors (impossible for a constant shader):

```
compile_status vs=0x1 fs=0x1 link_status=0x1 ; uTex loc=0x0<-unit0
readback centroid(WHITE)   @(640,360) = RGBA(255,255,255,255)
readback quad-(1,0)(GREEN) @(900,150) = RGBA(0,255,0,255)
readback quad-(0,0)(RED)   @(300,150) = RGBA(255,0,0,255)
geometry wrapper Ok(0x0) ; post-draw swap Ok(0x1) ; exit 124
```

Captured frame (runs/sh26-tex.{rgb,png}): clear-blue bg + the triangle interior
4-colored (RED=155,909 / GREEN=155,899 / WHITE=52,001 / BLUE=51,947 ≈ 45.1% of the
frame — the SH25 footprint now textured). glTexImage2D is a 9-arg form where
pixels rides the guest stack ([sp+0]); the PLT stub is a leaf (never pushes sp), so
a fake sp whose [0] holds the pixels ptr is read by the bridge's gs_stack. New
debug aid: failing shaders dump their info log via the int bridge
(glGetShaderInfoLog/glGetProgramInfoLog via resolve_gles_int). Reproducible:
runs/capture_tex.sh; run-log runs/sh26-tex.txt. Doc:
docs/frontier-sh26-tex.md.

**Next:** pin the engine's GLES dispatch-table slot 11+ texture/uniform/shader
mapping (disasm the engine's texture-binding path so a textured engine-driven draw
routes through the slots, not harness @plt), then scale the coherent renderer to a
two-attrib (pos+UV) real mesh. ETC2/ASTC interception is already in the
bridge/texture-codec — needs a live-path prove. Baselines unchanged: --jni exit 0
(0x10006); stable idle exit 124; untextured triangle still solid-red. Workspace 473/0.

## SH25 (Sep 12, 2026): the coherent renderer renders a REAL visible triangle through the engine's OWN geometry wrapper. Workspace 473/0; HEAD 035ff6a.

Follows SH24's draw-probe (empty prim list → dispatch-only proof). SH25 feeds
primitive-setup 0x5b353d0 a **coherent** renderer + REAL GL resources through
the JIT GLES int bridge (compiled+linked shader program with aPos→gl_Position
VS, solid-red FS; real VBO 3×vec4 ±0.95 NDC; real EBO 0,1,2). Driving the
engine's own geometry wrapper 0x5b35288 dispatches a real indexed
glDrawElements(GL_TRIANGLES,3,GL_UNSIGNED_INT) that RENDERS 415,696 red px =
45.11% of frame (clean triangle shape). Readback centroid RGBA(255,0,0,255);
wrapper Ok(0x0); swap Ok(0x1); exit 124.

Root cause of the SH24-observed "wrapper collapse": the fabricated primitive's
format-table index [prim+8]=5 selects format[5]={size4, **GL_SHORT**}; the
engine's glVertexAttribPointer misread float verts as shorts → degenerate.
Fix fmt_index=3 = format[3]={size4, **GL_FLOAT**=0x1406}. Also fixed the wrapper
count register (rides in the 4th drive arg w20, not x5) + added
glViewport/glScissor + dedicated glGenBuffers id slots.

Full coherent-renderer reverse in docs/frontier-sh25-triangle.md. Reproducible:
runs/capture_triangle.sh; run-log runs/sh25-triangle.txt.

Next (closest unblocked): sustainable real-geometry rendering (loop of bind →
clear → coherent draw → swap on the detached host thread, mirroring
--rendersustain), then GLES slots 11+ (texture/uniform/shader) + ETC2/ASTC
texture interception. Baselines unchanged: --jni exit 0; idle exit 124.

---

## SH13 (Sep 12, 2026): REAL engine vtable dispatch — the engine's native task-processor runs our injected nodes (~124 pops, exit 124, zero crash); block-cache grows past probe baseline

Work on the `dev` branch (HEAD 1a0ffbb+), workspace 469/0. SH12
confirmed+sustained type-4 dispatch through OUR host-thunk probe. SH13 steps it
forward: instead of the probe, inject with the REAL sentinel vtable
(`--deque-node-live 0x106829f00`, whose `[vt+40]=0x10285371c` is the engine's
own drain-node task-processor).

- **Mechanically verified + stable:** the engine's native dispatch machinery now
  consumes our injected foreign nodes — ~124 pops in 16s, process stable to
  harness timeout (exit 124), zero crash (a faulting dispatcher would give exit
  134). The run has NO probe logging, i.e. the dispatch goes through the engine's
  real processor.
- **Block-cache grows past probe:** `JIT_STATS=1` shows ~2147 compiles /
  7,361,652 hits vs the probe's flat ~434 — the obfuscated dispatch table
  (`0x102853a04..9b4`) keeps compiling+running real engine regions the host-thunk
  probe never touched.
- **Not yet render:** the real processor type-dispatches on `w4` (injected nodes
  always get `w4=4`, task-maintenance) through an obfuscated hash table; hostcall
  histogram is still syscall + pthread/JNI/mem, **zero egl*/gl***. The next
  lever: construct a node whose `[node+32]`/dispatch-index reaches a render/tick
  handler in that table, or give the maintenance path real framework state.

### Repro (reproducible, headless)
```bash
cargo build -p arm64jit --example elfjit
JIT_DRIVE_LIFECYCLE=1 timeout 16 ./target/debug/examples/elfjit \
  ~/.cache/open-sober/robbox/libroblox.so 0x2173ff4 --jni --startapp 0x258b144 \
  --kicker 0x106863af8 --drain-poll 100 --deque-node-live 0x106829f00
# expect: repeated "NODE 0x1073.. POPPED by live drainer", NO probe logging, exit 124
```
Run-log: `/home/hermes-worker/runs/sh13-realvt-runlog.txt`.
Doc: `docs/frontier-sh13-realvt-dispatch.md`. Baselines unchanged:
`--jni` clean exit 0; stable idle exit 124.

## SH12 (Sep 12, 2026): type-4 dispatch CONFIRMED + SUSTAINED through the real engine idle drain — foreign nodes now pop AND dispatch continuously (107 dispatches/104 pops across 105 node addrs in 14s, exit 124, zero crashes)

Work on the `dev` branch (HEAD 3b37deb), workspace 469/0. The SH11 residual
(object) is closed:

- **Root cause: vtable handler offset off-by-one.** Both probe vtable builders
  wrote `[vt+40]` at `add(4)` = byte 0x20 instead of `add(5)` = byte 40. The
  drain does `ldr [vt,#40]`, read 0, guard failed, node consumed WITHOUT
  dispatch. Fix `a2448fc`: `add(5)` in both sites.
- **Verified:** the real idle drainer pops our injected guest-arena node and
  type-4 dispatches it through our host-thunk handler with the exact engine ABI
  (`x0(vt+16)=0xdeadbeef`, `x3(node)=<our node>`, `w4=4`, `x5=0`).
- **Sustained (`3b37deb`):** re-inject a fresh node per pop → continuous stream.

Run-logs: `/home/hermes-worker/runs/sh12-probe-runlog.txt`,
`/home/hermes-worker/runs/sh12-sustain-runlog.txt`.
Doc: `docs/frontier-sh12-dispatch-confirmed.md`.