# SH53 — Disprove the "in-image TaskScheduler installs the type-4 producer vector" reframe

## Summary

A 2026-09-12 recon (docs/recon-framework-boot-order.md, spec at
`/home/hermes-worker/open-sober-framework-glue-spec.md`) reframed the ~50-cycle
standing wall: it claimed the type-4 producer vector `[0x106829ea8]` is
installed **IN-IMAGE** by TaskScheduler/V2-init code that SH46's scan "never
reached" because it ran on a bare boot. It proposed driving the real V2 ladder
(`nativeGameGlobalInit → nativeUpdateAdapterInit → V2InitWithParams →
StartLuaAppDM → [Surface] → StartAppWithParams`) in order and watching the
vector populate — labeled "the highest-leverage change".

This cycle disproves the reframe on two independent grounds and pins the
disproof so no future cycle re-derives the wrong path.

## Argument 1 — the scan is static, so "never reached it" can't apply

SH46's conclusion is from a **whole-`.text` disassembly scan**, which is
execution-independent. Whether or not the harness ever *runs* TaskScheduler
init is irrelevant to whether `libroblox.so` **contains** an instruction that
stores to guest `0x106829ea8`. If V2/TaskScheduler init installed the vector
in-image, some decoded instruction would write `0x106829ea8` — none does.

## Argument 2 — the computed-base escape hatch is closed

The recon's one plausible static dodge: the vector `0x106829ea8` is the `.bss`
array base `0x6829e80` **+ 0x28**, so a *computed-base* store
(`adrp 6829000; add xN,xN,#0xe80; str [xN,#0x28]`) would escape a
literal-`#3752`-offset scan. Disassembling **every** `adrp xN,6829000` site in
the real binary:

- **0x2953e30** — `x19←0x6829e80; ldr x0,[x19]; bl 2a1ce78; str xzr,[x19]`
  → writes the array base `+0x0` (clears the first qword), **not** `[x19+0x28]`.
- **0x295427c / 0x29542ec** — operate on **0x6829e88** (`+0x8`) as an atomic
  counter (`ldxr`/`stxr` at 427c–4290, `stlr` at 300) — not the vector.
- Every other `adrp`-6829000 `add` targets `#0xba8 / #0xe80 / #0xe88 / #0xf00`
  — none reaches `#0xea8`.
- All `add #0xea8` sites in the image are **struct-relative** on dynamic bases
  (`x0/x1/x2/x19/sp`), never a 6829000-derived register.

So neither a literal-static NOR a computed-base in-image store exists.

## Corollary — the V2-ladder probe is provably futile in this harness

Closed-shape argument for why the recon's "drive the real ladder and watch the
vector populate" cannot succeed here, independent of the disassembly:

- The arm64jit harness loads exactly **one guest ELF** (`libroblox.so`). Every
  Android/mediandk/bionic library the client touches is resolved to a *host*
  symbol (dlsym/thunk/GLES-bridge), NOT loaded as a second guest ELF the JIT
  could execute-from-image-guest-addresses.
- A "cross-module guest install site" therefore cannot exist in this harness:
  there is no other guest `.text` (or guest `.bss`) to install into
  `[0x106829ea8]` from.
- Combined with the no-in-image-store disproof, the *only* remaining mechanism
  to populate the vector is a **host-side seed** — which is precisely what the
  existing `--taskv4-seed <addr>` lever already does (SH44/SH49 proved the
  plane is live when seeded).
- Conclusion: building the full AutoValue-ladder drive to test an "in-image or
  cross-module installation" that cannot exist is wasted effort. If a future
  cycle wants a self-driven frame, the productive levers are either (a) find a
  *real host-glue* way to seed the vector with an engine guest handler, or
  (b) the documented (SH51) objective-2b direction — exercise the client's real
  fsmap/SQLite serialization, not the type-4 producer.

## Conclusion / frontier

If the V2 ladder installs `[0x106829ea8]` at all, it is via **cross-module
glue** (a different loaded library, or a host-side seed) — out of scope of an
in-binary scan and fully consistent with SH46's original "external glue"
conclusion. The recon's stated *justification* ("in-image TaskScheduler init
installs the vector, we just never reached it") is disproven. Driving the V2
ladder empirically remains an open frontier **but only to test cross-module /
runtime install**, not an in-image one; its cost (synthesizing JNI AutoValue
`InitParams`/`StartAppParams` jobjects + a real `Surface`) should be weighed
against that narrower expectation.

## Verification

- `cargo test --workspace`: **497 passed / 0 failed** (unchanged; regression
  strengthened, not added).
- Regression `type4_taskv4_vector_has_no_in_code_install_site_and_uses_static_base`
  now pins: the vector's offset **0x28** within the `.bss` array (`0x6829e80`),
  the disproof audit (the real sites touch `+0x0`/`+0x8`, never `+0x28`), and
  the static+computed-base both-ruled-out headline.
- The untracked recon doc `docs/recon-framework-boot-order.md` is added to the
  repo alongside this disproof so the analysis is preserved with its verdict.