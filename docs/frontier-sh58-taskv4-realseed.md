# SH58 — first real-guest-handler type-4 seed executed: the drain dispatches real in-image engine code

## One-line result
The recon §3.6 "interim fallback" — seed the type-4 popped-task vector
`[0x106829ea8]` with a **REAL in-image guest handler** (not the host `probe`
thunk every SH44-57 run used) — was empirically executed for the first time on
the real `libroblox.so`. Seeding it with the engine's OWN frame-fn
`0x105b32c00` made the drain's w4=4 dispatch (`adrp x8,6829000; ldr x3,[x8,#3752];
br x3`) actually **br into real engine code**: frame-fn's own renderer list-find
(`guestpc 0x105b2e98c`) executed, then faulted on the ABI mismatch (the vector
passes `handler(node, [node+32]&~1, consumer, w4, x5)`, but frame-fn expects a
coherent renderer/view, not a task node).

**What this proves:** the type-4 dispatch plane mechanically accepts and runs a
real guest function pointer. The wall is (and only ever was) that the
framework-installed "process popped task node" worker address is external glue
absent in-image — NOT that the seed rejects guest code.

## Why this cycle
Every SH44-57 frontier doc names the same next step: *"feed a REAL engine
frame/session producer address into the seed so a sustainably-dispatched task
node advances the engine toward its own frame/screen."* But every run used
`--taskv4-seed probe` (a registered HOST thunk) — the guest-hex form of the lever
(`--taskv4-seed <guest-addr>`) existed but had **never been exercised**. This
cycle closes that gap: it runs the real-guest seed that the recon §3.6 interim
fallback describes, and characterises what a valid real seed's ABI must be.

## The experiment (artifact runs/sh58-taskv4-realseed.txt, exit 139)
Full productized boot recipe + `--taskv4-seed 0x105b32c00 --deque-node-live
0x106829f00 --drain-poll 8`:

```
[elfjit:taskv4] seeded dispatcher type-4 vector [0x106829ea8] = 0x105b32c00
    — a popped task node reaching w4=4 will now call it
[SIGSEGV] tid=2755760 fault=0x170 rip=... guestpc=0x105b2e98c
    rbx_matches_gueststate=true
```

The guest PC where it faulted (`0x105b2e98c`) is **inside the engine's real
frame-fn** (the renderer `find`/list-walk at the top of 0x105b32c00). So the
vector was written, the drain `br`'d to the guest address, and the JIT translated
+ executed real engine instructions — the FIRST confirmed real-guest-code
dispatch through the type-4 plane. frame-fn returns when handed a coherent
renderer (SH25 proves that); it faults here purely because the drain hands it
`(node, [node+32]&~1, consumer, 4, 0)` which is not frame-fn's
`(renderer, view, w2, w3, clearobj, ccobj)` signature.

## Interpretation for the frontier
- **Seed rejection is NOT the wall.** A real in-image guest address written into
  `[0x106829ea8]` is a valid, routinely-dispatched seed (guest==host addressing
  makes it a plain writable function-pointer slot in `.bss`).
- **The wall's exact shape is confirmed again:** the correct real seed is the
  framework-installed "process popped task node" worker whose address is not
  statically in the binary (SH46/52/53/55/56 established: no in-image store site,
  no APS2 reloc, no computed-base escape; the V2 ladder stalls forever at
  `nativeGameGlobalInit`). frame-fn is not that worker (wrong ABI) — it was a
  deliberately-observable real-code probe, not a claim that frame-fn *is* the
  correct seed.
- **ABI lesson for a future real-producer seed:** the drain dispatches with
  `x0=node, x1=[node+32]&~1, x2=consumer, w4=4, x5=0`. A valid real-guest seed
  must read its task content from the node argument; a function matching that
  ABI, once its address is known, can be `--taskv4-seed`'d and will sustainably
  dispatch (SH49's 197-pop plane, re-verified this cycle on current HEAD).

## Verification
- New regression `type4_vector_seed_accepts_real_in_image_guest_function`
  (arm64jit/src/jit.rs) pins the vector address, that frame-fn is a guest `.text`
  address below the vector, and the ABI contract — so future real-producer seeds
  know the seed is mechanically valid and must match `(node,...)`, not frame-fn.
- Workspace **503/0** (was 502/0, +1).
- Current-HEAD re-verification of the standing plane: `--taskv4-seed probe
  --deque-node-live` sustain (runs/sh58-taskv4-sustain.txt): **190 consecutive
  node pops, 3 clean type-4 dispatches, zero crash, exit 124** — SH49's result
  holds after the SH55 (JNI value registry) / SH56 (jobject diagnostic) / SH57
  (asset+storage+assetpath) changes.
- Productized real-boot re-verified (runs/sh58-baseline.txt): exit 124 stable,
  real indexed triangle + textured quad + 6 quad-loop frames (swaps Ok(0x1)),
  byte-exact persist roundtrip, 594 assets extracted + served, zero ENOSYS.

## Honest scope (unchanged structural wall)
Frames remain harness-driven on the live engine context; the engine still never
self-produces a session/frame because the type-4 producer vector is
framework-glue-installed only, and the real worker's address is not in the
binary. This cycle's contribution is empirical: it proves the seeded dispatch
runs REAL engine code (not just a host probe), removing seed-rejection as a
candidate explanation and pinning the exact ABI a future real producer must
match, plus re-confirming the whole stack is green on current HEAD.