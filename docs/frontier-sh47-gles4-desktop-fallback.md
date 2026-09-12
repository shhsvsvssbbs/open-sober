# SH47 — closed the last NULL-dispatch gap in the engine's GLES render table: GL4/extension slots (11/12) now resolve via desktop-libGL fallback

## Summary

The engine's real render dispatch table (BSS `0x106d3b2f0 + 8*N`, built via
`eglGetProcAddress` / SH3 interception) had **slots 11 and 12 reading 0x0**,
yet the engine's own render code `bl`s those two slots **unguarded** (slot-11
stub `0x5b3a244` is `bl`'d twice; slot-12 `0x5b3a250` four times). A
self-driven frame dispatching through them would `br` to NULL and SIGSEGV —
the last uncovered NULL-dispatch crash surface in the engine's GLES table
(SH19/SH24/SH35 bug class, applied to the slots those cycles never reached).

Root cause was **not** a missing whitelist entry or a float-ABI rejection: it
was that the requested names are **GL4 / extension functions**
(`glBufferStorage`, `glMapBuffer`, `glQueryCounter`, `glObjectLabelKHR`,
push/pop-group-marker, query-object-`ui64v`) that Mesa's **ES-only
`libGLESv2.so.2` does not export**, so both our int bridge and the Mesa GLESv2
fallback inside `w_eglGetProcAddress` returned 0 → the engine's slot stayed NULL.

## Evidence (new `JIT_EGL_LOG` diagnostics + live boot)

Added an env-gated diagnostic (`JIT_EGL_LOG=1`) in `w_eglGetProcAddress` that
logs every requested-name that fails all bridges and Mesa GLESv2 (returns 0),
with the guest PC so we can map it to the 0x5b3a1c0+0xc*N slot stubs.

A full productized boot (`JIT_DRIVE_LIFECYCLE` + render-init + triangle + quad
+ quad-loop) with `JIT_EGL_LOG=1` showed the engine requests exactly **16
GL4/extension names** that came back unresolved:

```
glQueryCounter/EXT  glPushGroupMarker/EXT  glPopGroupMarker/EXT
glObjectLabelKHR    glMapBuffer/OES        glGetQueryObjectui64v/EXT
glGetQueryObjectiv/EXT  glBufferStorage/EXT
```

`nm -D` on desktop `libGL.so.1` confirmed **all but the two un-EXT-suffixed
marker names** are exported there (the un-EXT-suffixed `glPush/PopGroupMarker`
are the only two missing from both GLESv2 and desktop GL).

## Fix (crates/arm64jit/src/resolver.rs)

1. New `gl_desktop_handle()`: one-time `dlopen(libGL.so.1)` with the same
   `RTLD_NOW|RTLD_LOCAL` discipline as `gles_handle()`, so these names stay
   visible only to the whitelisted int-ABI resolver and never leak into the
   general `RTLD_DEFAULT` `resolve()` (which would grab float-taking `gl*`).
2. `resolve_gles_int()` now falls back to the desktop handle when a
   whitelisted int-ABI name is absent from GLESv2. All 16 names were added to
   `GLES_INT_NAME_LIST` (each is pure int/ptr ABI, ≤4 args — safe through the
   integer HostCall, correctly rejected by `resolve_gles_mixed`).
3. Result: the engine's `eglGetProcAddress` now returns a real dispatchable
   int-bridge slot for these names, so its table auto-heals (SH3 mechanism) —
   no harness re-seed needed.

## Verification (live + regression)

- Productized `open-sober play --apk --jit` (the real product command) re-run
  with my changes — see `runs/sh47-play-jit.txt`: exit 124 (stable idle loop),
  full render green (3 geometry-wrapper Ok(0x0), triangle centroid red, textured
  quad BL=RED/BR=GREEN/TR=WHITE/TL=BLUE, 6 fresh quad-loop frames, every swap
  Ok(0x1)), and **slots 11/12 = 0x7f0000003098/90** in the seed snapshot.
- Direct harness re-run with `JIT_EGL_LOG=1` — `runs/sh47-egllog2.txt`:
  **slots 11/12 went `0x0` → `0x7f0000003098/90` (real bridge slots)**;
  UNRESOLVED dropped from **16 → 2** (only un-EXT-suffixed `glPush/PopGroupMarker`,
  absent from both libs — the EXT variants are bridged).
- New regression
  `gles4_extension_names_resolve_via_int_bridge_desktop_gl_fallback` pins all
  15 desktop-exported names resolve through the int bridge (trailing NUL) and
  are rejected by mixed; asserts the critical set (`glBufferStorage`,
  `glMapBuffer`, `glQueryCounter`, `glObjectLabelKHR`) all resolve.
- `cargo test --workspace` **494/0** (was 493/0, +1). Build clean.

## What slot 11 actually is (disassembly)

Slot-11 stub `0x5b3a244`'s callers pass `w0=0x8a11 (GL_UNIFORM_BUFFER)`, a
size, `x2=xzr` (NULL data), and a flags word — the exact
`glBufferStorage(target, size, data, flags)` signature. So the engine's modern
buffer path already uses `glBufferStorage` for UBOs; previously that dispatched
to NULL. Now it routes through the int bridge to real Mesa.

## Honest framing / frontier

This closes the last NULL-dispatch slot in the engine's own GLES3 table —
another real crash surface removed on the path to a self-driven frame
(objective 2a). The structural wall is UNCHANGED: the engine still never
self-produces a render/session task (type-4 producer vector `[0x6829ea8]` is
framework-glue-installed only, SH44/SH46). When that wall falls, a real engine
frame's buffer/timer/query path now routes through the bridge instead of
jump-to-NULL.