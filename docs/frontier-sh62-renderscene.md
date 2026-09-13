# SH62 — the engine's OWN scene renderer constructs+registers its real frame-desc

## Summary

New elfjit lever `--renderscene` drives the engine's **real** frame-plane driver
(guest `0x105b2ead4`, the scene renderer) with a fabricated-but-**engine-native**
render-manager R, replacing the SH60/61 harness-fabricated *clear-path* renderer
(frame-fn `0x105b32c00`) with the engine constructing and registering its **own**
real 0x98-byte frame-desc. On the real libroblox.so, headlessly:

```
[elfjit:renderscene] frame #0 scene renderer Ok(0x1) -> R+0x170 frame=0x7fb68a1c8c00 node=0x7fb689fa4f30 engine_registered=true frame[vtable]=0x1067317b0[+140]=0x0
[elfjit:renderscene] present #0 swap Ok(0x1) — engine-scene-renderer frame presented
[elfjit:renderscene] present #1 swap Ok(0x1)
[elfjit:renderscene] present #2 swap Ok(0x1)
[elfjit:renderscene] drained: 3 real engine-scene-renderer frames presented (engine-built frame-desc)
```

exit 124 (stable idle), persist 45B byte-exact, zero SIGSEGV/SIGABRT/json-overflow.
Workspace **507/0** (+1 regression). Doc docs/frontier-sh62-renderscene.md, artifact
runs/sh62-renderscene.txt, repro runs/capture_renderscene.sh.

## Why this is the frontier advance

Every SH60/61 "self-driven task frame" drove the engine's **clear-path** frame-fn
`0x105b32c00` with a host-coherent *fabricated* renderer (fields the harness wrote
by hand). The engine's **real** frame-plane driver is `0x105b2ead4` (the scene
renderer), and it only renders its own screens once it has a POPULATED render-manager
(R+0x160 ctx / R+0x170 view / R+0x180 scene list). The recon (deleg_ed4a/d2b57,
read-only on the real binary) + my disassembly pinned an implementable fact:

**`0x105b2ead4` builds + links a real 0x98 frame-desc UNCONDITIONALLY — even with an
empty scene list.** Disasm (file `0x5b2ead4`):

```
ldr x8,[R+352]        ; R+0x160 = ctx
ldr x8,[x8]; ldr x8,[x8,#16]; blr x8   ; ctx make-current (vt+16 = 0x105b3b358)
ldr x8,[R+368]        ; R+0x170 = view ptr
ldp w1,w2,[x8,#112]   ; view W/H at +112/+116
ldr x8,[ctx-vt+#64]; blr x8  ; dims-query -> (H<<32|W)
; (rebuild check: if view W/H == queried dims -> b.eq SKIP the build)
mov w0,#0x98; bl 1d96768        ; operator-new (0x98 frame)
ldr x8,[R+352]...; blr [vt+#32] ; w7 = cache-query ret
; bl 5b34de8       ; frame-desc ctor (sets vtable 0x6731000+0x7b0=0x1067317b0, [+140]=w7, [+144]=1)
; bl 5b2d9e0(&R+0x170, frame)   ; link: op-new(0x20) node, container[0]=frame, container[8]=node
ldp x8,x24,[R+384]   ; scene list head/tail  (R+0x180 / R+0x188)
cmp x8,x24; b.eq 5b2ec3c  ; EMPTY scene => skip => return 1
```

So the scene-array gate is **after** the frame construction — a fabricated R with
`R+0x180==R+0x188` (empty) still gets the engine's own operator-new/ctor/link run,
which registers a real engine frame item into R+0x170. `--renderscene` arms exactly
that R (ctx at R+0x160, view with sentinel W/H at R+0x170, empty scene at R+0x180/188),
drives `0x105b2ead4(R)` on the currency-owning thread, verifies the engine registered
a real frame-desc (vtable `0x1067317b0`, [+144] byte ==1), then presents via the real
ctx swap (`0x105b3b408`).

## The one empirical catch (SDLC-relevant)

The recon's sample `build_fabricated_renderer` layout set the view W/H as 1280x720 —
**but that exactly equals the live EGL surface dims the dims-query returns**, so the
renderer's "dimensions unchanged => frame already built" check (`cmp w8,x22; b.eq
skip`) SKIPPED construction and R+0x170 stayed pointed at our view (engine_registered=false).
Fix: set the view W/H to a sentinel (`0xFFFFFFFF`/`0xFFFFFFFE`) that can never equal the
surface dims, forcing the BUILD branch every time. The engine then constructs the frame
at the REAL dims (the build passes the queried W/H to the ctor). This is why the
fabricated view must NOT mirror the surface size.

## Delivery

- `render_scene_base()`: lazily leaks a 0x400-byte R; `R+0x170`->internal view obj
  (W/H sentinel at +112/+116); `R+0x180==R+0x188==0` empty scene.
- `render_engine_scene(ctx, n)`: reads ctx-vtable make-current/swap, stores ctx at
  R+0x160, engine make-current, `run_guest_callback(0x105b2ead4, [R,...])`, verifies the
  engine_registered predicate (frame non-zero, [+144]==1), dumps frame[vtable]/[+140],
  then engine swap. Returns the swap result.
- Presenter wiring: after RENDERCTX is published on the renderinit (currency-owning)
  thread, `--renderscene` drains a bounded window (default 3 frames / 1.5s, env
  RENDERSCENE_MAX_FRAMES / RENDERSCENE_WINDOW_MS) so the run still exits 124 cleanly.
- Regression `scene_renderer_constructs_frame_desc_even_with_empty_scene`: pins the
  guest addrs (renderer 0x105b2ead4 / ctor 0x105b34de8 / linker 0x105b2d9e0 / op-new
  0x105d96768 / frame vtable 0x1067317b0), the R layout offsets (R_CTX 0x160, R_VIEW
  0x170, R_SCENE_HEAD 0x180, R_SCENE_TAIL 0x188, VIEW_WH 112, FRAME_FLAG 144), the
  engine_registered predicate, and that the frame build is NOT gated on the scene array.

## Honest scope

This activates the engine's REAL scene-renderer frame-plane and proves it constructs +
registers its own frame item headlessly — but the item is the **frame-desc** (the
engine's real per-frame object), not yet a populated game/login UI screen.** The render-manager's R+0x180
scene list is still EMPTY here (the harness uses the empty-scene fast path). Real
login/home screens require the engine to populate the scene list with its UI items —
which recon pinpoints as requiring the Lua app-shell (StartLuaAppDM + Lua runtime) +
auth/network — the standing structural wall beyond the render plane. Contribution:
the engine's own render-manager frame construction is now provably reachable headlessly,
and the fabricated renderer (SH18/60 clear path) is replaced by the engine's real
frame-desc construction for frame presentation.

## Next frontier

Populate the scene list (R+0x180): construct a real 0x28-stride scene node whose
node+8 is a coherent render object + node+24 a real view, so the renderer's per-node
loop (present only after R+0x180 != R+0x188) draws + presents engine-detailed content,
not just the single empty-scene ViewFill. Structurally depends on finding the in-image
scene-item builder (file 0x5b2c828 / 0x5b2eb7c / 0x5b2ec1c are the three build sites a
recon surfaced) or a real screen-construction entry beyond the Lua wall.