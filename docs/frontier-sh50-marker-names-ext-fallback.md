# SH50 — close the LAST two NULL-dispatch GLES slots: plain (non-EXT) marker names resolve via an EXT-sibling fallback

## Summary

The engine's real render dispatch table (BSS `0x106d3b2f0 + 8*N`, built via
`eglGetProcAddress`) had two function-pointer slots that still read `0x0`: the
**plain, non-EXT** debug-marker names `glPushGroupMarker` / `glPopGroupMarker`
(GL_EXT_debug_marker without the EXT suffix). JIT_EGL_LOG (SH47) reported these
as the last two `UNRESOLVED` names. A guest `br` through those slots — the
SH19/SH24/SH47 NULL-dispatch crash class, applied to the two names that cycle
never reached — would jump to address 0 in a self-driven frame.

## Root cause

- The names ARE in `GLES_INT_NAME_LIST` (SH47 added them), so they passed the
  int-bridge whitelist gate.
- But `resolve_gles_int` then required the exact symbol from Mesa:
  `sym_from(libGLESv2, "glPushGroupMarker")` → NULL, then
  `sym_from(libGL, "glPushGroupMarker")` → NULL.
- Both Mesa libraries export **only the EXT-suffixed spellings**:
  `nm -D libGL.so.1 | grep GroupMarker` → `glPushGroupMarkerEXT` /
  `glPopGroupMarkerEXT` only (libGLESv2 exports neither spelling).
- Native `sym_from`/`dlsym` cannot alias the plain name to the EXT spelling, so
  without an explicit fallback the plain names were unresolvable → slot 0.

## Fix (crates/arm64jit/src/resolver.rs, `resolve_gles_int`)

When a whitelisted name resolves to NULL in both Mesa libs AND does not already
end in `EXT`, fall back to the EXT-suffixed sibling (`{name}EXT`), which is
itself a whitelisted integer-ABI-safe name. Tried against libGLESv2 then
libGL.so.1. Both plain marker names now resolve to a real bridge slot, so a
guest `br` through the engine's table dispatches real Mesa code instead of
jumping to NULL.

## Verification

- New regression `plain_non_ext_marker_names_resolve_via_int_bridge_ext_sibling_fallback`
  (arm64jit/src/resolver.rs): all four spellings (plain + EXT, both push/pop)
  resolve via the integer bridge (trailing-NUL form), are bridge-pointer slots,
  and are rejected by the mixed (float) bridge. Workspace **496/0** (+1).
- The SH47 test `gles4_extension_names_resolve_via_int_bridge_desktop_gl_fallback`
  now resolves all 16 names (its `resolved >= 13` bound still holds).
- Productized `open-sober play --apk roblox-android.apk --jit` re-verified
  (runs/sh50-product-reverify.txt): exit 124 stable, real indexed triangle
  (centroid RGBA(255,0,0,255)) + textured quad (BL=RED / BR=GREEN / TR=WHITE /
  TL=BLUE exact texels) + 6 fresh quad-loop frames, every post-draw swap
  Ok(0x1). **Zero eglGetProcAddress UNRESOLVED marker lines** (SH47's run had
  2).
- Note: a bare direct-elfjit `--renderinit` run SIGABRTs in a background guest
  thread right after ANativeWindow wiring — confirmed **pre-existing** (aborts
  identically with the change stashed) and before this cycle's code path, i.e.
  the documented SH46 harness-bootstrap artifact, not a regression. The
  productized deliverable path is unaffected.

## Honest scope / frontier

This removes the last NULL-dispatch surface in the engine's real GLES render
table — every slot a self-driven frame can dispatch now routes through the
bridge. The standing structural wall is unchanged (SH14/SH46/SH49): the type-4
task-producer vector `[0x6829ea8]` is populated only by real Android framework
glue absent headlessly, so the engine still does not self-produce a frame; frames
remain harness-driven on the live engine context.