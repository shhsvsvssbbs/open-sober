# Open-Sober Status — Ongoing Autonomous Development

## SH46 (Sep 12, 2026): DISPROVED SH45's LSM-seed hypothesis for the bare-`--startapp` `RBX::json::Writer string length overflow` abort — the leaked "length" is the guest STACK POINTER (== sp or sp−0x30), an uninitialized guest stack std::string in StartApp's json serialization of launch params, NOT the harness-seeded LSM map (~0x260–0x2a0 MB away). Also proved the type-4 task vector `0x6829ea8` has NO in-code install site (framework-glue only). Workspace 493/0 (was 491/0). Doc frontier-sh46-json-abort-sp-disproof.md.

Per SH45's frontier doc the proposed next lever was "make the seeded LSM
empty-map / fake-object memory guest-shaped / zero-length so StartApp's json
serialization reads a valid empty string length instead of a host pointer."
This cycle empirically disproves the premise:

- **Leak == guest sp.** `JIT_DUMP_PC` at the json append check-fn entry
  (0x102355d40) and throw helper (0x1025fb6bc) over 3 independent runs (fresh
  ASLR): overflow value is always `== sp` (run A leak 0x7f4462ffd9f0 exactly
  equals the throw frame's x29==x31) or `== sp−0x30` (runs B/C). The seeded LSM
  bucket array sits ~0x260–0x2a0 MB away — not the leaking allocation. Real
  mechanism: StartApp's `nativeAppBridgeAppStart`-family json writer reads an
  uninitialized stack std::string as a length; the append bound-check
  (`ldrsw [0x7275000+1608]; cmp; b.cc`) throws "RBX::json::Writer string length
  overflow: %zu" (fmt file 0x57765a) via 0x25fb6bc. Guest-internal.
- **Type-4 vector: no install site in the binary.** Full objdump scan: no
  `adrp 0x6829000` + `[x,#3752]` store exists (only the dispatching `ldr` at
  file 0x2853784); the only `[x,#3752]` stores are struct-relative on heap/sp
  regs. "Reverse what the framework installs into 0x6829ea8 in-code" is a dead
  end — it's external framework/game-activity glue absent headlessly; the
  dispatch plane stays proven live when seeded (SH44).

Two new regressions pin both facts (arm64jit/src/jit.rs):
`type4_taskv4_vector_has_no_in_code_install_site_and_uses_static_base` and
`json_overflow_leak_reads_guest_stack_pointer_not_seeded_lsm_map`.

Verified: cargo test 493/0 (+2); build clean; productized `play --jit`
re-verified (exit 124, real indexed triangle centroid red + textured quad
BL=RED/BR=GREEN/TR=WHITE/TL=BLUE + quad-loop, swaps Ok(0x1)); bare
`--jni --startapp` still reproduces the abort (harness-bootstrap-only).

Next (unchanged wall, reframed): the engine never self-produces a frame/session
task because task-v4 `[0x6829ea8]` is only populated by real framework producer
glue (absent in-code); the bare-StartApp json abort is NOT fixable by LSM
seeding (guest stack state). Directions: (a) synthesize the framework
task-producer registration the engine's own game-activity/lifecycle init would
perform and call that guest registration path; or (b) advance objective 2b by
exercising the fsmap/SQLite datastore plane with the client's real
serialization — the productized run currently makes ZERO fsmap remaps (engine
never reaches a session), so persistence is proven only hermetically
(SH38–SH42), not by the live client.

## SH43b (Sep 12, 2026): also prove the legacy `gethostbyname` resolution path — the client imports both APIs. `gethostbyname("localhost")` returns a static thread-local `hostent` (h_addrtype@16/h_length@20/h_addr_list@24), a differently-shaped result than getaddrinfo; the regression walks it to an AF_INET 127.0.0.1. Workspace 491/0 (was 490/0). Commit 75d9d31.

## SH43 (Sep 12, 2026): prove the guest DNS plane through the real ABI — `getaddrinfo("localhost") → ai_addr → connect → send/recv` roundtrip to a real host TCP peer. Workspace 490/0 (was 489/0). Commit 23f4ff4.

A logged-in session's FIRST network action is hostname resolution (`getaddrinfo`)
BEFORE any `connect`. SH42b proved `socket/connect/sendto/recvfrom` only against a
hardcoded loopback IP; the resolution step was unproven. `getaddrinfo` is a libc
JUMP_SLOT import the resolver binds to HOST glibc via `dlsym` (not a raw syscall), so
the plane rides the resolver, not `guest_svc`.

New hermetic regression `guest_dns_getaddrinfo_resolves_hostname_then_connect_roundtrip`
(crates/arm64jit/src/resolver.rs): resolves the `getaddrinfo`/`freeaddrinfo` slots,
drives a guest `blr x16` with (node="localhost", service=<live-port>, hints=NULL, &res),
walks the returned aarch64-LP64 addrinfo chain (ai_family@4/ai_len@16/ai_addr@24/
ai_next@40), asserts localhost is an AF_INET sockaddr exactly 127.0.0.1, feeds
ai_addr/ai_addrlen into guest_svc socket(198)/connect(203), roundtrips a login payload
to a real host TCP listener (gets PONG), and frees via the guest's own freeaddrinfo.
Closes the last network-plane gap between "socket works" and "a session reaches a real
Roblox API host."

Re-verified the productized deliverable end-to-end (runs/sh43-play-jit.txt):
`open-sober play --apk roblox-android.apk --jit` extracts the real libroblox.so, drives
JNI_OnLoad → StartApp → render-init → engine geometry path through the JIT GLES bridge:
real indexed triangle (centroid red), textured quad (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE
exact texels), 6 fresh quad-loop frames, swaps Ok(0x1), exit 124. Boot fully-wired: 534
JUMP_SLOT bound (0 unbound), zero ENOSYS/unhandled hostcalls.

**Next (closest unblocked):** network + data + DNS planes all proven through the real
ABI. The framework task-producer wall (SH14/SH41/SH42) stands; a GPU host remains the
environment for the final self-driven-login / frame-performance proof.

## SH42b (Sep 12, 2026): proved the client-side network plane end-to-end — guest `socket(198)/connect(203)/sendto(206)/recvfrom(207)` roundtrip a login payload to a REAL host TCP peer. Workspace 489/0 (was 488/0). Commit 5cc3dd8.

The existing `socketpair(199)+sendmsg/recvmsg` test proved only a pre-connected
pair. A logged-in session's TLS/HTTPS stack (bionic+boringssl inside the guest)
funnels byte I/O through socket→connect→send/recv to an EXTERNAL peer, so the
real `guest_svc` ABI is now driven against a host `TcpListener` on loopback:
socket, connect(127.0.0.1:ephemeral), sendto("SESSDATA\n"), recvfrom("PONG"
echo), close — asserting the connected stream roundtrips exactly as a real
session's HTTPS would. Test-fix: `sin_addr.s_addr` is network-order (portable
`htonl(INADDR_LOOPBACK)` form, not naive `from_be_bytes` which made 127.0.0.1
read as 1.0.0.127 → connect ETIMEDOUT).

