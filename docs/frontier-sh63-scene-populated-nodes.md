# SH63 — the engine's real scene renderer walks a POPULATED scene list: one real frame per 0x28-stride node

## Summary

SH62 proved the engine's OWN scene renderer (`0x105b2ead4`) constructs + registers
a real 0x98 frame-desc even with an EMPTY scene list (only the single base frame
at R+0x170). SH63 closes that → the "populate the scene list" gap: `--renderscene`
now lays N **0x28-stride scene nodes** into the engine-native render-manager R
(R+0x180=head, R+0x188=tail), and the engine's per-node loop (file `0x5b2eb9c`)
builds **one real frame per node**. On the real libroblox.so headlessly:

```
[elfjit:renderscene] frame #0 scene renderer Ok(0x1) -> R+0x170 frame=0x7fd2ddea5c30 node=0x7fd2ddea5ad0 engine_registered=true scene_nodes=3 per_node_frames=[140543646281776, 140543646282944, 140543646274672] per_node_ok=true frame[vtable]=0x1067317b0[+140]=0x0 view=4294967295x4294967294
[elfjit:renderscene] present #0 swap Ok(0x1) — engine-scene-renderer frame presented (base frame 0x7fd2ddea5c30 + 3 per-node frames)
[elfjit:renderscene] present #1 swap Ok(0x1) — engine-scene-renderer frame presented (base frame 0x7fd2ddea5c30 + 3 per-node frames)
```

- `scene_nodes=3`, `per_node_frames=[<3 host ptrs>]`, `per_node_ok=true` — the
  engine built a real frame-desc per node (each [+144]==1, vtable realm ok).
- 2 swap Ok(0x1) presents, exit **124** (stable idle), persist 45B byte-exact,
  zero SIGSEGV/SIGABRT/json-overflow.
- SH62 empty baseline re-verified green (RENDERSCENE_NODES=0 → the fast path,
  base frame only, still swaps Ok(0x1)).
- New regression `scene_per_node_build_contract_populated_scene_list` pins the
  node offsets + the head/tail one-past-end termination. Workspace **508/0**
  (was 507/0, +1).

## The per-node contract (disasm of file 0x5b2eb9c, the renderer's scene walk)

For each 0x28-stride node from R+0x180 (head) to R+0x188 (tail):
```asm
ldur x0,[x20,#-16]   ; node+0x08 -> RENDER-OBJ
ldr  x8,[x20]        ; node+0x18 -> VIEW
ldr  x9,[x0]; ldp w1,w2,[x8,#112]; ldr x8,[x9,#64]; blr x8  ; obj-vt[+64] dims-query
; rebuild-skip: if node view W/H (+112/+116) == queried dims -> skip build
mov  w0,#0x98; bl 1d96768            ; operator-new (0x98 frame)
... ctx-vt[+32] cache-query -> w7 ...
bl   5b34de8                          ; frame ctor (this, R?, w2,w3=W/H, 1,1,4, w7)
add  x0,x20,#0x10; mov x1,x21(frame); bl 5b2d9e0   ; LINK(container node+0x18, frame)
add  x8,x20,#0x10; add x20,x20,#0x28; cmp x8,x24    ; advance 0x28; next==tail?
b.ne loop
```
The linker `0x5b2d9e0` writes the frame + a 0x20 link-node into the container.
SH63 sites each node's container at **node+0x18** (the SAME slot the walk reads
as the view for W/H). Node layout laid out by `render_scene_base(node_count)`:

