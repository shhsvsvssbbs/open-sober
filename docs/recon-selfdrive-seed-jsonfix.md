# Recon v3 — self-driven frame: concrete seed handler + json-leak fix

Static derivation (deleg_4ed1298e, read-only on real libroblox.so; guest = file vaddr + 0x100000000).
The type-4 vector IS external glue (re-confirmed: zero reloc/store); these give the concrete
implementable artifacts to turn "host-side seed" + "json abort" into measured results.

## A. Type-4 seed handler (the self-driven-frame path)

Dispatch (confirmed): drain pop-loop 0x102856e40 -> node-processor 0x10285371c (w4==4) ->
`ldr x3,[0x106829ea8]; BR x3` at 0x1028537b8 (BR = leaf, x30 preserved into re-enqueue 0x10285682c).
**ABI: handler(node=x0, [node+32]&~1=x1, consumer=x2)**, return discarded.

Node struct (feeds only the deque, NOT mesh data): +0 next-link, +32 task arg (low bit = per-CPU
slot selector), +40 dispatchable flag (must be |=1), +112 processor vtable (-> vt; [vt+40]=processor,
[vt+16]=ctx marker). IMPORTANT: geometry wrapper 0x5b35288/primitive-setup 0x5b353d0 are on a SEPARATE
render-stream path (single caller) — a type-4 node cannot reach them. The vector is the terminal
task-consumer forward edge; the framework glue mapping node->render work is what we synthesize.

**NO genuine in-image handler exists** (vector = true .bss, zero stores, zero relocs). Seed = a
registered non-recursive leaf **host-thunk** at HOST_THUNK_BASE (0x7f00_0000_0000, via
register_host_call_auto). It must NOT re-enter 0x10285371c/0x102856e40/the vector (would recurse).

Reference thunk (Rust, jit.rs): build_fabricated_renderer is SH18's; GLES slots seeded via SH19
resolve_gles_mixed (0x106d3b2f0..0x106d3b328):
```
extern "C" fn type4_frame_thunk(node, arg1, consumer, _a3,_w4,_a5,_a6,_a7) -> u64 {
    let ctx = RENDERCTX.load();          // filled by --renderthunk (real 0x48 ctx, vtable 0x106731ae0)
    if ctx == 0 { return 0; }
    let vt = *(ctx as *const u64);
    let bind = *( (vt+16) as *const u64);    // 0x105b3b358 make-current
    let swap = *( (vt+24) as *const u64);    // 0x105b3b408 eglSwapBuffers
    run_guest_callback(bind, &[ctx,0,0,0,0,0,0,0]);            // engine make-current
    let fb = build_fabricated_renderer();                       // SH18 coherent renderer/view
    run_guest_callback(0x105b32c00, &[fb, view,0,0,0,0,0,0]);  // engine frame-fn
    run_guest_callback(swap, &[ctx,0,0,0,0,0,0,0]);            // engine swap -> real frame
    0
}
```
Install: `--taskv4-seed 0x{type4_frame_thunk_addr}` (existing write path elfjit.rs L1749 =
`*(0x106829ea8)=addr`). ORDER: --renderinit/--renderthunk must recover RENDERCTX BEFORE the seed;
--renderframe-seedgles before first frame-fn; --deque-node-live 0x106829f00 sustains dispatch.

Full cmd: `elfjit ... --jni --startapp 0x258b144 --renderinit 0x105b3a280 --renderthunk
  --renderframe --renderframe-seedgles --taskv4-seed 0x{thunk} --deque-node-live 0x106829f00
  --drain-poll 8 --kicker 0x106863af8` (JIT_DRIVE_LIFECYCLE=1, RENDERINIT_WARMUP_MS=1000).

Verify (concrete markers): `[elfjit:taskv4] seeded ... vector [0x106829ea8] = 0x7f00...`;
`type4-frame task #N ...`; `present #N ... swap Ok(0x1)` monotonically with pops (the real
frame/session-tick counter); JIT_FRAMEWORK_DUMP egl/gl histogram nonzero; x11grab window shows color.
Exit 124 (stable idle loop) not crash.

Reject: seed = engine's own 0x10285371c (infinite recursion), seed drain itself (re-entrant),
wire to 0x5b35288 directly (wrong path, node has no geometry).

## B. json-Writer stack-leak fix

Fault: append bound-check `0x102355d40` compares `len(x2) > cap([0x102727648])` -> throw
`RBX::json::Writer string length overflow` (fmt 0x10057765a, throw helper 0x1025fb6bc).
Root cause: an **unconstructed guest-stack libc++ std::string** in StartApp's serializer caller frame
(AppStarted struct built from StartAppParams fields). Its `data()/size()` are leftover stack words
(host mmap ptr / sp or sp-0x30, ASLR). The AutoValue getters/JNI layer is NOT involved (SH56: zero
Call*Method lines).

Why play --jit dodges it: `JIT_DRIVE_LIFECYCLE=1` populates the app/framework object so the
std::string members ARE constructed (len=0); bare --startapp omits it that.

**THE FIX: force string length 0 — do NOT raise the cap** (raising cap makes the writer memcpy with
len's low 32 bits ~1.6GB -> SEGV). Minimal deterministic host-side seed: zero-fill the leaking slot
at `[append-entry-sp - 0x38]` (8 bytes; also [S+16], [S+23]) so size()=0 -> SSO empty string -> no
overflow. Slot is a fixed frame offset (code-relative, `len == append-framebase`, S~=E-0x38), so the
write is deterministic; the harness recovers E/x2 from the JIT_DUMP_PC register dump at 0x102355d40.
Simpler robust variant: drive JIT_DRIVE_LIFECYCLE on the StartApp path (already proven to dodge), or
pre-construct the StartAppParams std::string members in the seed (SH45 seed_static_empty_map precedent).