No prod-code change (all four syscalls were already handled); this pins the full
CLIENT network path as drop-through-functional. Real boot re-verified unchanged
in SH42. 

**Next (closest unblocked):** data-plane (SQLite lifecycle incl. preadv/pwritev/sync)
and the client network plane are both proven. Standing structural wall (SH14,
re-confirmed SH41/42): the engine never self-produces a render task (w4=4 cap),
so frames are harness-driven. Directions: (a) drive the confirmed-live
deque-maintenance globals (0x1068262e8/300/308) via --deque-node-live; or
(b) further harden the Android-framework JNI path a logged-in session touches
when it reads/writes its store.

## SH42 (Sep 12, 2026): closed the last data-plane syscall gap — vectored positional I/O + durability (preadv(69)/pwritev(70)/sync(81)) now handled in guest_svc. Workspace 488/0 (was 487/0). Commit 7351654.

A real SQLite-backed session datastore flushes db/shm pages with **pwritev**
(batched vectored positional write) and reads them back with **preadv**; it
issues **sync(81)** (PRAGMA synchronous=FULL) before reporting a transaction
durable. All three previously fell through to -ENOSYS — the last unhandled arm
in the data-plane for a session store. Added beside the pread/pwrite (67/68) and
readv/writev (65/66) arms:

- preadv(69)/pwritev(70): `struct iovec` is byte-identical across aarch64/x86-64,
  so a raw forward writes the guest iovec array in place; the aarch64 loff_t
  pos as two syscall words (a[3]=lo, a[4]=hi) maps onto x86-64's __NR3264 form.
- sync(81): trivial host `libc::sync()` (returns ()), 0 on success.

New hermetic regression
`fsmap_preadv_pwritev_sync_support_sqlite_durability_path` (tests/fsmap_persist.rs)
drives the real guest_svc ABI under a configured root: pwritev writes two pages
at distinct offsets into the store, sync returns 0 (not -ENOSYS), and after
close+reopen preadv reads page1 back byte-exact from the store across a fresh
fd. This completes the SQLite lifecycle data-plane: create → write → statx-exists
→ flock → fallocate → truncate → preadv/pwritev → sync → readlink → read.

Real boot re-verified through the modified dispatch (runs/sh42-boot-reverify.txt):
exit 124 stable, real indexed triangle centroid RGBA(255,0,0,255), textured quad
BL=RED/BR=GREEN/TR=WHITE/TL=BLUE exact texels, 3 fresh quad-loop frames, swap
Ok(0x1). Baselines unchanged (--jni exit 0; stable idle exit 124).

**Next (closest unblocked):** the data-plane now covers the full SQLite session-datastore
lifecycle. The engine's own main-loop producer still never enqueues a render-task type
(SH14 w4=4 structural cap; SH41 re-confirmed the deque-maintenance dispatch reaches real
engine code but never a render producer), so frames are harness-driven. Two directions:
(a) use the confirmed-live maintenance globals + --deque-node-live to probe whether a
maintenance dispatch advances the session past idle; or (b) harden the JNI/network
surface the client touches once a real session reads/writes its now-persistent store.

## SH41 (Sep 12, 2026): fixed a real faccessat(48) arg-order + remap bug in guest_svc (data-plane hardening for the datastore-accessibility probe), and corrected a stale documented premise (the deque-maintenance BSS globals ARE populated at runtime). Workspace 487/0 (was 486/0). Commit e08ddee.