| offset | meaning | value |
|---|---|---|
| node+0x00 | not read | 0 |
| node+0x08 | render-obj (only obj-vt[+64] dims-query blr'd) | = ctx (reuses real vt[+64]) |
| node+0x10 | not read | 0 |
| node+0x18 | view ptr (W/H at +112/+116; MUST be non-NULL — `ldp` derefs before null-check); the linker overwrites it with the frame | sentinel view |
| node+0x20 | linker's `[container+8]` old tail (0 → skip chaining) | 0 |

R+0x180=head, R+0x188 = head + N×0x28 (one-past-end) so the walk's
`cmp (x+0x10),tail` terminates exactly after the last node.

**Why sentinel W/H still matters per-node:** each node's view (+0x18) is the
same sentinel view (W=`0xFFFFFFFF`, H=`0xFFFFFFFE`), so the per-node
rebuild-skip check (`cmp w9,w22; b.eq`) never matches the live surface dims and
the engine always takes the BUILD branch — it constructs the frame at the REAL
queried dims. (Setting it to 1280×720 would make the engine skip per-node
construction, same class as the SH62 gotcha.)

## Honest scope

The engine now builds a real frame-desc per scene node AND the base frame, and
presents them via its real swap — so a POPULATED render-manager causes the engine
to register a genuine multi-item frame plane headlessly. The node's **render-obj**
is the recovered ctx only (its vt[+64] is a real dims-query, so the per-node
build runs); it is NOT yet a real engine UI/GuiObject, so the per-node frame
carries engine-detail metadata, not a populated login/home screen. Real screens
still need the Lua app-shell (StartLuaAppDM) → the standing structural wall
(nativeGameGlobalInit parks; recon disproofs SH53/SH56). This closes the 
specifically-named per-node frame-build plane and validates the exact node/item
ABI a Lua-created screen would consume.

**Next frontier:** (a) drive the engine's real per-node PRESENT walker
`0x105b2ed48` (the fn that blr's each node+8 item's vt[+24] as the per-item draw
then swaps — SH63's per-node frame-build is the construction side; the present
side is gated on nativeGameGlobalInit, so reach it by setting the walker's R+559
/ R+664 / R+608 gate bytes, or synthesizing its draw item's vt[+24] → real
geometry emitter 0x105b35288); or (b) target the engine's own geometry emitter
(0x105b35288 / primitive-setup 0x105b353d0, the SH25-34 proven path) as the
per-node render-obj so a populated node actually draws engine-detailed content.

## SH64 empirical note (present-walker drive attempted, reverted — read before re-trying)

I tried driving the engine's REAL per-node PRESENT walker to make it draw
engine-detailed content, and captured a hard empirical constraint:

- **Full-body drive of `0x105b2ed48` aborts at entry** (SIGSEGV guestpc=0x105b2ed48,
  fault=0x18) — its prologue/epilogue touch TLS stack-canary + the post-present
  teardown tail calls `nativeOnDestroyed` helpers (0x2839ae4/0x283a3f8), which
  fault when entered from the harness with only the gate bytes
  (R+559=0/R+664=0/R+608=1) set. Not a clean drive.
- **Mid-function present-loop region `0x105b2eec0` (x19=R preset via CpuState):
  the engine's real loop DOES run and blr's our fabricated per-item draw** —
  `item draw #1 engine frame-fn Ok(0x...)` (real engine frame-fn 0x105b32c00 ran),
  validating the exact per-scene-item `vt[+24]` draw ABI a Lua-created screen's
  node+8 item would dispatch. But the run SIGSEGVs on the SECOND loop iteration
  (0x105b2eedc): the nested `jit_run` inside the item draw thunk recompiled /
  replaced the very present-loop block the outer jit_run was currently executing
  — the same class as the SH44/SH49 drain recompile-desync. So per-node present
  via a thunk that itself drives engine code is blocked by block-cache mutation
  while mid-block.
- **Conclusion / next-try:** to present via the engine's real per-node loop, the
  per-item draw must NOT trigger a nested jit_run that recompiles the
  present-loop block — either (i) pre-compile/lock the present-loop block so a
  nested drive can't evict it, or (ii) synthesize the draw as a host-thunk that
  calls the geometry emitter through the ALREADY-seeded GLES dispatch-table slots
  (no nested jit_run at the present-loop address), or (iii) patch the walker's
  parked `nativeGameGlobalInit` bl (0x5b2ee54) to a `ret` so its FULL body runs
  natively to the present loop + real swap. This is left as the standing next
  frontier; the SH63 per-node frame-BUILD (this cycle) is the committed,
  verified deliverable.