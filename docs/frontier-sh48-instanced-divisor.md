# SH48 — the engine's instanced-mesh pipeline is no longer NULL-bound: `glVertexAttribDivisor` joins the GLES int bridge

## Summary

A **real, self-driven engine frame** that draws an instanced mesh would, in the
middle of its per-instance setup, call `glVertexAttribDivisor(index, n>0)` to mark
the attribute that varies per instance. That name was **absent from
`GLES_INT_NAME_LIST`** (only the two *draw* functions — `glDrawArraysInstanced` /
`glDrawElementsInstanced` — were whitelisted in SH35). So a guest `br` through the
engine's slot resolved to the NULL/0 stub, and every instance read instance 0's
data — the instanced draw silently degenerated to a single duplicated triangle
(no crash, just wrong output). This closes that gap: `glVertexAttribDivisor` now
resolves through the integer GLES bridge (real Mesa), and the functional SH37 gate
is extended from a count=0 no-op draw probe to a **real non-empty instanced draw**
(count=1, 4 instances) with a bound vertex buffer + divisor 1, asserting
GL_NO_ERROR throughout.

## What changed (crates/arm64jit/src/resolver.rs)

- `GLES_INT_NAME_LIST` gains `glVertexAttribDivisor` (pure integer/pointer ABI,
  2 args ≤ 8 — safe through the integer HostCall; rejected by the float/mixed
  wrapper). Because the engine builds its render dispatch table via
  `eglGetProcAddress` → `resolve_gles_int` (SH3 interception), its table
  **auto-heals** — no harness re-seed needed; the whitelist entry is sufficient
  for a self-driven engine frame.
- `sealed_gles3_ubo_and_instanced_slots_dispatch_real_mesa_clean` (SH37's
  functional gate) is extended: after the existing count=0 instanced dispatch
  probe, it now (1) gen+bind a real vertex buffer to attrib 0 through the JIT
  bridge, (2) `glEnableVertexAttribArray` + `glVertexAttribPointer` at the format
  table, (3) `glVertexAttribDivisor(0, 1)`, and (4) issues a **non-empty**
  `glDrawArraysInstanced(GL_TRIANGLES, 0, 1, 4)` through the sealed slot —
  asserting GL_NO_ERROR after the divisor set AND after the real instanced draw.
- New focused regression
  `gl_vertex_attrib_divisor_resolves_via_int_bridge_only_for_instancing`: pins
  that `glVertexAttribDivisor` (with its setup companions `glVertexAttribPointer`
  + `glEnableVertexAttribArray`) resolve via the int bridge with a trailing NUL,
  and are rejected by the mixed wrapper.

## Verification

- `cargo test --workspace`: **495 passed / 0 failed** (was 494/0; +1 regression).
- `cargo build --workspace`: clean.
- Productized real-boot baseline re-verified through the modified resolver:
  `open-sober play --jit` recipe → StartApp → render-init → engine's OWN geometry
  path — real indexed triangle + textured quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE)
  + 3 fresh quad-loop frames, every post-draw swap Ok(0x1), exit 124 stable. The
  whitelist change did not regress the proven render path.

## Why this matters (objective-facing)

SH35 sealed the two instanced *draw* slots, but a real instanced mesh does not
draw correctly with only the draw functions bridged — it needs the per-instance
attribute divisor or every instance collapses onto instance 0. This cycle makes
the *entire* instanced path (attribute setup + divisor + draw) resolve through the
bridge for a self-driven engine frame, closing the last NULL-dispatch surface of
the modern GLES3 instanced pipeline (heavy in real Roblox meshes). The standing
structural wall is unchanged (SH14/SH46): the engine still never self-produces a
session/render task — its framework task-producer vector `[0x6829ea8]` is populated
only by real Android framework glue absent headlessly — so frames remain
harness-driven on the live engine context.