aarch64 raw `faccessat(48)` is `(dirfd, pathname, mode)` — x0=dirfd, x1=pathname,
x2=mode. The old handler mapped `a[0]` (the dirfd, e.g. AT_FDCWD=-100) as a
pathname C-string and passed the real pathname pointer (truncated to c_int) as
the mode — so any guest "is my /data session file there?" accessibility probe
read garbage against the host root. Fix: dirfd=a[0], pathname=a[1] remapped,
mode=a[2] (same arg-order class as SH40's readlinkat fix). Regression
`fsmap_faccessat_uses_true_pathname_and_remaps_into_store` drives the real
guest_svc ABI: R_OK/W_OK on an existing store prefs.xml pass, a missing store
path is -ENOENT (store-index resolution, not host-root), and a relative probe
resolves against a real openat(O_DIRECTORY) store dirfd (proves dirfd honored,
not hardcoded AT_FDCWD).

Corrects SH39b/SH14: a live JIT_FRAMEWORK_DUMP under the stable boot shows the
three deque-maintenance BSS globals hold real .text addresses
(0x1068262e8=0x10620db24, 0x106826300=0x102176bfc, 0x106826308=0x1022199e0) —
NOT "statically 0 / not host-drivable." The targets are thin bionic/atrace-ish
upkeep functions (not render producers), and the drain still hardcodes w4=4
(maintenance), so the structural "engine never self-produces a render task" wall
stands — but future cycles should treat those globals as live, re-opening the
--deque-node-live path (SH13). Verified: cargo test 487/0; build clean (only
pre-existing warnings); full real-boot render reproduces (exit 124, real
triangle centroid RGBA(255,0,0,255) + textured quad exact texels, 4 fresh
quad-loop frames, swap Ok(0x1), zero ENOSYS/unhandled). Doc
docs/frontier-sh41-faccessat-fix.md; run-log runs/sh41-boot-render-verify.txt.

## SH40b (Sep 12, 2026): added flock(32)+fallocate(285) to guest_svc (SQLite datastore concurrency/preallocation) and dropped a dead duplicate truncate arm. Workspace 486/0. Commit bd00e88.

The real client's datastore is SQLite-backed: it takes advisory file locks
(flock) on its db/shm files for read/write concurrency and preallocates space
(fallocate) when growing mmap-backed db files. Both previously hit -ENOSYS.
Added flock -> host advisory lock and fallocate -> SYS_fallocate; removed a dead
duplicate truncate(45) arm left at the durability block (the remapped arm added
earlier in SH40 runs). Extended the fsmap meta test: reopen a store file, flock
LOCK_EX|NB succeeds, fallocate grows the host file to >=4096, LOCK_UN releases.
Combined with SH40, the persistent store now survives the full SQLite-style
life: create, write, statx-exists, flock, fallocate, truncate, readlink, read.
Verified the whole product boot+render still reproduces with zero
ENOSYS/unhandled syscalls after both commits (runs/sh40-boot-render-verify.txt:
real triangle + 6 sustainable textured-quad frames, exit 124).

## SH40 (Sep 12, 2026): completed the fsmap data-plane path coverage — statx/statfs/truncate/chdir/linkat/symlinkat/readlinkat now remap into the persistent store, plus readlinkat AND symlinkat arg-order bug fixes. Workspace 486/0 (was 484/0). Commit 56d7697.

The SH38 data plane remapped openat/mkdirat/unlinkat/renameat/faccessat/newfstatat,
but the remaining path-taking syscalls a real session's datastore touches were
forwarded raw against the host root — a guest `/data/...` path ENOENTed. Most
critically **statx(291)**: bionic/Java answer "does my session file exist" there,
so a wrong statx makes the client think its store is gone. This cycle routes the
rest of the path-taking syscalls through `remap_path` into the store:

- `statx(291)` (dirfd a0=AT_FDCWD for absolute guest paths; `struct statx` is
  asm-generic/byte-identical, raw forward writes the guest buffer),
- `statfs(43)`, `truncate(45)`, `chdir(49)`, `fchmodat(53)`, `fchownat(54)`,
  `linkat(37)`, `utimensat(88)`, `readlinkat(78)`.
- **readlinkat arg-order BUG fixed**: the old handler passed the dirfd (a0) as the
  pathname with a hardcoded AT_FDCWD, so any real guest readlinkat failed; now
  dirfd=a0, pathname=a1.

2 new hermetic regressions (tests/fsmap_persist.rs, drive guest_svc real ABI, no
APK): (1) statx reads the store's real stx_size for an existing session file and
returns -ENOENT for a missing one; statfs on guest `/data` succeeds. (2) truncate
shrinks the mapped host file, chdir lands in the store, linkat hard-funds a link
in the store, readlinkat resolves a store symlink (and -ENOENT for missing) —
which doubles as proof the arg-order fix works. Hermetic proof of the full
persistence path. Next: the standing producer/deque wall (SH39b) or further
path-taking syscall hardening as the real client surfaces them.

## SH38 (Sep 12, 2026): closed the data-plane FS gap — guest file paths under Android's writable roots now remap to a real persistent host store, so the client's datastore/login session can persist "like the real app" (objective 2b enabler). Workspace 482/0 (was 479/0).

New `arm64jit::fsmap` module + syscall wiring: the JIT's `guest_svc` was passing
guest file-path pointers verbatim to host libc, so a real session's
`/data/data/com.roblox.client/...`, `/sdcard/...`, `/storage/emulated/0/...`,
`/cache/...` reads/writes hit the host root and failed ENOENT/EPERM — the client
could not persist anything. Now, when a host root is configured
(`SOBER_ANDROID_ROOT` / test setter), those writable Android roots remap to
`{root}/...` and parent dirs are scaffolded for O_CREAT/mkdir. Input-off by
default → existing boot untouched (verified: JNI_OnLoad 0x10006, StartApp, stable
idle). Wired: openat/mkdirat/unlinkat/renameat/faccessat/fstatat.

Hermetic proof (`tests/fsmap_persist.rs`, no APK): a guest write under
`/data/user/0/com.roblox.client/files/session.dat` through the real `guest_svc`
ABI lands in a real host file, and a *fresh* CpuState ("restart") reopens the
same guest path and reads the exact bytes back — the store survives a restart.
Doc docs/frontier-sh38-fsmap-persist.md.

This is the data-plane persistence enabler (objective 2b). The standing
structural frontier (SH14/SH37) is unchanged: the engine's own main-loop producer
never enqueues a render task, so the engine renders what the harness drives.

**SH38b (ef0cf2b):** elfjit real runs now ARM the persistence root — create +
export `SOBER_ANDROID_ROOT` (stable host dir under XDG/HOME data) so the client's
`/data`/`/sdcard`/`/cache` writes reach the fsmap store. Verified SAFE: with the
root armed the full real boot (JNI_OnLoad 0x10006, StartApp, `--kicker
0x106863af8` lifecycle pulse) still exits 124 stable (runs/sh38-fsmap-boot-armed-
default.txt); the root dir is created. Root stays empty while idle (the idle main
loop does no `/data` I/O yet) — the remap gains effect when a session reads/writes
its store. Note: an early run that "crashed" (json-Writer overflow, exit 139) was
a red herring — it was the `--kicker` flag being omitted, NOT the remap; with the
canonical flags both armed and unarmed boots are stable exit 124.

## SH37 (Sep 12, 2026): the SH35-sealed GLES3 pipeline slots are now proven FUNCTIONAL, not just resolvable — dispatched through the engine's OWN slot stubs on the live context. Workspace 479/0 (was 478/0). Commits a0ba81c (+8f57 ledger).

New elfjit `--renderframe-progbin` drives the engine's dispatch stubs `0x5b3a1c0+0xc*N`
(the exact `adrp x8,6d3b000; ldr x3,[x8,#752+8N]; br x3` mechanism a real session's frame
dispatches through) with real guest-ABI args on the live Mesa-llvmpipe context. All clean,
glGetError NO_ERROR throughout, exit 124 stable, coexisting with the standard render path
(geometry wrapper Ok, textured-quad exact texel readbacks, triangle glDrawElements drawn,
swaps Ok(0x1)):

- slot15 glProgramParameteri(GL_PROGRAM_BINARY_RETRIEVABLE_HINT=0x8257) pre-link.
- slot13 glGetProgramBinary -> a REAL 3498-byte Mesa binary (format 0x875f) — Mesa
  produced a retrievable program binary THROUGH the sealed slot.
- slot14 glProgramBinary re-upload accepted (err 0x0).
- slot5  glBindBufferBase(GL_UNIFORM_BUFFER,0,real_gen_buf) binds a UBO (err 0x0).
- slot10 glDrawArraysInstanced(GL_TRIANGLES,0,0,3) dispatches clean (err 0x0).

Corrected the slot-stub addresses to GUEST vaddrs (0x105b3a... not the .so file vaddrs
0x5b3a..., +0x100000000). New hermetic regression
`sealed_gles3_ubo_and_instanced_slots_dispatch_real_mesa_clean`: on a surfaceless ES3
context it drives glBindBufferBase + glDrawArraysInstanced through resolve_gles_int and
asserts GL_NO_ERROR (a mis-bridged slot would error or crash). Live log
runs/sh37-progbin-full.txt. This closes the SH35 "prove they run, not just resolve" step.

Next: the render harness has now proven every GLES dispatch slot 0-15 is both bridged AND
functionally dispatchable, plus real frames (solid/triangle/textured-quad/grid, ETC1/
ETC2/ASTC compressed textures) through the engine's own geometry wrapper and swap.
Remaining structural frontier (unchanged, SH14-documented): the engine's own main-loop
producer still never enqueues a render task — frames are harness-driven on a time base
from a detached thread. The path to a fully self-driven boot is either re-opening the
producer/deque wall or wiring sober-core's `open-sober play --apk` to reproduce this
elfjit boot+render automatically.

## SH36 (Sep 12, 2026): sealed the LAST raw clear-dispatch gap — slot 3 (guest BSS 0x106d3b308) now resolves as glClearBufferfi through the MIXED (float) GLES bridge, not glClearStencil. Workspace 478/0 (was 477/0). Commit ea3e692.

Disasm of the real clear-state sub-fn 0x5b32ef4 (the per-buffer COMBINED depth+stencil
clear): `mov w0,#0x84f9` (GL_DEPTH_STENCIL), `ldr s0,[x21,#68]` (depth -> s0, the FIRST
FP arg), `ldr w2,[x21,#72]` (stencil), `mov w1,wzr` (drawbuffer), then bl the slot-3
stub 0x5b3a1e4 (`adrp x8,6d3b000; ldr x3,[x8,#776]` = guest 0x106d3b308). Because depth
is a FLOAT, glClearBufferfi is a mixed-ABI function — the integer HostCall only
marshals x-regs and would DROP the s0 depth (mis-clearing the depth attachment). The
pre-SH36 seed put glClearStencil (single-int-ABI) on slot 3, which a real
GL_DEPTH_STENCIL dispatch would mis-route (float depth ignored, none of the 3 int args
decode as a mask).

Fix: new `w_glClearBufferfi` mixed wrapper (AAPCS: depth from gs_f(s,0), buffer/
drawbuffer/stencil from gs_x(s,0/1/2)) registered in `gles_mixed_wrapper`, so
w_eglGetProcAddress auto-heals the engine table; `resolve_gles_int` correctly rejects
it (float ABI). elfjit `--renderframe-seedgles` slot 3 corrected
glClearStencil->glClearBufferfi.

Verified live (runs/sh36-clearbufferfi.txt): slot 3 guest 0x106d3b308 pre-seed
`0x7f0000018050` + seed `0x7f0000018058` (both bridge), textured-quad/triangle draws +
all swaps Ok(0x1), exact texel readbacks (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE), exit 124
stable. New regression `resolve_gles_mixed_clearbufferfi_is_mixed_abi_not_int`.
This closes the SH19/SH24 raw-Mesa dispatch class for the CLEAR path entirely — every
one of slots 0-15 a real frame can dispatch now routes through our bridge.

## SH35 (Sep 12, 2026): the engine's REAL GLES3 dispatch-slot table is no longer raw-Mesa — UBO / instanced / program-binary slots now resolve through the JIT bridge. Workspace 477/0 (was 476/0). Commit 51e336f.

SH28's live snapshot showed the engine's own GL-init fills its GLES dispatch table
(BSS 0x106d3b2f0 + 8*N) slots 4-8 (glUniformBlockBinding / glBindBufferBase /
glBindBufferRange / glGetUniformBlockIndex / glGetActiveUniformBlockiv), 9/10
(glDrawElementsInstanced / glDrawArraysInstanced) and 13-15 (glGetProgramBinary /
glProgramBinary / glProgramParameteri) with raw-Mesa host addresses — the SH19/SH24
crash class (a guest `br` through the 0x5b3a1c0+0xc*N stub jumps out-of-image). A
real self-driven engine frame dispatching those would crash. Added all ten to
`resolver::GLES_INT_NAME_LIST` (pure int/ptr ABI, ≤8 args; rejected by mixed).
Since the engine builds its table via `eglGetProcAddress` (SH3 interception →
resolve_gles_int), the table now auto-heals to bridge slots. Verified live
(runs/sh35-pipeline-slots.txt, exit 124 stable): PRE-SEED snapshot shows all ten
as `0x7f000000…` bridge slots (SH28 showed raw `0x7f44…` Mesa). Render path
unchanged (geometry wrapper Ok(0x0), post-draw swap Ok(0x1), 4×4 grid 16/16 cell
readbacks match texels). New regression
`gles3_pipeline_names_resolve_via_int_bridge_for_engine_draw_slots`.
Doc docs/frontier-sh35-gles3-pipeline-slots.md.

## SH34 (Sep 12, 2026): the coherent renderer scales to a REAL LARGER MESH. New `--renderframe-grid <N>` fabricates an N×N grid of textured quads (independent per-cell, each a distinct texel color at the interpolated vertex UV) driven through the REAL libroblox.so's OWN geometry wrapper 0x5b35288. Verified N=3 (9/9), N=4 (16/16), N=6 (36/36) cell-center glReadPixels readbacks ALL match each cell's exact texel color (±1 rounding): 6×6 = 144 interleaved verts / 216 idx drawn in one call through engine primitive-setup + indexed glDrawElements, wrapper Ok(0x0), swap Ok(0x1), exit 124 stable; sustainable (quad-loop 20 iters all Ok(0x1), fresh mesh each frame). Captures runs/sh34-grid.{txt,mp4}; reproducible runs/capture_grid.sh. Fixed real buffer-overlap bugs (VBO/EBO -> 0x2000/0x4000, grid tex -> 0x6000; the old 0xc00/0xf60 clobbered for N≥4/8). Harness-only; single-quad mode (4 distinct checkerboard readbacks) + --jni boot baseline (JNI_OnLoad Ok(0x10006)) unchanged. Doc docs/frontier-sh34-grid.md. Workspace 476/0. Commit fa1839f.

## SH33 (Sep 12, 2026): SUSTAINABLE TEXTURED real-geometry rendering. `--renderframe-quad-loop <N>` re-drives clear(cycling 5-color bg) -> engine geometry wrapper 0x5b35288 -> swap N times after the single textured-quad proof frame. 6 iters all drew+swap Ok(0x1), 5 distinct cycling backgrounds, textured readback intact (BL=RED/BR=GREEN/TR=WHITE/TL=BLUE); 8-frame x11grab runs/sh33-quad-loop.mp4. Textured recipe now renders correctly AND sustains. Workspace 476/0; baselines unchanged. Commit 9f1a409.

## SH32 (Sep 12, 2026): the engine's OWN geometry path renders a REAL ASTC texture (`--renderframe-astc`, GL_COMPRESSED_RGBA8_ASTC_4x4=0x93B0 — the load-bearing format desktop GL can't native-decode). 4 Khronos LDR void-extent blocks uploaded via glCompressedTexImage2D; bridge decodes via decode_astc; FS maps DECODED ALPHA to RGB gray-scale -> readback BL=RGBA(255,255,255,255) BR=190 TR=64 TL=128 = the 4 exact ASTC alphas; runs/sh32-astc.{png,rgb} = 4 gray quadrants. New regression astc_ldr_void_extent_blocks_decode_expected_color_and_alpha. Compressed live-prove now ETC1+ETC2-RGB+ETC2-RGBA8/EAC+ASTC. Workspace 476/0. Commit 25f18d7.

## SH31 (Sep 12, 2026): the engine's OWN geometry path renders a REAL ETC2-RGBA8/EAC texture (`--renderframe-etc2a`, 0x9278). 8x8 = 4 blocks (EAC alpha sub-block + SH29-proven ETC2-RGB); uploaded via glCompressedTexImage2D; bridge decodes via decode_etc2_rgba8. FS maps DECODED ALPHA to RGB gray-scale -> readback BL=RGBA(255,255,255,255) BR=190 TR=64 TL=128 = the 4 exact EAC alphas; runs/sh31-etc2a.{png,rgb} = 4 gray quadrants. New regression etc2_rgba8_eac_solid_blocks_decode_expected_alpha. Workspace 475/0. Commit 3fafcc4.

## SH30 (Sep 12, 2026): REAL TWO-ATTRIB TEXTURED QUAD renders through the engine's OWN geometry wrapper (`--renderframe-quad`). primitive-setup's multi-primitive loop sets up TWO vertex attribs (aPos @format[3]{4,GL_FLOAT} off0, aUV @format[1]{2,GL_FLOAT} off16) on an interleaved [pos.xyzw, uv.xy]x4 VBO (stride 24, 6-idx EBO). FS samples a 2x2 checkerboard at the REAL interpolated vertex UV -> BL=RED/BR=GREEN/TR=WHITE/TL=BLUE. Captured runs/sh30-quad.{rgb,png} = 4 near-equal color quadrants, exit 124. Doc docs/frontier-sh30-quad.md. Workspace 474/0; baselines unchanged.

## SH29 (Sep 12, 2026): the engine's OWN geometry wrapper renders a REAL ETC2 texture (`--renderframe-etc2`, GL_COMPRESSED_RGB8_ETC2=0x9274 — the actual Android Roblox format). Same hand-crafted blocks as SH27 relabeled ETC2; bridge decodes via decode_etc2_rgb. Readback pixel-identical to ETC1 (WHITE/GREEN/RED distinct). Captured runs/sh29-etc2.{rgb,png}. Regression extended. Workspace 474/0; baselines unchanged.

## SH28 (Sep 12, 2026): captured the real engine GLES dispatch-table content live. --renderframe-seedgles dumps all 16 raw slots (BSS 0x106d3b2f0+8*N) before seeding and dladdr-resolves each. The engine's REAL renderer dispatch is a modern GLES3 pipeline (slots 4-8 = UBO/buffer, 9/10 = instanced draws, 13-15 = program binary), NOT the simple clear/draw map. Slots 0-2 read as our bridge slots, confirming SH3's eglGetProcAddress interception reaches the engine's own table. Diagnostic-only. Workspace 474/0; baselines unchanged.

## SH27 (Sep 12, 2026): the engine's OWN geometry wrapper renders a REAL COMPRESSED-ETC1 texture. `--renderframe-etc` uploads a hand-crafted 8x8 ETC1 texture (4 solid blocks) via glCompressedTexImage2D; the GLES bridge decompresses ETC1->RGBA (texture-codec) and re-uploads. Three on-triangle probes read back distinct decoded colors (WHITE/GREEN/RED); the rounded channels match the hand-computed (c*0x11)+2 prediction exactly, proving the decode ran. Captured runs/sh27-etc.{png,rgb}: triangle interior 4-colored on clear-blue, exit 124. New regression `crafted_etc1_solid_blocks_decode_to_expected_colors`. Workspace 474/0. Baselines unchanged (--jni exit 0, idle 124).

## SH26 (Sep 12, 2026): the engine's OWN geometry wrapper renders a REAL TEXTURED triangle — a 2x2 RGBA checkerboard sampled by a textured fragment shader, with every texture/uniform/shader call (glGenTextures/glBindTexture/glActiveTexture/glTexImage2D/glTexParameteri/glGetUniformLocation/glUniform1i) dispatching through the JIT GLES bridge. New `--renderframe-tex` lever. Three on-triangle probes read back three DIFFERENT colors (WHITE/GREEN/RED), proving the sampled texture rendered (not solid). Captured runs/sh26-tex.{png,rgb}: triangle interior 4-colored on clear-blue, exit 124. glTexImage2D's 9th arg (pixels) rides the guest stack; PLT leaf stub + fake sp. Fixed GLSL ES 1.00 `precision mediump float;` (needed for a local vec2). Workspace 473/0; HEAD (this commit). Baselines unchanged (--jni exit 0, idle 124).

## SH25 (Sep 12, 2026): the coherent renderer RENDERS a REAL VISIBLE triangle — and the engine's OWN geometry wrapper does it. A real indexed glDrawElements draws 415,696 red px (45.11% of frame) captured from the real X11 window. Workspace 473/0; HEAD 035ff6a (then fd5227b for the sustainable-loop follow-on).

SH25b (fd5227b): `--renderframe-triangle-loop <N>` — the same bind → clear →
coherent-draw → swap recipe now runs SUSTAINABLY on the detached host thread,
cycling the clear background through a 5-color palette per frame. Verified 6
iterations all `drew+swap Ok(0x1)` with 5 distinct backgrounds and red triangle
pixels in every captured frame (exit 124 stable). This is the geometry analog of
SH23's clear-only --rendersustain: the sustainable real-geometry-frame property a
real main-loop frame drive needs. Artifact: runs/capture_triangle_loop.sh.

The SH24 frontier ("feed primitive-setup 0x5b353d0 a real primitive list so the
draw wrapper produces a real rendered triangle") is CROSSED. New elfjit lever
`--renderframe-triangle` fabricates a *coherent* engine renderer — real
1-primitive list, inline vertex-descriptor table at renderer+0x48, stride table,
IBO — plus REAL GL resources created/uploaded through the JIT GLES int bridge:
a compiled+linked shader program (vertex aPos → gl_Position, fragment solid
red), a real VBO (3 × vec4 NDC triangle) and a real EBO (indices 0,1,2). Driving
the engine's own geometry wrapper 0x5b35288 then dispatches a real indexed
glDrawElements(GL_TRIANGLES, 3) that actually RENDERS:

- Readback: `centroid(640,360)=RGBA(255,0,0,255)` (red triangle interior).
- Capture runs/sh25-triangle.{png,rgb}: 415,696 px exact RED (255,0,0) = 45.11%
  of the 1280x720 frame on the 0,0,0.3 clear-blue background — a clearly visible
  triangle-shaped mask (apex → base).
- `geometry wrapper 0x5b35288 returned Ok(0x0)`, `post-draw swap Ok(0x1)`,
  exit 124 stable, zero crash.

Three real bugs found+fixed en route (each had rendered nothing/collapsed):
1. **Format-table index.** The fabricated primitive's `[prim+8]`=5 selected
   format[5] = {size4, **GL_SHORT**=0x1402}, so the engine's glVertexAttribPointer
   misread float verts as shorts → degenerate/clipped. Fix: `fmt_index=3` =
   format[3] = {size4, **GL_FLOAT**=0x1406} (verified: glVertexAttribPointer now
   dispatches type=0x1406). With this the RAW engine path (no reference draw)
   renders the full triangle.
2. **Wrapper count register.** glDrawElements' COUNT rides in the wrapper's 4th
   drive arg (w20 → `mov w1,w20`), NOT x5 as SH24's comment said; an empty draw
   resulted. Plus `glViewport/glScissor(0,0,1280,720)` must be set or a stale
   0-size viewport rasterizes nothing.
3. **glGenBuffers out-pointer aliasing.** Writing the generated buffer id into
   the same memory as the vertex data clobbered the verts → used dedicated id
   slots.

`SH25_REF=1` retains a direct-draw reference (own glVertexAttribPointer) as an
opt-in cross-check; it now defaults OFF since the engine path renders.

Reproducible: runs/capture_triangle.sh; run-log runs/sh25-triangle.txt; verify
the engine-only path (SH25_REF absent) prints
`readback centroid=RGBA(255,0,0,255)` + the capture shows the red triangle.
Doc: docs/frontier-sh25-triangle.md. Baselines unchanged: `--jni` clean exit 0;
stable idle exit 124.

SH25b (fd5227b): `--renderframe-triangle-loop <N>` makes the same bind → clear →
coherent-draw → swap SUSTAINABLE on the detached host thread, cycling the clear
bg through 5 colors/frame; verified 6 iters all Ok(0x1), red triangle in every
captured frame. Artifact runs/capture_triangle_loop.sh. This is the geometry
analog of SH23's --rendersustain — the sustainable real-geometry property a real
main-loop drive needs.

**Next (closest unblocked):** GLES slots 11+ (texture / uniform / shader
dispatch) + compressed-texture (ETC2/ASTC) interception so a *textured/shaded*
draw renders, then scale the (now fully reversed) coherent-renderer rotation
onto a larger real mesh. The engine's own main-loop producer still never enqueues
a render task (the long-standing structural wall), so the harness drives its own
code on a time base. Baselines unchanged: `--jni` exit 0; stable idle exit 124.

---

## SH24 (Sep 12, 2026): the engine's OWN real geometry draw path now dispatches glDrawElements through the GLES bridge. Workspace 473/0; HEAD e358df0.

Completed the GLES dispatch-slot map to the geometry path and PROVED a real
geometry draw dispatch fires through the bridge. The clear-only frame-fn
(0x105b32c00) never touches geometry — the engine's real draw is wrapper
0x5b35288 (calls primitive-setup 0x5b353d0 to bind buffers + set vertex attrib
pointers via direct @plt, then dispatches the indexed/array draw through GLES
dispatch-table slots **9/10 = glDrawElements/glDrawArrays**). Those extended
slots held raw-Mesa addresses (SH19 bug class), so a driven draw would have
jumped out-of-image.

Fixes (commit e358df0):
- seedgles now seeds slots by explicit (slot,name) map: 0-7 clear (SH22 names)
  + **9=glDrawElements, 10=glDrawArrays** (draw), all through the integer bridge.
- New regression `draw_slots_gl_draw_elements_arrays_resolve_via_int_bridge`.
- New elfjit lever `--renderframe-drawprobe`: drives the engine's own wrapper
  0x5b35288 with a fabricated minimal renderer (empty primitive list →
  primitive-setup returns fast; nonzero [renderer+120] index-buffer obj + w5=3
  count → INDEXED path) and **dispatches a real glDrawElements through the bridge**:

```
[elfjit:renderframe-seedgles] slot 9 (glDrawElements) <- bridge 0x7f0000002a38
hostcall@glBindBuffer        pc=0x7f00000029f8 x0=0x8893 x30=0x105b35550
hostcall@glDrawElements      pc=0x7f0000002a38 x0=0x4 x2=0x1405 x30=0x105b352f8
[elfjit:renderframe-drawprobe] geometry wrapper 0x5b35288 returned Ok(0x0)
[elfjit:renderframe-drawprobe] post-draw swap returned Ok(0x1)
```

exit 124 stable, zero crash. First-ever real-geometry draw through the bridge
(analogous to SH17/22 for clear). Reproducible: runs/capture_drawprobe.sh;
run-log runs/sh24-drawprobe.txt. Doc: docs/frontier-sh24-draw-slots.md.

Honest scope: the fabricated renderer is EMPTY (no real mesh/buffer/VAO), so
this proves the DRAW DISPATCH is bridge-functional, not the render of real
geometry. Next wall: feed primitive-setup 0x5b353d0 a coherent primitive list
+ vertex buffers (the renderer C++ object reverse). Slots 11+ (texture/
uniform/shader) still unseeded. Baselines unchanged (--jni exit 0; idle 124).

---

## SH23 (Sep 12, 2026): the engine's OWN render recipe (bind -> frame-fn 0x105b32c00 -> post-frame swap) now runs CONTINUOUSLY as a live animated render loop via new elfjit `--rendersustain <fps>`. Workspace 472/0; HEAD b692077.

160 consecutive frame-fn->swap pairs, every `frame-fn returned Ok(0x..)` +
`post-frame swap returned Ok(0x1)`, exit 124 stable, zero crash/heap abort —
driven on a detached host thread concurrent with StartApp's idle main-loop
jit_run. A real x11grab recording (runs/sh23-sustain-loop.mp4) proves every
frame is a **fresh render**: majority pixel color tracks the per-frame 5-color
palette exactly (green->red->blue->yellow->magenta; float fracs match to 3 dp),
12+ distinct frames over 6 s. Each iteration re-seeds both engine clear-color
sources (frame-fn 5th-arg clear-state obj at base+0x400 [+4..16], 6th-arg
color-source obj at base+0x500 [+0..16]) before calling the real frame-fn, so a
capture cannot be a static buffer. This is the sustainable/reentrant property a
real main-loop frame drive needs.

- New lever: `--rendersustain <fps>` (unbounded; `--renderframe-loop <N>` bounds
  only when --rendersustain absent).
- Reproducible: `runs/capture_sustain_loop.sh` (FPS=2 CAP_SECS=8). Run-log:
  runs/sh23-sustain-loop.txt (160 iterations); video: runs/sh23-sustain-loop.mp4.
  Doc: docs/frontier-sh23-sustain-render.md.
- Baselines re-verified unchanged: `--jni` exit 0; stable idle exit 124.

Honest framing (unchanged shape): still harness-driven — fabricated renderer/
view/clear-state objects, clear-only frame (real glDrawElements draw path gated
on a coherent renderer C++ object not yet reversed). The engine's own
main-loop producer still never enqueues a render task, so it does not call
frame-fn by itself yet — we drive its own code on a time base. But the GLES
dispatch-slot map is complete and the engine's GL path is proven
bridge-functional AND sustainable: the two properties a real main-loop frame
drive needs. SH24 supersedes the clear-gate note with a real geometry-draw
dispatch proof.

---

## SH22 (Sep 12, 2026): BROKEN THE SH21 WALL — the engine's OWN frame-fn 0x105b32c00 now presents a real, correctly-colored full frame through the GLES bridge. Workspace 472/0; HEAD (this commit).

SH21 left the window black, misattributed to "the per-buffer clear loop clears
depth-style buffers via a guessed slot2=glClearDepthf". Disassembly of the
clear-state sub-fn 0x5b32e08 corrects it: **0x1800=GL_COLOR / 0x1801=GL_DEPTH
are glClearBufferfv buffer enums** — the loop is
`slot2(0x1800, drawbuffer=i, value=clearstate+4+i*0x10)` over 4 color
draw-buffers; the depth branch is `slot2(0x1801,0,...)`. So **slot2 =
glClearBufferfv** (not glClearDepthf) and **slot0 = glDrawBuffers** (preamble
dispatches {GL_COLOR_ATTACHMENT0..3} / {GL_BACK} arrays). The SH19-21
glClearColor/glClearDepthf seed guesses mis-routed both — glClearDepthf's float
bridge ignored the int/ptr args and cleared nothing. That was the black window.

Fixes: resolver.rs adds glClearBufferfv + glDrawBuffers to GLES_INT_NAME_LIST
(both pure int/ptr ABI); elfjit seed_names set slot0=glDrawBuffers,
slot2=glClearBufferfv; objB[+140]=0 for the glDrawBuffers(1,{GL_BACK}) default-FB
path.

Verified real-binary run (runs/sh22-color-frame-from-engine-framefn.txt):
slot0/2 seed to bridge slots, `frame-fn returned Ok`, `post-frame swap Ok(0x1)`,
exit 124 stable. Frame artifact runs/sh22-color-frame-from-engine-framefn.{png,rgb}:
raw RGB(102,51,242) = (0.4,0.2,0.95) = the exact --renderframe-color; 18430/18432
sampled px at that color; ~0.013% black (window edge). No channel swap (SH21's
script mislabeled grab byte order). Doc: docs/frontier-sh22-color-frame.md.

Honest framing: the frame is still harness-driven (fabricated renderer/view/
clear-state objects, one frame-fn call + manual swap); the engine's real
main-loop producer still never enqueues a render task, so it doesn't drive
frames natively yet. But the mechanical reverse of slot0/slot2 removes the last
guess-blocker in the engine's own clear path. Full slot map (SH22c): slot0=glDrawBuffers, slot1=glClearBufferiv, slot2=glClearBufferfv, slot3=glClearBufferfi; glClearBufferiv added to GLES_INT_NAME_LIST. Sustained-render proof (SH22d): --renderframe-loop 3 repeats the engine's real frame-fn->swap 3x, all Ok(0x1), exit 124 — render path is reentrant. Baselines unchanged.

---



## SH21 (Sep 12, 2026): reverse — frame-fn clear-color object is the 5th arg x4 (SH20's x2 premise WRONG). Fabricating a clear-state x4 object makes the engine's OWN clear-state sub-fn 0x105b32e08 run glColorMask(all-1) + per-buffer clear-dispatch loop + glGetError through the GLES bridge. Workspace 471/0; HEAD (this commit).

SH20 left the drive silently stopping after the GL preamble — no clear ever
fired, window black. Root cause: the frame-fn passes its 5th arg x4 to x20
(`0x105b32c30 mov x20,x4`), gated on x4!=0 AND [x4]!=0 (`0x105b32d44 cbz x20` /
`0x105b32d4c cbz [x4]`), then bl's the clear-state sub-fn 0x105b32e08 which
reads the clear RGBA float4 from [x4+4..16] (`mov x21,x2`; `ldp s0,s1,[x21,#4]`
/ `ldp s2,s3,[x21,#12]`). x4 was unset → the entire clear path was skipped.

New elfjit --renderframe-drive fabricates a clear-state object ([+0]=0xF = w20
per-buffer clear bitmask, RGBA float4 at [+4..20] from new `--renderframe-color`
lever, default 0.4,0.2,0.95,1) and passes it as x4 (+ a 6th-arg x5->x22
color-source obj at base+0x500). JIT_TRACE now shows NEW
hostcalls: glColorMask(all-1) x30=0x105b32e44 + per-buffer clear loop at
0x105b32ec8 (mov w0,#0x1800; bl slot2-stub) iterating 4 bits of w20 + glGetError,
then clean `frame-fn returned Ok` + `post-frame swap Ok(0x1)`, exit 124.
Run-log: runs/sh21-clearstate-x4.txt. Doc: docs/frontier-sh21-clearstate-x4.md.

**Honest remaining wall (visible COLOR frame not yet):** window still black —
the per-buffer clear loop dispatches slot2 (seeded glClearDepthf, a heuristic
guess) with integer 0x1800, clearing depth/stencil-style buffers, not the color
buffer. To get color need: (1) the TRUE function of the slot 0x105b32ec8
dispatches + real slot0/2 names; (2) which w20 bit maps to GL_COLOR_BUFFER; (3)
the second main-fn object at 0x105b32d5c (x22, `ldr q0,[x22]; str q0,[x27]`) —
likely the color-clear source. Baselines: --jni exit 0; stable idle exit 124.

(SH21's wall was broken by SH22: slot0=glDrawBuffers, slot2=glClearBufferfv.)

## SH20 (Sep 12, 2026): resolve_gles_int + resolve_egl now accept trailing-NUL names — ALL 8 engine GLES dispatch slots resolve through the bridge. Workspace 471/0; HEAD 78eca29.

Built on SH19's seedgles lever (only 2 of 8 dispatch slots resolved — the 2 float
slots). Root cause of the other 6 failing: `resolve_gles_int` built its CString
cache-key from the RAW name, so a NUL-terminated caller (elfjit's
`--renderframe-seedgles`, a guest eglGetProcAddress C-string) got None even for
whitelisted int-ABI names (glClear/glViewport/glColorMask/glDepthMask/
glStencilMask/glClearStencil) that Mesa exports. `resolve_gles_mixed` strips the
NUL first (that's why slots 0/2 seeded); the int resolver did not. Same latent
bug in `resolve_egl`. Fix: build the key from the NUL-stripped name. Regression
`resolve_gles_int_accepts_trailing_nul_like_mixed` pins all 8 names.

Real-binary proof (runs/sh20-seedgles-all-slots-ok.txt): the engine's own
frame-fn 0x105b32c00 now dispatches its ENTIRE clear path through the bridge —
all 8 slots <- bridge slots, `frame-fn returned Ok`, `post-frame swap Ok(0x1)`,
exit 124 stable. Also un-skipped a dead graphics gate
`resolve_gles_mixed_float_and_stack_abi_execute_real_mesa` that silently SKIPPED
its whole body for its life — it now genuinely runs a surfaceless EGL->ES3->GLES
chain through the JIT bridges and passes (fixing i32 attrib arrays, pbuffer
surface for a queryable buffer, and a Mesa-surfaceless GL_INVALID_ENUM
glGetFloatv state-query -> replaced with a real glClear+glReadPixels check).

## SH19b (Sep 12, 2026): the engine's OWN frame-fn 0x105b32c00 now RETURNS Ok through the GLES bridge. Workspace 470/0; HEAD 5c72e22.

New `--renderframe-seedgles` overwrites the 8 engine GLES dispatch slots (BSS
0x106d3b2f0..0x106d3b328, loaded via `adrp x8, 6d3b000; ldr xN,[x8,#752+8*k]`,
which held raw-Mesa addresses) with host-thunk GLES bridge slots
(resolve_gles_mixed, fallback resolve_gles_int). Seeding slot0 (glClearColor) +
slot2 makes the frame's clear-state sub-fn 0x5b32e08 dispatch through the bridge
so frame-fn returns cleanly: `engine frame-fn 0x105b32c00 returned Ok(...)` +
post-frame swap Ok(0x1), exit 124 stable. Also fixed a shutdown heap corruption
by growing the fabricated renderer scratch 512B -> 8KiB.

## SH19 (Sep 12, 2026): pinned the frame-fn 0x105b32c00 clear-path wall to the engine's RAW-MESA GLES dispatch table. The 8 slots at BSS 0x106d3b2f0 hold raw Mesa host addresses (not host-thunk slots) → a guest `br` through the 0x5b3a1c0-family stubs jumps out-of-image. Same bug class as SH3/SH3b but for the engine's RUNTIME-built frame-lines table.

## SH18 (Sep 12, 2026): CORRECTED SH17's "don't drive the render-init THUNK" note. Correct thunk drive (--renderthunk: x0=win=XID, x1=parent=0) recovers the engine's REAL ctx object — vtable 0x106731ae0 (live-dumped [0..5]), [ctx+32/40/48]=EGLDisplay/surface/context — and presents a real colored frame (blue 0.2,0.3,0.9) through the engine's own path on that real ctx. New levers --renderbind (engine's own make-current vtable[16]=0x105b3b358), --renderframe-drive (probe engine's frame-fn with fabricated renderer/view).

## SH17 (Sep 12, 2026): REAL Roblox binary now RENDERS a real COLORED FRAME headlessly — live EGL context, engine's own eglSwapBuffers, and a GLES-bridge clear/swap present a solid-green 1280x720 frame. `--renderframe` drives swap fn 0x105b3b408; `--renderclear <r,g,b,a>` draws a colored clear through the float bridge (glClearColor@0x1062d7710 + glClear@0x1062d7740 + swap). Capture: runs/sh17-frame-green.png. Baselines unregressed.

## SH16 (Sep 12, 2026): the render-init's FULL real EGL chain SUCCEEDS headlessly (ANativeWindow_acquire→eglGetDisplay→eglInitialize→eglChooseConfig→eglCreateContext→eglCreateWindowSurface(win=0x200000)→eglMakeCurrent→eglQuerySurface→eglSwapInterval, then Ok(0x0)). Fixed `vfprintf` crash-mask (libc++ terminate body via bionic FILE* + AAPCS64 va_list) + root-caused eglCreateWindowSurface's native window = render-init's x1 param (the wired XID). Real Roblox now has a LIVE Mesa llvmpipe EGL context on a real X11 window, headlessly.

## SH15 (Sep 12, 2026): the real render-init (0x105b3a2d8) drives its EGL chain headlessly through the JIT bridges (ANativeWindow_acquire→eglGetDisplay→eglInitialize→eglGetError | libc++ terminate abort). Fixed `dl_iterate_phdr` guest-callback shim + `fwrite`/`vfprintf` bionic-FILE* to fd 2. New `--renderinit` harness + JIT_FRAMEWORK_DUMP.

## SH14 (Sep 12, 2026): deque-injection path proven STRUCTURALLY capped — the drain pop-loop passes w4=4 hardcoded (not node-derived) and the type-4 maintenance handler dispatches through runtime-built BSS globals (0x1068262e8/300/308) that are statically 0. Located the REAL render-init fn 0x105b3a2d8 (full egl chain). New JIT_REGION_WATCH diagnostic.

## SH13: REAL engine vtable dispatch — --deque-node-live 0x106829f00 makes the engine's own drain-node task-processor 0x10285371c run our injected nodes (~124 pops, exit 124). Not yet render (w4=4 always maintenance).

## SH12: type-4 dispatch CONFIRMED + SUSTAINED through the real idle drain (vtable handler offset bug FIXED: +4 byte=0x20 vs the real [vt+40]=byte40=u64 index 5).

## SH11: sequenced deque-node-live injection crosses the stable idle drain (NODE POPPED, exit 124; sentinel crash eliminated). Defer force-pop + clone live HEAD node + ARM force-pop after placement + block_cache_drop_region.

## SH9: drain SELF-NODE-SKIP guard discovered — sentinel-repoint can never fire; foreign-node path gives a controlled guest dispatch. New --deque-node-live probe.

## SH8: dispatch ABI fully reversed + pinned (regression deque_dispatch_node_layout_matches_engine_abi); --deque-probe live-repoints sentinel vtable (honest failure — sentinel-as-task faults in untracked thread).

## SH7b: --drain-force-pop makes the engine's task-deque pop-loop run + dispatch for the first time (faults on sentinel = controlled crossing). New --deque-node-live. Empirically pinned the drain wait = generic-wait with infinite timeout (-1) → pop-loop only runs when wait times out.

## SH6: host enqueue into the task-deque PROVEN not-a-producer (three negatives) — deque only drained by a real producer that re-enters the drain. --deque-node harness.

## SH5: producer/enqueue contract pinned from disassembly; JIT_DEQUE_PROBE locates each parked consumer's live deque head-cell.

## SH4: idle barrier re-characterized as a per-CPU task-deque consumer; version+latch is NOT a producer (--futex-bump re-parks).

## SH3b: GLOB_DAT *function* slots now resolve through the full GLES chain. New regression glob_dat_function_chain_resolves_gles_names_to_real_slots.

## SH3: eglGetProcAddress routed through a GLES bridge (dynamic GLES loader no longer returns raw Mesa pointers). eglGetProcAddress returns OUR host-thunk slots, not raw Mesa.

## SH2: idle barrier PROVEN a work-queue futex; snapshot captures futex args; --futex-set <hex>. Latch is a signal, not the work.

## SH: arm64jit DECODER REACHES 100% COVERAGE on real libroblox.so — 11,437 Unsupported -> ZERO, 0 PANIC over the whole .text span. Workspace 464/0; HEAD 092e829.

The frontier thereafter was purely the boot (producer never enqueues work onto
the idle futex) + graphics translation (GLES float bridge, compressed textures).
SH2-SH14 pursued the producer/deque; SH15-SH24 pivoted to driving the engine's
own render pipeline directly — now proven clear-path AND geometry-draw dispatch
through the bridge, with a live Mesa llvmpipe EGL context plus a real X11 window.