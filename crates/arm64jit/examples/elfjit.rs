//! Integration spike: load an aarch64 ELF (static non-PIE OR PIE/ET_DYN) with
//! libloader's `load_elf_image`, which lays every PT_LOAD into one contiguous
//! kernel-chosen mapping so **guest vaddr == host address**, then run the entry
//! function through the in-process arm64jit translator — NO QEMU.
//!
//! Build a test ELF with:
//!   cat > t.c <<'EOF'
//!   int entry(void){ return 42; }
//!   EOF
//!   aarch64-linux-gnu-gcc -static -nostdlib -Wl,-e,entry t.c -o tiny.elf
//!
//! Run with: cargo run -p arm64jit --example elfjit -- /path/to/tiny.elf [entry-guest-addr-hex]
//!
//! Because guest==host, the `entry` you pass is BOTH the guest virtual address
//! of the first instruction and (==) its host address; ADRP/ADR of globals and
//! guest loads/stores dereference the correct host pointers directly.

use arm64jit::jit::{CpuState, jit_run};
use arm64jit::shims::set_anativewindow_xid;
use input_wrapper::x11;

// Guest-arena: allocate guest-visible RW buffer (node/vtable for the deque
// injector) in the reserved guest RW tail, so the allocated address (a) is a
// stable guest address < 2^48 (the deque's low48 head-packing keeps only
// bits 47..0, so host-heap 0x7f2a... nodes get MANGLED on pop) and (b) is
// mapped, so the guest's `ldr [vt+40]` derefs real RW memory instead of
// reading garbage. Bump a tick counter from the tail base.
static GUEST_ARENA_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static GUEST_ARENA_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Set the guest-arena base (called with the reserved tail start). Must be a
/// guest RW mapping below 2^48.
fn guest_arena_set_base(b: u64) {
    GUEST_ARENA_BASE.store(b, core::sync::atomic::Ordering::Relaxed);
}

/// Allocate `size` bytes of zeroed guest-visible RW memory from the arena.
/// Returns 0 if the arena wasn't set. 16-byte aligned.
fn guest_arena_alloc(size: usize) -> u64 {
    let base = GUEST_ARENA_BASE.load(core::sync::atomic::Ordering::Relaxed);
    if base == 0 {
        return 0;
    }
    let off = GUEST_ARENA_TICK.fetch_add(size as u64, core::sync::atomic::Ordering::Relaxed);
    let addr = base + off;
    unsafe {
        std::ptr::write_bytes(addr as *mut u8, 0, size);
    }
    addr
}

// Diagnostic: on a host SIGSEGV inside a translated block, print the guest PC
// (CpuState.pc, offset 256) + a few guest regs read from the CpuState (RBX).
// elfjit is a diagnostic binary, so this stays in.
unsafe fn install_fault_debug() {
    extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
        unsafe {
            let uc = ctx as *const libc::ucontext_t;
            let rbx = (*uc).uc_mcontext.gregs[libc::REG_RBX as usize];
            let rip = (*uc).uc_mcontext.gregs[libc::REG_RIP as usize];
            let pc = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(256) as *const u64) } else { 0 };
            let x0 = if (rbx as usize) & 7 == 0 { *(rbx as *const u64) } else { 0 };
            let x1 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(8) as *const u64) } else { 0 };
            let x2 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(16) as *const u64) } else { 0 };
            let x3 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(24) as *const u64) } else { 0 };
            let x4 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(32) as *const u64) } else { 0 };
            let x5 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(40) as *const u64) } else { 0 };
            let x6 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(48) as *const u64) } else { 0 };
            let x7 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(56) as *const u64) } else { 0 };
            let x8 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(64) as *const u64) } else { 0 };
            let x9 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(72) as *const u64) } else { 0 };
            let sp = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(248) as *const u64) } else { 0 };
            let fault = (*info).si_addr() as u64;
            // Dump the raw host bytes around the faulting translated x86 so the
            // memory-op (e.g. a `mov rax,[rax+0x30]` = guest `ldr x8,[x8,#48]`)
            // can be identified precisely even though CpuState.pc is coarse.
            let mut raw = [0u8; 48];
            std::ptr::copy_nonoverlapping(rip.wrapping_sub(24) as *const u8, raw.as_mut_ptr(), 48);
            let hex = raw.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
            let x10 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(80) as *const u64) } else { 0 };
            let x19 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(152) as *const u64) } else { 0 };
            let x20 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(160) as *const u64) } else { 0 };
            let x21 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(168) as *const u64) } else { 0 };
            let x22 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(176) as *const u64) } else { 0 };
            let x23 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(184) as *const u64) } else { 0 };
            let x28 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(224) as *const u64) } else { 0 };
            let x29 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(232) as *const u64) } else { 0 };
            let x30 = if (rbx as usize) & 7 == 0 { *(rbx.wrapping_add(240) as *const u64) } else { 0 };
            let name = if sig == libc::SIGSEGV { "SIGSEGV" } else if sig == libc::SIGILL { "SIGILL" } else if sig == libc::SIGABRT { "SIGABRT" } else { "SIGFAULT" };
            let tid = unsafe { libc::syscall(libc::SYS_gettid) };
            // Does the faulting host thread actually run a guest CpuState?
            // Compare the ucontext RBX against the registered CpuState pointer
            // for this host tid. If they match, the guest reg dump is real; if
            // not, RBX is an arbitrary host value and the dump is garbage.
            let reg_state = arm64jit::jit::guest_state_of_host(tid as i64);
            let state_matches = reg_state != 0 && reg_state == rbx as u64;
            // Guest tid of the matching registered state (0 if unregistered).
            let reg_guest_tid = if reg_state != 0 {
                unsafe { *(reg_state as *const u64).wrapping_add(848 / 8) } // CpuState.tid field
            } else {
                u64::MAX
            };
            // Enumerate all registered guest threads: (host_tid, guest_tid, state).
            let thr = arm64jit::jit::dump_guest_threads()
                .iter()
                .map(|(h, g, s)| format!("({h}:{g},{s:#x})"))
                .collect::<Vec<_>>()
                .join(" ");
            let in_jit = arm64jit::jit::in_jit_run();
            // Diagnostic: dump the LocalStorageManager static-map global on fault
            // to confirm whether our boot-time seed persisted.
            let lsm_global = unsafe { *(0x10726f8c0u64 as *const u64) };
            // Full host x86-64 register file (SysV). The guest regs above are read
            // through rbx==CpuState base, so if rbx itself is corrupt the guest
            // dump is an artifact; the host frame disambiguates a real guest fault
            // from a handler/spurious read.
            let g = |i: usize| (*uc).uc_mcontext.gregs[i];
            let (rax, rcx, rdx, rsi, rdi, rbp, rsp, r8, r9, r10, r11, r12, r13, r14, r15, fl) = (
                g(libc::REG_RAX as usize), g(libc::REG_RCX as usize), g(libc::REG_RDX as usize),
                g(libc::REG_RSI as usize), g(libc::REG_RDI as usize), g(libc::REG_RBP as usize),
                g(libc::REG_RSP as usize), g(libc::REG_R8 as usize), g(libc::REG_R9 as usize),
                g(libc::REG_R10 as usize), g(libc::REG_R11 as usize), g(libc::REG_R12 as usize),
                g(libc::REG_R13 as usize), g(libc::REG_R14 as usize), g(libc::REG_R15 as usize),
                g(libc::REG_EFL as usize),
            );
            let s = format!(
                "\n[{name}] tid={tid} fault={fault:#x} rip={rip:#x} guestpc={pc:#x} rbx_matches_gueststate={state_matches} (reg_state={reg_state:#x}, tid={reg_guest_tid})\n  x0={x0:#x} x1={x1:#x} x2={x2:#x} x3={x3:#x} x4={x4:#x}\n  x5={x5:#x} x6={x6:#x} x7={x7:#x} x8={x8:#x} x9={x9:#x} sp={sp:#x}\n  x10={x10:#x} x19={x19:#x} x20={x20:#x} x21={x21:#x} x22={x22:#x}\n  x23={x23:#x} x28={x28:#x} x29={x29:#x} lr(x30)={x30:#x} lsm_map_global=0x{lsm_global:x}\n  HOST rax={rax:#x} rbx={rbx:#x} rcx={rcx:#x} rdx={rdx:#x} rsi={rsi:#x} rdi={rdi:#x}\n  HOST rbp={rbp:#x} rsp={rsp:#x} r8={r8:#x} r9={r9:#x} r10={r10:#x} r11={r11:#x}\n  HOST r12={r12:#x} r13={r13:#x} r14={r14:#x} r15={r15:#x} eflags={fl:#x}\n  GUEST_THREADS {thr} in_jit_run={in_jit}\n  raw[]= {hex}\n"
            );
            let b = s.as_bytes();
            libc::write(2, b.as_ptr() as *const libc::c_void, b.len());
            // Native frame-pointer backtrace (SysV: rbp chain, [rbp]=prev rbp,
            // [rbp+8]=return addr). Classifies every ret addr as host-JIT vs
            // guest-text vs libc so we see WHICH dispatcher path jumped to guest.
            let mut btd = String::from("\n  BT:");
            let mut fp: u64 = rbp as u64;
            let classify = |ra: u64| -> String {
                if ra >= 0x100000000 && ra < 0x120000000 {
                    format!("GUEST({ra:#x})")
                } else if ra >= 0x7f0000000000 && ra < 0x7f8000000000 {
                    format!("HOST({ra:#x})")
                } else if ra >= 0x7f0000000000 {
                    format!("HOST({ra:#x})")
                } else {
                    format!("{ra:#x}")
                }
            };
            for _ in 0..24 {
                if fp & 7 != 0 || fp < 0x400000 || fp >> 56 != 0 {
                    break;
                }
                let ra = unsafe { *(fp.wrapping_add(8) as *const u64) };
                if ra == 0 {
                    break;
                }
                btd.push_str(&format!(" -> {}", classify(ra)));
                let nfp = unsafe { *(fp as *const u64) };
                if nfp <= fp || nfp - fp > 0x4000 {
                    break;
                }
                fp = nfp;
            }
            btd.push('\n');
            libc::write(2, btd.as_bytes().as_ptr() as *const libc::c_void, btd.len());
        }
        // Restore default disposition for SIGABRT before re-raising via
        // process::abort() (which delivers SIGABRT); otherwise we recurse into
        // this handler in an infinite dump loop.
        if sig == libc::SIGABRT {
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = libc::SIG_DFL;
                libc::sigaction(libc::SIGABRT, &sa, std::ptr::null_mut());
            }
        }
        std::process::abort();
    }
    for sig in [libc::SIGSEGV, libc::SIGILL, libc::SIGABRT] {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as usize;
        sa.sa_flags = libc::SA_SIGINFO;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(sig, &sa, std::ptr::null_mut());
    }
}

/// How a single `--kicker` drives its target guest global.
#[derive(Clone, Copy)]
enum KickerMode {
    /// `pthread_cond_broadcast` the address every tick.
    Broadcast,
    /// Write an exact u64 value every tick (`--kicker 0xADDR=0xVAL`).
    Fixed(u64),
    /// Historical lifecycle pulse: write 1 through the first gate, then 2.
    Pulse,
}

/// Bring up an Xvfb X server + a 1280x720 window and register its XID as the
/// guest's ANativeWindow handle (GRAPHICS_RECOMMENDATION §5.3). Runs
/// SYNCHRONOUSLY so the real window is wired before StartApp reaches the
/// window/EGL surface path — a racing spawned thread would lose and hand the
/// guest the sentinel instead of a genuine window. The X connection is leaked
/// (kept alive) so the window outlives this function. Returns the wired XID,
/// or 0 if no window could be opened (caller keeps the sentinel fallback).
fn wire_real_window() -> u64 {
    let display_num = 220 + (std::process::id() % 50) as usize;
    let display = format!(":{display_num}");
    let mut xvfb = None;
    for _ in 0..20 {
        if std::path::Path::new(&format!("/tmp/.X11-unix/X{display_num}")).exists() {
            break;
        }
        if xvfb.is_none() {
            xvfb = std::process::Command::new("Xvfb")
                .arg(&display)
                .arg("-screen").arg("0").arg("1280x720x24")
                .arg("-nolisten").arg("tcp")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for _ in 0..40 {
        if let Ok((conn, win)) = x11::open_window_sized(Some(&display), 1280, 720) {
            Box::leak(Box::new(conn)); // keep the window alive for the boot
            unsafe {
                std::env::set_var("DISPLAY", &display);
                std::env::set_var("EGL_PLATFORM", "x11");
            }
            let xid = win as u64;
            set_anativewindow_xid(xid);
            eprintln!(
                "[elfjit:anativewindow] wired real X11 window XID=0x{xid:x} on {display} as the guest ANativeWindow"
            );
            return xid;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!(
        "[elfjit:anativewindow] could not open an X11 window (Xvfb absent?) — keeping the sentinel ANativeWindow"
    );
    if let Some(mut c) = xvfb {
        let _ = c.kill();
    }
    0
}

/// Arm the guest-persistence root for a real run: if `SOBER_ANDROID_ROOT` is
/// not already set, create a stable host directory under the runtime's data
/// dir and export it, so guest `/data`/`/sdcard`/`/cache` writes (the client's
/// datastore / login-session store) land on persistent host disk via
/// `arm64jit::fsmap` instead of the nonexistent host root. Verified not to
/// disturb the boot (stable idle exit 124 with the root armed); it only gains
/// effect when the client opens a `/data` sink.
fn arm_persist_root() {
    if std::env::var_os("SOBER_ANDROID_ROOT").is_some() {
        return;
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|h| h.join(".local/share"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let dir = data.join("open-sober").join("android-root");
    if let Ok(()) = std::fs::create_dir_all(&dir) {
        unsafe { std::env::set_var("SOBER_ANDROID_ROOT", &dir) };
        println!(
            "[fsmap] armed persistence root SOBER_ANDROID_ROOT={}",
            dir.display()
        );
    } else {
        println!("[fsmap] warn: could not create persistence root {}", dir.display());
    }
}

fn main() {
    unsafe {
        install_fault_debug();
    }
    arm_persist_root();
    let path = std::env::args()
        .nth(1)
        .expect("usage: elfjit <aarch64-elf> [entry-guest-addr-hex]");
    let entry_arg = std::env::args().nth(2);

    let el = unsafe { libloader::elf::load_elf_image(std::path::Path::new(&path)) }
            .expect("load_elf_image");

        // Fold the import resolver + host shims into the boot path: bind every PLT
        // JUMP_SLOT GOT slot to a host thunk so translated Roblox `blr`s hit real
        // host functions (libc/libm/float/bionic/graphics-stub) instead of stalling.
        let (nbound, nunresolved) = arm64jit::plt::bind_image_plt(&el, None);
        if nbound > 0 {
            println!("PLT imports bound: {nbound} to host thunks ({} unbound)", nunresolved);
        }

    // Route the TLS-block allocator's big-allocation path to host calloc so the
    // unseeded MemoryPool empty-free-list returns a real buffer instead of a
    // NULL+abort. Site 0x1d9801c is the big allocator of Roblox v2.738.1397's
    // per-thread TLS block (reachable from 0x1d96a40's empty free-list tail).
    match arm64jit::jit::route_mempool_big_alloc_to_host(el.guest_of(0x1d9801c), 0x1_0000_0000) {
        Ok(tp) => {
            println!("[mempool] big-alloc 0x1d9801c routed to host calloc (thunk @ {tp:#x})");
            let p = tp as *const u8;
            let hex: Vec<String> = (0..20).map(|i| unsafe { format!("{:02x}", *p.add(i)) }).collect();
            println!("[mempool] thunk bytes: {} (JIT-readable via guest image)", hex.join(" "));
        }
        Err(e) => println!("[mempool] warn: big-alloc patch skipped: {e}"),
    }

    // Guest entry: the ELF's own e_entry (already relocated to guest space by
    // load_elf_image) unless a link-time address is supplied, in which case we
    // translate it to guest/runtime space with guest_of().
    let entry = match entry_arg {
        Some(h) => {
            let link = u64::from_str_radix(h.trim_start_matches("0x"), 16)
                .unwrap_or_else(|e| panic!("bad entry hex: {e}"));
            el.guest_of(link)
        }
        None => el.info.entry,
    };

    println!(
        "loaded '{}': is_pie={} base_load_vaddr=0x{:x} e_entry=0x{:x}",
        path, el.info.is_pie, el.info.base_load_addr, el.info.entry
    );
    for s in &el.segments {
        println!(
            "  segment guest=[0x{:x},0x{:x}) host=same prot={}{}{}",
            s.guest_vaddr,
            s.guest_vaddr + s.memsz,
            if s.prot.read { "r" } else { "-" },
            if s.prot.write { "w" } else { "-" },
            if s.prot.execute { "x" } else { "-" }
        );
    }

    // Pick the executable (text) segment to translate code out of.
    let seg = el
        .segments
        .iter()
        .find(|s| s.prot.execute)
        .expect("no executable segment");
    let base = seg.guest_vaddr; // == host addr of image[0] (guest==host)

    // Use the FULL mapped span (every PT_LOAD + inter-segment gaps, which
    // load_elf_image lays into ONE contiguous anonymous region at the fixed
    // base) as the valid-pc extent. The guest may legitimately branch/call
    // into higher sections (data-backed trampolines, .bss-slotted function
    // pointers) that live past the r-x slice; bounding `run_loop` to only the
    // text slice wrongly flags those as "outside image". Compute the extent as
    // the largest guest_vaddr+memsz across segments (the whole mmap is zero-
    // filled), relative to this text-segment base.
    let full_end = el
        .segments
        .iter()
        .fold(0u64, |m, s| m.max(s.guest_vaddr + s.memsz));
    let len = (full_end - base) as usize;

    // Reserve a writable guest tail past the ELF's mapped span. Real Roblox
    // `nativeInitCrashpad` walks a link-time `& bss` telemetry table base by a
    // slot index that reaches tens of MB past the last PT_LOAD `.bss` end; on
    // real Android that adjacent memory is mapped anonymous, our loader maps
    // only the ELF span. Reserve 384MB of RW headroom so the deep table writes
    // (and other large guest tables/arenas) have real backing instead of
    // SIGSEGV. MAP_FIXED at a page-aligned address after base+len is safe
    // (host heap/stack live elsewhere); must start page-aligned or mmap EINVALs.
    let tail_start = (full_end as usize + 0xfff) & !0xfff;
    const TAIL_SIZE: usize = 384 * 1024 * 1024;
    match libloader::elf::reserve_guest_tail(tail_start, TAIL_SIZE) {
        Ok(_s) => println!("[tail] reserved {TAIL_SIZE}B guest RW tail @0x{tail_start:x}"),
        Err(e) => eprintln!("[tail] warn: guest-tail reserve skipped: {e}"),
    }
    // Give the deque-node injector a guest-visible arena in the RW tail so its
    // node/vtable allocations are stable guest addresses (< 2^48, low48-safe)
    // backed by real mapped RW memory.
    guest_arena_set_base(tail_start as u64);

    // Route the LocalStorageManager static hash-map's bucket-array allocator
    // (`0x1d97744`, receives its byte size in x0) to host calloc, so the
    // unseeded per-object MemoryPool empty free-list returns a real zeroed
    // buffer. The map's lazy init then stores a valid non-NULL bucket array
    // into its header global (0x726f8c0) instead of NULL, so the hash-lookup
    // reader (0x1d99e30: `ldr x8,[0x726f8c0]; ...; ldar x8,[x8]; ldr x0,[x8,idx<<3]`)
    // finds a real map instead of derefing a NULL bucket.
    match arm64jit::jit::route_allocator_x0_to_calloc(el.guest_of(0x1d97744), 0x1_0000_0000) {
        Ok(tp) => println!("[lsm-map] allocator 0x1d97744 routed to host calloc(x0) (thunk @ {tp:#x})"),
        Err(e) => eprintln!("[lsm-map] warn: allocator route skipped: {e}"),
    }

    // Disable the LSM map's lazy-init store that would clobber our seed.
    //
    // The init at 0x1d975f8 does `str x0,[x22,#2240]` writing its (routed)
    // allocator result into the map global 0x726f8c0, then memsets and builds a
    // two-level bucket structure into that discarded buffer. The reader
    // (0x1d99e40/0x1d99e4c/0x1d99e50) instead requires the global to point at a
    // bucket array whose every slot (key>>29) is a pointer to a zeroed
    // sub-array (indexed by (key>>16)&0x1fff); a bare all-zero calloc leaves
    // bucket slots NULL and the reader derefs NULL. `seed_static_empty_map`
    // below constructs exactly the required layout, so NOP the init store to
    // keep that seed authoritative. Guest insn -> NOP (0xd503201f).
    let init_store = el.guest_of(0x1d975f8);
    {
        let page = init_store & !0xfff;
        if unsafe { libc::mprotect(page as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_WRITE) } == 0 {
            unsafe { *(init_store as *mut u32) = 0xd503_201fu32 };
            unsafe { libc::mprotect(page as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_EXEC) };
            println!("[lsm-map] NOP'd LSM init store at 0x{init_store:x} (guest 0x{:x}) to keep seeded empty map", 0x1d975f8);
        } else {
            eprintln!("[lsm-map] warn: could not mprotect init-store page RW");
        }
    }

    // Seed the LocalStorageManager C++ static hash-map whose .bss base global
    // (guest 0x10726f8c0 = 0x726f000+0x8c0) is 0 because its constructor never
    // ran (.init_array is empty). The reader (0x1d99e40) does
    //   ldr x8,[0x726f8c0]; add x8,x8,key>>29<<3; ldar x8,[x8]; ldr x0,[x8,idx<<3]
    // and faults on the NULL bucket (`key` is a heap/guest pointer, so
    // key>>29 lands up to ~0xb00 buckets in). Seed a real zeroed region as the
    // bucket array, with every bucket slot pointing at a shared (also-zeroed,
    // non-overlapping) sub-array slot, so any lookup reads 0 -> "not found".
    // (Fallback: if the guest later overwrites it with a real map, all the better.)
    unsafe fn seed_static_empty_map(map_global_guest: u64) {
        const BUCKETS: usize = 0x400000; // key>>29: pointers ~0x56.. give idx ~0x2b2a7; grant headroom
        const SLOT_STRIDE: usize = 8;
        const SLOT_REGION: usize = 0x10000; // (key>>16)&0x1fff max index * 8
        let buckets_bytes = BUCKETS * SLOT_STRIDE;
        let total = buckets_bytes + SLOT_REGION;
        let buf = Box::leak(vec![0u8; total].into_boxed_slice());
        let bufp = buf.as_mut_ptr();
        let base = bufp as u64;
        let sub = base + buckets_bytes as u64;
        // Every bucket slot points at `sub` (a zeroed shared sub-array), so
        // `ldar x8,[bucket[key>>29]]` returns a non-null pointer and
        // `ldr x0,[x8, idx<<3]` reads 0 -> cbz -> return NULL (not found).
        let slots = std::slice::from_raw_parts_mut(bufp.cast::<u64>(), BUCKETS);
        for s in slots {
            *s = sub;
        }
        *(map_global_guest as *mut u64) = base;
        println!("[lsm-map] seeded static empty LocalStorageManager map: global 0x{map_global_guest:x} -> bucket array 0x{base:x} ({} buckets, shared zero sub @0x{sub:x})", BUCKETS);
    }
    fn link_to_guest(el0: &libloader::elf::LoadedElf, link: u64) -> u64 {
        el0.guest_of(link)
    }
    unsafe { seed_static_empty_map(link_to_guest(&el, 0x726f000 + 0x8c0)) };

    // Seed the JNICallProtocol-ish refcounted-singleton pointer at guest
    // 0x107333948 (link-time 0x7333000+0x948). The once-init atomic store
    // (0x2b9e890) normally writes the object address 0x7333950 into that slot;
    // under the JIT the .init_array never runs so it stays 0, and the acquire
    // path (0x21daf00) locks `this+8` (a pthread_mutex at object+0x8) — with
    // `this` NULL it calls pthread_mutex_lock(0x8) and faults. The object
    // itself is zeroed bss (a valid PTHREAD_MUTEX_INITIALIZER at +8), so just
    // wiring the pointer releases the lock into the zeroed (== initial, unlocked)
    // mutex.
    let singleton_slot = link_to_guest(&el, 0x7333000 + 0x948); // [ptr] slot
    let singleton_obj = link_to_guest(&el, 0x7333000 + 0x950); // object base
    unsafe { *((singleton_slot) as *mut u64) = singleton_obj };
    println!(
        "[JNICall-singleton] seeded ptr 0x{singleton_slot:x} -> object 0x{singleton_obj:x} (zeroed bss ~ PTHREAD_MUTEX_INITIALIZER at +8)"
    );
    // Read back the seed to confirm it landed where the guest reads it.
    let g_chk = link_to_guest(&el, 0x726f000 + 0x8c0);
    let v_chk = unsafe { *(g_chk as *const u64) };
    println!("[lsm-map] readback global 0x{g_chk:x} = 0x{v_chk:x} (must be non-zero)", );

    println!(
        "running entry guest=0x{:x} host=0x{:x} (segment base guest=0x{:x} size=0x{:x})",
        entry, entry, base, len
    );

    // image = the executable segment's bytes. Because guest==host, the `base`
    // passed to compile_image is the guest address of image[0] and the `entry`
    // is the guest address of the first instruction to run.
    let image = unsafe { std::slice::from_raw_parts(base as *const u8, len) };
    let mut st = CpuState::new();

    // Optional x0/x1/x2 init. Pass `buf` in position 3 to allocate a
    // writable 256-byte host buffer (guest==host, so its address is a valid
    // guest pointer) and put its address in x0; also x1=x0+32. Even when the
    // guest is a real binary we don't yet bootstrap (no TLS/stack), this lets
    // small aarch64 test functions run through the dispatcher.
    for (i, arg) in std::env::args().skip(3).take(3).enumerate() {
        if arg == "--jni" || arg == "buf" || arg == "--startapp" {
            let v = if arg == "buf" {
                let b = Box::leak(vec![0x7fu8; 256].into_boxed_slice());
                if i == 0 {
                    let base = b.as_ptr() as u64;
                    st.set(0, base);
                    st.set(1, base + 32);
                }
                b.as_ptr() as u64
            } else {
                0 // --jni / --startapp aren't x-register values; handled separately
            };
            if arg == "buf" && i == 0 {
                continue;
            }
            let _ = v;
        } else {
            let v = u64::from_str_radix(arg.trim_start_matches("0x"), 16)
                .unwrap_or_else(|e| panic!("bad x{i} hex: {e}"));
            st.set(i, v);
        }
    }

    // Bootstrap a guest runtime the binary can actually use -------------
    // (1) Guest stack: allocate a real writable region (guest==host addressing,
    //     so its host pointer is a valid guest pointer) and point SP at the top.
    // (2) TLS base: point CpuState.tpidr at a writable region so `mrs tpidr_el0`
    //     returns a non-zero, writable base (FS/GS-style thread pointer).
    const STACK_SIZE: usize = 4 * 1024 * 1024;
    let stack = Box::leak(vec![0u8; STACK_SIZE].into_boxed_slice());
    // Lay out a real kernel-style initial stack (argc/argv/envp/auxv) so glibc
    // IFUNCs resolve to scalar paths instead of reading garbage auxv into SMP
    // (which drove the JIT into an unsupported `str za` wall). No SME/SVE bits.
    let mut auxv = arm64jit::boot::standard_auxv(
        &el,
        arm64jit::boot::HWCAP_FP | arm64jit::boot::HWCAP_ASIMD,
        0,
    );
    let sp = arm64jit::boot::layout_initial_stack(
        stack.as_ptr() as *mut u8,
        STACK_SIZE,
        Some(&[0u8; 0]), // argv[0] (empty) — keeps argc==1 like a real shell exec
        &[],
        &mut auxv,
    );
    st.set(31, sp); // x31 = SP (points at argc on the initial stack)
    const TLS_SIZE: usize = 1024 * 64;
    let tls = Box::leak(vec![0u8; TLS_SIZE].into_boxed_slice());
    // Seed the guest TLS region from the image's PT_TLS (local-exec/initial-exec
    // thread-locals) and point tpidr_el0 at the AArch64 TCB (16 bytes before the
    // module's TLS block). `__thread` globals then read/write real data.
    st.tpidr = libloader::elf::setup_guest_tls(
        &el.info,
        std::path::Path::new(&path),
        tls.as_ptr() as *mut u8,
        TLS_SIZE,
    )
    .expect("setup_guest_tls");
    // Publish the main thread's TLS block as the template that spawned guest
    // threads (pthread_create/clone children) clone per-thread, so their
    // `__thread` locals and TP-indexed tables match the main thread instead of
    // a bare zeroed buffer.
    arm64jit::jit::publish_guest_tls_template(tls.as_ptr() as u64, TLS_SIZE);
    println!("guest sp=0x{:x} tls(tpidr)=0x{:x}", sp, st.tpidr);

    // JNI boot mode: hand the guest a guest-visible JavaVM* in x0 (as the Android
    // runtime would). Pass `--jni` to set x0 = vm. If x0/x1/x2 were already
    // supplied via positional args they win (we don't clobber a caller's x0).
    if std::env::args().any(|a| a == "--jni") && st.x[0] == 0 {
        let (_env, vm) = arm64jit::jni::build_jni();
        st.x[0] = vm; // JNI_OnLoad(JavaVM* vm, void* reserved) -> x0 = vm
        println!("JNI boot: x0 = JavaVM* 0x{:x}", vm);
    }

    // PC-driven dispatcher: compiles reachable regions and re-enters on
    // indirect branch (`blr`) / `br` / `ret`, so real (blr-heavy) Roblox code
    // can actually *execute* rather than stopping at the first blr.
    match jit_run(image, base, entry, &mut st as *mut CpuState) {
        Err(e) => {
            eprintln!("arm64jit run_loop stopped: {e}");
            std::process::exit(1);
        }
        Ok(r) => {
            // stderr is unbuffered; if this line appears BEFORE the SIGSEGV dump,
            // the fault is in post-run teardown, not the boot loop itself.
            eprintln!("[elfjit] jit_run returned Ok({r:#x}) — entering post-run phase");
            println!("JIT(no-QEMU) entry() -> {} (0x{:x})", r, r);
        }
    }
    // Let spawned worker guest threads (pthread_create/clone children started
    // during boot) run to completion before the process exits, so jit_run on a
    // detached child isn't torn down mid-translation (which surfaces as a
    // SIGSEGV reading a freed child CpuState as 'registers'). Wait for the
    // active-guest-thread count to return to the baseline (main only).
    let baseline = 1;
    for _ in 0..400 {
        if arm64jit::jit::active_guest_threads() <= baseline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    std::thread::sleep(std::time::Duration::from_millis(50));

    // --startapp <link-addr>: after JNI_OnLoad completes, drive the next real
    // boot stage — the Java side's `nativeAppBridgeV2StartAppWithParams` (the
    // entry that creates the engine main loop + EGL/GLES context). We chain it
    // as a fresh guest entry after the registration phase, giving it the same
    // JNIEnv in x0 plus fake but VALID (non-null, dereferenceable) jobject /
    // jstring handles, exactly as the real JVM would. Captures how far the real
    // binary gets into StartApp (main-loop / graphics init) before the next wall.
    if let Some(hex) = {
        let args: Vec<String> = std::env::args().collect();
        args.iter()
            .position(|a| a == "--startapp")
            .and_then(|i| args.get(i + 1).cloned())
    } {
        // Reuse the singleton env/vm; build a fake-but-valid jobject (a 5-word
        // object header) and a jstring handle containing the StartApp params JSON.
        let (env_ptr, _vm) = arm64jit::jni::build_jni();
        let activity = arm64jit::jni::new_fake_object(); // non-null jobject
        let params = arm64jit::jni::new_string_utf_handle(b"{\"key\":\"\"}");
        let link = u64::from_str_radix(hex.trim_start_matches("0x"), 16)
            .unwrap_or_else(|_| panic!("bad --startapp hex"));
        let start_app = el.guest_of(link);
        eprintln!("[elfjit] driving StartApp @ guest {start_app:#x} after JNI_OnLoad (env={env_ptr:#x} jobject={activity:#x} params={params:#x})");
        let mut s2 = arm64jit::jit::CpuState::new();
        s2.tpidr = arm64jit::jit::current_guest_tp();
        // Continue on the boot-phase guest stack (real SP), not a fresh 0 —
        // StartApp's prologue `sub sp,#0xf0` would otherwise wrap to 0xffff..10
        // and the frame-write faults. The Java side enters natives on whatever
        // thread is current; elfjit reuses the main guest thread's SP.
        s2.x[31] = st.x[31]; // guest SP
        s2.x[0] = env_ptr;
        s2.x[1] = activity;
        s2.x[2] = params;
        // Concurrent guest-thread state sampler (JIT_THREADS=1). StartApp's
        // `jit_run` parks the main thread forever (the engine main-loop
        // lifecycle-await), so a post-run sampler would never run. Instead
        // spawn a detached host sampler that polls `snapshot_threads()`
        // every ~200 ms for a bounded window, dumping each parked thread's
        // hostcall slot (pc), guest call-site (x30/lr) and wait-object args
        // (x0..x2). This pins the boot wall to the exact guest function that
        // blocks and what it awaits. Runs concurrently with the jit_run.
        if std::env::var_os("JIT_THREADS").is_some() {
            std::thread::spawn(|| {
                for it in 0..100 {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let (c, h) = arm64jit::jit::block_cache_stats();
                    eprintln!("[elfjit:stats] it={it} compiles={c} hits={h}");
                    let snaps = arm64jit::jit::snapshot_threads();
                    for t in &snaps {
                        let at = arm64jit::resolver::name_of_call_addr(t.pc)
                            .unwrap_or_else(|| format!("{:#x}", t.pc));
                        eprintln!(
                            "  host_tid={} guest_tid={} pc={at} lr={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x} x5={:#x} x19={:#x}[*={:#x}] x20={:#x} x21={:#x} x29={:#x} sp={:#x}",
                            t.host_tid, t.guest_tid, t.lr, t.x0, t.x1, t.x2, t.x3, t.x5, t.x19,
                            // deref [x19]: the wait-fn arg0 Q (host-heap; its high-32
                            // is the self-syncing version epoch, +4 the futex latch).
                            if t.x19 >= 0x100000000 && t.x19 >> 56 == 0 && t.x19 & 7 == 0 { unsafe { *(t.x19 as *const u64) } } else { 0 },
                            t.x20, t.x21, t.x29, t.sp
                        );
                        // JIT_DEQUE_PROBE=1: recover the parked consumer's
                        // deque-root from the waiter's SAVED frame and read the
                        // live deque head. The generic wait-with-timeout at
                        // 0x10284d018 leaves the caller's (drain fn 0x2856e40)
                        // callee-saved regs on its stack: stp x20,x19,[sp,#64]
                        // stored the DRAIN's x20 (= deque root, awk the waiter's
                        // own x20 is -1 = the infinite-timeout arg) and x19 (=
                        // consumer struct) at [sp+64] / [sp+72]. Read-only — the
                        // prerequisite to a host-side producer enqueue (push onto
                        // the deque the parked consumer drains).
                        if std::env::var_os("JIT_DEQUE_PROBE").is_some()
                            && t.lr == 0x10284d134
                        {
                            let sp = t.sp;
                            if sp >= 0x100000000 && sp >> 56 == 0 {
                                let root = unsafe { *(sp as *const u64).add(8) }; // [sp+64]
                                let cstruct = unsafe { *(sp as *const u64).add(9) }; // [sp+72]
                                let is_ptr = |p: u64| p >= 0x100000000 && p >> 56 == 0 && p & 7 == 0;
                                let q = if is_ptr(cstruct) { unsafe { *(cstruct as *const u64).add(13) } } else { 0 }; // [struct+104]
                                let headcell = if is_ptr(root) { unsafe { *(root as *const u64) } } else { 0 };
                                let head = if is_ptr(headcell) { unsafe { *(headcell as *const u64) } } else { 0 };
                                eprintln!(
                                    "  [deque] waiter_sp={sp:#x} drain_root=[sp+64]={root:#x} drain_struct={cstruct:#x} Q=[struct+104]={q:#x} headcell=[root]={headcell:#x} head={head:#x} (node={:#x} tag={:#x})",
                                    head & 0xffffffffffff, head >> 48
                                );
                                // Read the head node's internals to tell a real
                                // pending task node from a sentinel/garbage cell:
                                // next=[node], cb40=[node+40], vt=[node+112]&~0x3f
                                // then dispatch-cb [vt+40]; and [root+8] tag.
                                // Head node internals + the per-CPU slot layout.
                                // SH5 disasm pinned the deque head ATOMIC at
                                // slot+0x10 (packed low48=node, high16=tag) and
                                // tail at slot+0x18; slot+0 is likely a separate
                                // field (the sentinel/root ptr). Dump the whole
                                // neighborhood to resolve which offset the parked
                                // consumer actually drains.
                                let node = head & 0xffffffffffff;
                                // Dump the per-CPU slot neighborhood around the
                                // head-CELL to resolve the real deque head offset.
                                // The probe mislabeled slot+0 as the head; SH5
                                // disasm says the head ATOMIC is at slot+0x10.
                                if is_ptr(headcell) {
                                    let off = |o: usize| unsafe { *(headcell as *const u64).add(o / 8) };
                                    eprintln!(
                                        "      slot[{headcell:#x}] +0x00={:#x} +0x08={:#x} +0x10(HEAD)={:#x} +0x18(TAIL)={:#x} +0x20={:#x}",
                                        off(0), off(0x08), off(0x10), off(0x18), off(0x20)
                                    );
                                }
                                let rt8 = if is_ptr(root) { unsafe { *(root as *const u64).add(1) } } else { 0 };
                                if is_ptr(node) {
                                    let nxt = unsafe { *(node as *const u64) };
                                    let cb40 = unsafe { *(node as *const u64).add(5) }; // +40
                                    let v112 = unsafe { *(node as *const u64).add(14) }; // +112
                                    let vt = v112 & !0x3f;
                                    let dcb = if is_ptr(vt) { unsafe { *(vt as *const u64).add(5) } } else { 0 }; // [vt+40]
                                    eprintln!(
                                        "      node.next={nxt:#x} node[+40]={cb40:#x} node[+112]={v112:#x} vt={vt:#x} [vt+40]={dcb:#x} root[+8]tag={rt8:#x}"
                                    );
                                } else {
                                    eprintln!(
                                        "      head cell not a valid node (0); root[+8]tag={rt8:#x}"
                                    );
                                }
                                // Epoch: waiter x19 = the wait object Q' whose
                                // high-32 is the self-syncing version epoch, futex
                                // at Q'+4 = x1.
                                if is_ptr(t.x19) {
                                    let qw = unsafe { *(t.x19 as *const u64) };
                                    eprintln!(
                                        "      Q'=t.x19={:#x} [Q']={:#x} (refc=low32 {:#x} epoch=high32 {:#x}) futex_uaddr=x1={:#x}",
                                        t.x19, qw, qw & 0xffffffff, qw >> 32, t.x1
                                    );
                                }
                                // RAW STACK DUMP: print the parked waiter's sp
                                // window so the true frame layout (drain root,
                                // consumer struct, Q, timeout, saved x30) is
                                // resolved empirically instead of by inference.
                                // sp is host-readable (guest==host addressing).
                                if std::env::var_os("JIT_STACKDUMP").is_some() {
                                    let mut line = format!("      [stack sp={sp:#x}]");
                                    for o in (0..96usize).step_by(8) {
                                        let v = unsafe { *(sp as *const u64).add(o / 8) };
                                        line.push_str(&format!(" +{o:02x}={v:#018x}"));
                                    }
                                    eprintln!("{line}");
                                }
                                // [sp+0x50]=drain x20 (root), [sp+0x58]=drain x19
                                // (consumer) per drain 0x2856e54 stp x20,x19,[sp,#80]
                                // + generic-wait clobbers [sp+40..72] only. Try those.
                                if std::env::var_os("JIT_DEQUE_PROBE2").is_some() {
                                    let dr = unsafe { *(sp as *const u64).add(0x50 / 8) };
                                    let dc = unsafe { *(sp as *const u64).add(0x58 / 8) };
                                    eprintln!(
                                        "      [probe2] sp+0x50(drain x20 root)={dr:#x} sp+0x58(drain x19 consumer)={dc:#x}",
                                    );
                                    if is_ptr(dr) {
                                        let rd = unsafe { *(dr as *const u64) };
                                        eprintln!("        [root]={rd:#x}");
                                        if is_ptr(rd) {
                                            let head = unsafe { *(rd as *const u64) };
                                            eprintln!("        [[root]] head={head:#x} (node {:#x} tag {:#x})",
                                                head & 0xffffffffffff, head >> 48);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }
        // Host-side lifecycle kicker (experimental): the engine owner parks
        // busy-polling a guest global (`ldar x8,[x8]; cmp #1; b.eq`) until the
        // Java layer's app-command sets it. On this box there is no Java side,
        // so `--kicker 0x<guest-hex-global>=<value-hex>` spawns a detached host
        // thread that writes the value to that guest global repeatedly WHILE
        // jit_run is parked, to test whether releasing the awaited predicate
        // lets StartApp proceed past the rendezvous toward the looper.
        // Host-side lifecycle kicker (experimental): the engine owner parks
        let mut kickers: Vec<(u64, KickerMode)> = Vec::new();
        let args: Vec<String> = std::env::args().collect();
        let mut i = 0;
        while i < args.len() {
            if let Some(k) = args[i].strip_prefix("--kicker") {
                let spec = if k.is_empty() {
                    i += 1;
                    if i >= args.len() { panic!("--kicker needs a value"); }
                    args[i].clone()
                } else {
                    k.trim_start_matches('=').to_string()
                };
                let (addr_s, val_s) = spec.split_once('=').unwrap_or((spec.trim_start_matches("0x"), "1"));
                let addr = u64::from_str_radix(addr_s.trim_start_matches("0x"), 16).expect("bad kicker addr");
                let val_l = val_s.trim_start_matches("0x").to_ascii_lowercase();
                // `=bcast` broadcasts the pthread_cond at that address; `=0xVAL`
                // writes the exact u64 value repeatedly; a bare `--kicker ADDR`
                // (no explicit `=`) keeps the historical 1->2 lifecycle pulse.
                let mode = if val_l == "bcast" {
                    KickerMode::Broadcast
                } else if spec.contains('=') {
                    let v = u64::from_str_radix(val_s.trim_start_matches("0x"), 16).expect("bad kicker val");
                    KickerMode::Fixed(v)
                } else {
                    KickerMode::Pulse
                };
                kickers.push((addr, mode));
            }
            i += 1;
        }
        for (addr, mode) in kickers {
            std::thread::spawn(move || {
                let what = match mode {
                    KickerMode::Broadcast => "pthread_cond_broadcast".to_string(),
                    KickerMode::Fixed(v) => format!("write 0x{v:x}"),
                    KickerMode::Pulse => "pulse 1->2".to_string(),
                };
                eprintln!("[elfjit:kicker] host thread drives 0x{addr:x} ({what})");
                let bc: unsafe extern "C" fn(*const u8) -> i32 = unsafe {
                    std::mem::transmute(libc::dlsym(libc::RTLD_NEXT, c"pthread_cond_broadcast".as_ptr()))
                };
                for it in 0..400 {
                    unsafe {
                        match mode {
                            KickerMode::Broadcast => {
                                bc(addr as *const u8);
                            }
                            KickerMode::Fixed(v) => {
                                *((addr) as *mut u64) = v;
                            }
                            KickerMode::Pulse => {
                                // PULSE: hold 1 through the first gate (init poll
                                // wants *pred==1), then set 2 — the wait loops while
                                // *pred==1 (cd7c b.eq) and proceeds only when
                                // *pred !=1 and !=0 (cd84 cbz-on-zero); 2 is the
                                // terminal "done" state.
                                let v = if it < 60 { 1u64 } else { 2u64 };
                                *((addr) as *mut u64) = v;
                            }
                        }
                        if it % 100 == 0 {
                            eprintln!("[elfjit:kicker] t={it} guest_global 0x{addr:x}=%{:#x}", *((addr) as *const u64));
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            });
        }
        // Synthetic app-command feed (JIT_DRIVE_LIFECYCLE): a host thread pushes
        // Android lifecycle commands into the ALooper app-command queue, so a
        // GameActivity main loop that reaches `ALooper_pollOnce` dispatches
        // APP_CMD_START then APP_CMD_RESUME (the two commands that precede a real
        // EGL context / first frame on Android) instead of spinning on the empty
        // queue. `post_app_command` is the same channel the ALooper shim drains.
        if std::env::var_os("JIT_DRIVE_LIFECYCLE").is_some() {
            use arm64jit::shims::post_app_command;
            std::thread::spawn(|| {
                for (it, cmd) in [
                    arm64jit::shims::APP_CMD_START,
                    arm64jit::shims::APP_CMD_RESUME,
                    arm64jit::shims::APP_CMD_INIT_WINDOW,
                ]
                .iter()
                .enumerate()
                {
                    eprintln!("[elfjit:appcmd] posting APP_CMD_{it} ({cmd})");
                    post_app_command(*cmd);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });
        }
        // Real desktop X11 window for the ANativeWindow layer (GRAPHICS_-
        // RECOMMENDATION §5.3). Under JIT_DRIVE_LIFECYCLE bring up an Xvfb X
        // server, open a 1280x720 window, and register its XID as the guest's
        // ANativeWindow handle — SYNCHRONOUSLY before StartApp runs, so the
        // window is wired before the boot reaches the window/EGL surface path
        // (a racing spawned thread loses and hands the guest the sentinel).
        // Then eglCreateWindowSurface(dpy, config, win, ...) builds on a real
        // X11 window, not a fake address.
        if std::env::var_os("JIT_DRIVE_LIFECYCLE").is_some() {
            wire_real_window();
        }
        // Per-thread futex latch kicker (--futex-kick <period-ms>). The engine
        // main-loop idle barrier (cycle L) is a REAL per-thread futex: each
        // guest thread parks in guest_svc's FUTEX_WAIT_BITSET on its OWN latch
        // (uaddr = x1 = x19+4, awaited val 0xF4240) at call-site lr=0x10284d134
        // — a wait-until-changed tick/frame barrier. A host-side producer must
        // CHANGE the latch value and FUTEX_WAKE it to release the wait, else
        // the loop re-parks (a plain WAKE is a spurious wake; the value is
        // still the awaited one, so the futex immediately re-blocks). This was
        // unreachable by the static --kicker (which only writes fixed guest
        // globals). The sampler already exposes each parked thread's x1, so we
        // locate the per-thread latch live and write a value != awaited before
        // waking — advancing the loop one tick per kick into egl*/gl*.
        if let Some(hex) = {
            let args: Vec<String> = std::env::args().collect();
            args.iter()
                .position(|a| a == "--futex-kick")
                .and_then(|i| args.get(i + 1).cloned())
        } {
            let period_ms: u64 = hex.trim().parse().expect("--futex-kick needs integer period-ms");
            // Optional --futex-set <hex>: write a SPECIFIC latch value each tick
            // (the awaited token) instead of the free-running old+1. This tests
            // whether the idle barrier is a fixed "go" token (0xF4240) that the
            // producer must write verbatim, vs a pure version-counter (wait-until-
            // changed) where any new value works. `old.wrapping_add(1)` cannot
            // distinguish: if the waiter re-arms to a constant each cycle, a fixed
            // write is the correct producer signal and a version increment is a
            // stray number the loop ignores.
            let set_val: Option<i32> = {
                let args: Vec<String> = std::env::args().collect();
                args.iter()
                    .position(|a| a == "--futex-set")
                    .and_then(|i| args.get(i + 1).cloned())
                    .map(|v| i32::from_str_radix(v.trim_start_matches("0x"), 16).expect("--futex-set needs hex i32"))
            };
            const IDLE_FUTEX_CALLSITE: u64 = 0x10284d134; // guest lr when parked in the idle barrier
            // --futex-bump: the engine idle barrier is a wait on a VERSIONED
            // object. The parked consumer (wait-with-timeout 0x10284d018,
            // reached via blr — vtable-dispatched) gates on
            //   ldar x8,[Q]; cmp x21, x8 lsr#32   (0x2856ef4/efc)
            // where Q = t.x19 (arg0), and [Q+4] (== t.x1) is the futex latch.
            // It only PROCEEDS past the park when the version word [Q] high-32
            // CHANGES — a bare latch poke (--futex-kick/--futex-set) is not a
            // producer. --futex-bump also increments [Q] high-32 (version) so
            // the consumer's proceed-gate opens.
            let bump = {
                let args: Vec<String> = std::env::args().collect();
                args.iter().any(|a| a == "--futex-bump")
            };
            std::thread::spawn(move || {
                if let Some(v) = set_val {
                    eprintln!("[elfjit:futexkick] driving idle futex latch every {period_ms} ms, WRITING FIXED {v:#x} (awaited-token test)");
                } else if bump {
                    eprintln!("[elfjit:futexkick] driving idle barrier every {period_ms} ms, BUMPING version [Q]>>32 + latch (real producer shape)");
                } else {
                    eprintln!("[elfjit:futexkick] driving per-thread idle futex latch every {period_ms} ms");
                }
                for it in 0..6000 {
                    std::thread::sleep(std::time::Duration::from_millis(period_ms));
                    for t in arm64jit::jit::snapshot_threads() {
                        if t.lr != IDLE_FUTEX_CALLSITE {
                            continue;
                        }
                        let latch = t.x1; // per-thread futex uaddr (== x19+4)
                        // The latch must be host-addressable (guest==host map).
                        if latch < 0x100000000 || latch >> 56 != 0 {
                            continue;
                        }
                        // A futex uaddr is a 4-byte `int` (4-aligned) — read as a
                        // c_int, never as a u64 (the 4-aligned address misaligns).
                        let old = unsafe { *(latch as *const libc::c_int) };
                        // Version-counter futex: the waiter captures *latch as
                        // its "expected" value and blocks WHILE *latch is
                        // unchanged. Releasing it requires writing a NEW value
                        // (increment the version — never reuse the previous or
                        // the next waiter captures that same value and
                        // re-blocks; a fixed write is a self-defeating one-off).
                        // Gate is the exact idle call-site.
                        let nv = set_val.unwrap_or_else(|| old.wrapping_add(1));
                        // --futex-bump: also increment the VERSION word [Q]
                        // high-32 so the consumer's proceed-gate
                        // (cmp x21, [Q]>>32 at 0x2856efc) opens. Q = t.x19
                        // is a HOST-heap address (0x7f...), writable like the
                        // latch (t.x1 = Q+4); NOT a guest-image address.
                        if bump && t.x19 >= 0x100000000 && (t.x19 >> 56) == 0 && t.x19 & 7 == 0 {
                            let q = t.x19 as *mut u64;
                            let cur = unsafe { *q };
                            let nv_q = cur.wrapping_add(0x1_0000_0000);
                            unsafe { *q = nv_q };
                            if it % 50 == 0 {
                                eprintln!("[elfjit:futexkick] it={it} BUMP [Q]={:#x} ver {:#x}->{:#x}",
                                    q as usize, (cur >> 32), (nv_q >> 32));
                            }
                        }
                        unsafe { *(latch as *mut libc::c_int) = nv };
                        unsafe {
                            libc::syscall(
                                libc::SYS_futex,
                                latch as usize,
                                libc::FUTEX_WAKE as i64,
                                1i64,
                                0usize,
                            );
                        }
                        if it % 50 == 0 {
                            eprintln!(
                                "[elfjit:futexkick] it={it} guest_tid={} latch={latch:#x} old={old:#x}->{nv:#x}",
                                t.guest_tid
                            );
                        }
                    }
                }
            });
        }
        // Host-side task-deque PRODUCER (--deque-node <vtable-hex>). The cycle
        // SH5 frontier is that the parked threads are CONSUMERS of a per-CPU
        // lock-free task-deque (fns 0x285682c / 0x2856e40): each parks in the
        // generic version-epoch futex wait 0x10284d018 on Q'=t.x19 (futex at
        // Q'+4=t.x1) because the deque head-cell ([root]=0x10682a638 /
        // 0x10682b338) points at the self-referential SENTINEL (the drain
        // struct, [headcell].next==0). Version+latch bumping alone
        // (--futex-bump) re-parks — there is no work in the deque. This flag
        // makes a real PRODUCER: it CAS-es a freshly allocated task NODE into
        // the deque head-cell, links it into the circular intrusive list
        // (node.next = the old sentinel head), sets [node+112]=<vtable> so the
        // drain's dispatch ([node+112]&~0x3f -> [vt+40]) reaches a real guest
        // handler, then bumps [Q']>>32 (epoch) + FUTEX_WAKE on Q'+4. A zeroed
        // node (vt=0) trips the drain at [vt+40]=[0x28]; supplying the sentinel
        // vtable 0x106829f00 reaches the real engine handler 0x10285371c — the
        // first controlled crossing, even if that handler then faults on the
        // foreign node's task content.
        if let Some(vt) = {
            let args: Vec<String> = std::env::args().collect();
            args.iter()
                .position(|a| a == "--deque-node")
                .and_then(|i| args.get(i + 1).cloned())
                .map(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).expect("--deque-node needs hex vtable"))
        } {
            // --deque-node-bump: also bump [Q']>>32 + FUTEX_WAKE. NOTE: this is
            // SELF-DEFEATING per the drain's version gate (a changed version makes
            // the drain return instead of pop on its timeout poll) — kept for the
            // comparison data. Default (no bump) lets the consumer's natural
            // timeout poll drain the node we placed.
            let bump_version = std::env::args().any(|a| a == "--deque-node-bump");
            const IDLE: u64 = 0x10284d134; // parked consumer call-site
            std::thread::spawn(move || {
                use std::collections::HashSet;
                let mut enqueued: HashSet<u64> = HashSet::new();
                let mut placed: Vec<(u64, u64, u64)> = Vec::new(); // (headcell+0x10, node, Q')
                eprintln!("[elfjit:deque-producer] host enqueue on parked consumers (node vtable 0x{vt:x}, bump_version={bump_version})");
                for it in 0..300 {
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    // Post-enqueue verification: did the parked consumer wake and
                    // pop our node (head-cell back to the sentinel / off our node)?
                    if !placed.is_empty() {
                        let mut all_popped = true;
                        for (hc, np, qp) in placed.iter() {
                            let cur = unsafe { *(*hc as *const u64) };
                            let popped = cur != *np;
                            if !popped {
                                all_popped = false;
                            }
                            if it % 25 == 0 || popped {
                                eprintln!("[elfjit:deque-producer] check headcell={hc:#x} node={np:#x} now={cur:#x} popped={popped} Q'={qp:#x}");
                            }
                        }
                        if all_popped {
                            eprintln!("[elfjit:deque-producer] ALL placed nodes popped by consumers — deque crossed the barrier");
                            break;
                        }
                    }
                    for t in arm64jit::jit::snapshot_threads() {
                        if t.lr != IDLE {
                            continue;
                        }
                        if enqueued.contains(&t.guest_tid) {
                            continue;
                        }
                        let is_ptr = |p: u64| p >= 0x100000000 && p >> 56 == 0 && p & 7 == 0;
                        let sp = t.sp;
                        if !is_ptr(sp) {
                            continue;
                        }
                        // Parked drain saved its callee-saved registers at
                        // stp x20,x19,[sp,#64]: [sp+64]=drain root (the deque
                        // root ptr), [sp+72]=drain struct (the sentinel).
                        let root = unsafe { *(sp as *const u64).add(8) };
                        let sentinel = unsafe { *(sp as *const u64).add(9) };
                        if !is_ptr(root) || !is_ptr(sentinel) {
                            continue;
                        }
                        // The root points at a guest-bss head-CELL; its value is
                        // the deque head (now = sentinel = empty).
                        let headcell = unsafe { *(root as *const u64) };
                        if !is_ptr(headcell) {
                            continue;
                        }
                        let old = unsafe { *(headcell as *const u64) };
                        // Only enqueue when the head is still the empty sentinel
                        // (don't stack nodes over an already-pending one).
                        if old != sentinel {
                            continue;
                        }
                        // Allocate guest-visible task node (guest==host here).
                        let node = unsafe { libc::calloc(1, 256) as *mut u8 };
                        if node.is_null() {
                            continue;
                        }
                        let np = node as u64;
                        let qw = unsafe { *(t.x19 as *const u64) };
                        unsafe {
                            *(np as *mut u64) = 0; // node.next = null (this node becomes the tail)
                            (np as *mut u64).add(14).write_volatile(vt); // [node+112] = vtable
                            // WAIT — the drain's POP reads the head-node cell at
                            // [headcell + 0x0] (drain 0x2856f94: `ldr x23,[x20];
                            // ldar x24,[x23]` where x23 = [x20] = headcell, so the
                            // popped node = the VALUE at [headcell]). Prior cycles
                            // wrote to slot+0x10/0x18 (the ring arena's HEAD/TAIL
                            // internals) which the pop never reads — that is why
                            // nodes sat unconsumed. The real head-node cell the pop
                            // drains is offset +0x0. Publish our node there.
                            (headcell as *mut u64).write_volatile(np); // [headcell+0] = head node
                            // The drain's tag guard (0x2856e6c-78): `ldr x26,[x1,#104];
                            // ldr x24,[x23]; cmp x9, x24 lsr#48; b.ne ret` requires the
                            // head-node's high-16 tag == [headcell+8]. Publish the
                            // node's own tag word there so the guard passes.
                            (headcell as *mut u64).add(1).write_volatile(np >> 48);
                            // Keep next/self-link sane: node.next=0 (tail).
                            *((np as *mut u64)) = 0;
                            // Bump the wait object's version epoch so the parked
                            // consumer's proceed-gate (cmp [Q']>>32) opens. NOTE:
                            // self-defeating — see --deque-node-bump above.
                            if bump_version {
                                *(t.x19 as *mut u64) = qw.wrapping_add(0x1_0000_0000);
                                libc::syscall(
                                    libc::SYS_futex,
                                    t.x1 as usize,
                                    libc::FUTEX_WAKE as i64,
                                    1i64,
                                    0usize,
                                );
                            } else {
                                // Even without a version bump, a plain FUTEX_WAKE
                                // lets the drain's wait return; with --drain-poll
                                // forcing a finite timeout it re-enters the pop-loop
                                // and sees our node in [headcell+0].
                                libc::syscall(
                                    libc::SYS_futex,
                                    t.x1 as usize,
                                    libc::FUTEX_WAKE as i64,
                                    1i64,
                                    0usize,
                                );
                            }
                        }
                        eprintln!(
                            "[elfjit:deque-producer] enqueued node={:#x} into headcell[+0]={:#x} Q'{:#x} epoch {:#x} futex={:#x} guest_tid={}",
                            np, headcell, t.x19, qw >> 32, t.x1, t.guest_tid
                        );
                        enqueued.insert(t.guest_tid);
                        placed.push((headcell, np, t.x19));
                    }
                }
            });
        }
        // --deque-node-live <vt-hex>: inject a REAL task node into the LIVE
        // drainer's deque (guest_tid 0 under --drain-poll), NOT the parked
        // consumers' deques (tids 1/2) that --deque-node targets. This is the
        // SH7 documented next lever: the drain (0x2856e40) pop-loop at
        // 0x2856f94 reads the head node from [[root]] (x23=[x20]=[root],
        // x24=ldar[x23]=packed head), CAS-pops it, and — when it is not the
        // sentinel AND [node+40] != 0 AND [vt+40] != 0 — dispatches
        // [vt+40]([vt+16], consumer, [node+32]&~1, node, 4, 0). The deque root
        // for the live drainer is its x20, STABLE across the drain body and
        // readable from the host snapshot. We capture it once and write the
        // node into the head-cell it drains. Injection is gated on the deque
        // head being empty (low48==0) / the sentinel to avoid stacking over a
        // pending node, and we verify the node was popped (head-cell moved off
        // our packed value).
        if let Some(vt) = {
            let args: Vec<String> = std::env::args().collect();
            args.iter()
                .position(|a| a == "--deque-node-live")
                .and_then(|i| args.get(i + 1).cloned())
                .map(|v| {
                    if v == "probe" {
                        // Auto-build a HOST-THUNK PROBE vtable: [vt+40]=registered
                        // host thunk, [vt+16]=ctx marker. The drain dispatch of a
                        // FOREIGN node ([node+112]&~0x3f -> [vt+40]) then calls OUR
                        // probe with the real engine ABI args, firing the logging
                        // counter — the controlled type-4 crossing SH7b demanded.
                        // This avoids hand-resolving a real render/tick vtable.
                        extern "C" fn probe(a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, _a6: u64, _a7: u64) -> u64 {
                            use std::sync::atomic::{AtomicU64, Ordering};
                            static CNT: AtomicU64 = AtomicU64::new(0);
                            let c = CNT.fetch_add(1, Ordering::Relaxed) + 1;
                            // The drain re-enqueues every popped node, so the head
                            // stays = our node while it IS being dispatched; the
                            // only discriminating signal is this type-4 dispatch.
                            if c <= 3 || c % 10000 == 0 {
                                eprintln!(
                                    "[elfjit:deque-probe] type-4 dispatch #{c}: x0(vt+16)={a0:#x} x1(consumer)={a1:#x} x2(node+32&~1)={a2:#x} x3(node)={a3:#x} w4={a4} x5={a5}"
                                );
                            }
                            0
                        }
                        let probe_addr = arm64jit::jit::register_host_call_auto(probe);
                        // Vtable MUST live at a guest-visible address (< 2^48,
                        // mapped RW), not host heap: the drain does `ldr [vt+40]`
                        // as guest memory, so a host-heap vt (0x55..) reads garbage.
                        let v = unsafe { libc::calloc(1, 8 * 8) as *mut u8 };
                        let vt_host = v as u64;
                        let v = guest_arena_alloc(8 * 8) as *mut u8;
                        unsafe {
                            (v as *mut u64).add(2).write_volatile(0x_dead_beef); // [vt+16] ctx
                            (v as *mut u64).add(5).write_volatile(probe_addr); // [vt+40] handler
                        }
                        eprintln!(
                            "[elfjit:deque-node-live] PROBE vtable (vt=0x{:x} guest, host-def 0x{vt_host:x}, [vt+40]=0x{probe_addr:x}) — foreign-node dispatch will hit a registered host-thunk",
                            v as u64
                        );
                        v as u64
                    } else {
                        u64::from_str_radix(v.trim_start_matches("0x"), 16).expect("--deque-node-live needs hex vtable or 'probe'")
                    }
                })
        } {
            // Drain body span (guest vaddrs) where the drain holds x20 = deque root.
            const DRAIN_LO: u64 = 0x102856e40;
            const DRAIN_HI: u64 = 0x1028570a4;
            // Optional --deque-arg2 <hex>: override [node+32] of the injected node
            // (the drain passes it as dispatch arg2, x2 = [node+32]&~1). Default
            // keeps the cloned sentinel's [node+32] (or 0). Sweeping this value is
            // the controllable node-content selector into the real dispatcher.
            let arg2_override: Option<u64> = std::env::args()
                .position(|a| a == "--deque-arg2")
                .and_then(|i| std::env::args().nth(i + 1))
                .map(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).expect("--deque-arg2 needs hex"));
            std::thread::spawn(move || {
                use std::sync::atomic::{AtomicU64, Ordering};
                static ROOT: AtomicU64 = AtomicU64::new(0);
                static PLACED: AtomicU64 = AtomicU64::new(0);
                static HEADCELL: AtomicU64 = AtomicU64::new(0);
                eprintln!(
                    "[elfjit:deque-node-live] inject into LIVE drainer's deque (vtable 0x{vt:x}); draining when pc in [0x{DRAIN_LO:x},0x{DRAIN_HI:x})"
                );
                for it in 0..400 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let is_ptr = |p: u64| p >= 0x100000000 && p >> 56 == 0 && p & 7 == 0;
                    // Already placed a node?
                    let np = PLACED.load(Ordering::Relaxed);
                    if np != 0 {
                        let hc = HEADCELL.load(Ordering::Relaxed);
                        let cur = unsafe { *(hc as *const u64) };
                        let popped = cur != np;
                        if popped {
                            static POPS: AtomicU64 = AtomicU64::new(0);
                            let p = POPS.fetch_add(1, Ordering::Relaxed) + 1;
                            eprintln!(
                                "[elfjit:deque-node-live] NODE 0x{np:x} POPPED by live drainer (headcell now 0x{cur:x}) — dispatch #{}; re-injecting a fresh node to sustain the type-4 dispatch loop", p
                            );
                            // Reset so the next iteration places a NEW node at head
                            // (the drain consumed this one and its head is empty).
                            PLACED.store(0, Ordering::Relaxed);
                            HEADCELL.store(0, Ordering::Relaxed);
                            continue;
                        }
                        if it % 20 == 0 {
                            eprintln!("[elfjit:deque-node-live] node 0x{np:x} still head (headcell=0x{cur:x})");
                        }
                        continue; // keep polling until popped
                    }
                    // Recon for the first ~3 ticks only (a tiny window): the new
                    // SH11 strategy needs OUR node at head BEFORE the first forced
                    // pop, so we must inject almost immediately. The --drain-force-
                    // pop path faults the sentinel-as-task at ~200ms, so a 2s recon
                    // (SH9's it<40) structurally loses the race. Collapse recon to
                    // a one-shot diagnostic, then inject right away.
                    if it < 3 {
                        let snaps = arm64jit::jit::snapshot_threads();
                        let mut rr = 0u64;
                        for t in &snaps {
                            if t.pc >= DRAIN_LO && t.pc < DRAIN_HI && is_ptr(t.x20) {
                                rr = t.x20;
                                break;
                            }
                        }
                        if rr != 0 && is_ptr(rr) {
                            ROOT.store(rr, Ordering::Relaxed);
                            let cell = unsafe { *(rr as *const u64) };
                            if is_ptr(cell) {
                                let head = unsafe { *(cell as *const u64) };
                                let headnode = head & 0xffff_ffff_ffff;
                                eprintln!(
                                    "[elfjit:deque-node-live][recon it={it}] root={rr:#x}[0]={cell:#x}[8]={:#x} headcell[0]=0x{head:x} low48={headnode:#x}",
                                    unsafe { *(rr as *const u64).add(1) }
                                );
                                if is_ptr(headnode) {
                                    let rd = |base: u64, o: usize| unsafe { *(base as *const u64).add(o / 8) };
                                    let v112 = rd(headnode, 112);
                                    let vt = v112 & !0x3f;
                                    let vt40 = if is_ptr(vt) { rd(vt, 40) } else { 0 };
                                    eprintln!(
                                        "[elfjit:deque-node-live][recon] headnode={headnode:#x} +40={:#x} +112={v112:#x} vt={vt:#x} [vt+40]={vt40:#x}",
                                        rd(headnode, 40)
                                    );
                                }
                            }
                        }
                        // fall through to inject on ticks >= 1 (root may be 0 on
                        // tick 0; re-captured below if so).
                    }
                    // Capture the live drainer's deque root once.
                    let root = ROOT.load(Ordering::Relaxed);
                    let snaps = arm64jit::jit::snapshot_threads();
                    let mut live_root = 0u64;
                    for t in &snaps {
                        // Drain body in progress -> x20 IS the deque root.
                        if t.pc >= DRAIN_LO && t.pc < DRAIN_HI && is_ptr(t.x20) {
                            live_root = t.x20;
                            break;
                        }
                        // Just left the drain into the dispatch handler: x20
                        // may already be clobbered, but guest_tid 0's lr is a
                        // drain-body return address while the drain ran.
                    }
                    if root == 0 {
                        if live_root == 0 {
                            if it % 20 == 0 {
                                eprintln!("[elfjit:deque-node-live] waiting for live drainer pc in drain body (it={it})");
                            }
                            continue;
                        }
                        ROOT.store(live_root, Ordering::Relaxed);
                        eprintln!("[elfjit:deque-node-live] recovered live drainer deque root x20={live_root:#x}");
                    }
                    let root = ROOT.load(Ordering::Relaxed);
                    // headcell = [root]; the pop reads the packed head from it.
                    if !is_ptr(root) {
                        continue;
                    }
                    let headcell = unsafe { *(root as *const u64) };
                    if !is_ptr(headcell) {
                        continue;
                    }
                    let old = unsafe { *(headcell as *const u64) };
                    // Dump the deque struct neighborhood to reverse the exact
                    // layout (root -> headcell -> packed head) from live memory.
                    if it % 40 == 0 {
                        let r0 = unsafe { *(root as *const u64).add(0) };
                        let r1 = unsafe { *(root as *const u64).add(1) };
                        let r2 = unsafe { *(root as *const u64).add(2) };
                        let r3 = unsafe { *(root as *const u64).add(3) };
                        let h0 = unsafe { *(headcell as *const u64).add(0) };
                        let h1 = unsafe { *(headcell as *const u64).add(1) };
                        eprintln!(
                            "[elfjit:deque-node-live] root={root:#x}[0]={r0:#x}[8]={r1:#x}[+16]={r2:#x}[+24]={r3:#x} headcell={headcell:#x}[0]={h0:#x}(low48 {:#x})[8]={h1:#x}",
                            h0 & 0xffff_ffff_ffff
                        );
                    }
                    // The drain keeps the deque head non-empty (it
                    // continuously pops + re-enqueues the self/sentinel node),
                    // so there is no "empty" window to wait for. Inject by
                    // SWAPPING our node over the live head: the drain's next
                    // CAS-pop reads our packed value, truncates low-48 to our
                    // node, and dispatches it (non-sentinel, [node+40]!=0).
                    if it % 20 == 0 {
                        eprintln!("[elfjit:deque-node-live] headcell 0x{headcell:x} head=0x{old:x} (replacing with task node)");
                    }
                    // The drain's entry tag guard (0x2856e74) requires the head
                    // node's high-16 tag == [root+8]. Read that tag so the packed
                    // value passes the guard and the low-48 truncation yields our
                    // node on pop.
                    let tag = unsafe { *(root as *const u64).add(1) }; // [root+8]
                    // Bind the dispatch handler: [node+112]&~0x3f -> vt, [vt+40]=handler.
                    // CLONE the live head node's coherent payload as the base so
                    // the drain's post-dispatch RE-ENQUEUE (producer 0x285682c)
                    // walks valid link/refcount fields instead of zeroed garbage.
                    // The live head node (sentinel during idle, `low48(headcell[0])`)
                    // is a fully-constructed task node the drain already pops and
                    // re-enqueues every maintenance iteration — the ideal template.
                    // (SH9's "[consumer+104]" indexing is unreliable: the consumer
                    // x19 is rarely snapshotted in-body, so fall back to the head
                    // node, which is guaranteed present and coherent.)
                    let node: *mut u8 = {
                        let mut sentinel = 0u64;
                        let hn = old & 0xffff_ffff_ffff;
                        // Node MUST be guest-arena allocated: its address is
                        // low48-packed into the head cell AND the drain reads/
                        // writes its fields as guest memory, so a host-heap
                        // (0x7f2a...) node would be mangled by the pop's low48
                        // truncation (0x7f2a... -> 0x2a...) and fault.
                        let n = guest_arena_alloc(256) as *mut u8;
                        if !n.is_null() {
                            if is_ptr(hn) && hn != n as u64 {
                                // Copy head-node node-constructor layout (link + refcount
                                // + args + vtable handled below).
                                unsafe {
                                    std::ptr::copy_nonoverlapping(
                                        hn as *const u8, n, 256,
                                    );
                                }
                                sentinel = hn;
                                eprintln!(
                                    "[elfjit:deque-node-live] cloned head node 0x{sentinel:x} as node base (headcell[0]=0x{old:x}) -> guest node {:#x}",
                                    n as u64
                                );
                            } else {
                                eprintln!(
                                    "[elfjit:deque-node-live] no coherent head-node template, using zeroed node (may crash on re-enqueue)"
                                );
                            }
                        }
                        n
                    };
                    if node.is_null() {
                        continue;
                    }
                    let np = node as u64;
                    unsafe {
                        // Fresh tail: the re-enqueue producer (0x285682c) walks the
                        // node's [node+0] next-link to find the tail; the *cloned*
                        // head-node template still points at the old sentinel ring,
                        // so zero it to a clean tail before publishing (else the
                        // producer follows the stale link and faults at pc 0x51).
                        (np as *mut u64).write_volatile(0);
                        // [node+112] = vtable; [vt+40] must be a real handler fn.
                        (np as *mut u64).add(14).write_volatile(vt);
                        // [node+40] != 0 so the drain DISPATCHES the handler on pop.
                        (np as *mut u64).add(5).write_volatile(
                            ((np as *const u64).add(5).read_volatile()) | 1,
                        );
                        // [node+32] = arg (dispatch arg2 = [node+32]&~1); keep
                        // sentinel's (or 0) unless --deque-arg2 overrides it.
                        (np as *mut u64).add(4).write_volatile(
                            arg2_override.unwrap_or_else(|| {
                                (np as *const u64).add(4).read_volatile()
                            }),
                        );
                        // Pack: low48 = node pointer (so pop truncates to it),
                        // high16 = tag matching [root+8].
                        let packed = np | ((tag & 0xffff) << 48);
                        // Publish into the head-cell the drain pops from.
                        (headcell as *mut u64).write_volatile(packed);
                        HEADCELL.store(headcell, Ordering::Relaxed);
                        PLACED.store(packed, Ordering::Relaxed);
                        // ARM FORCE-POP (deferred from startup when --deque-node-live
                        // is set): now that OUR node is placed at head, patch the
                        // drain's pop-loop to always fall through — `mov w24,w0`
                        // (0x102856f4c) -> mov w24,#1 and NOP the tbz (0x102856f7c) —
                        // so the next drain iteration pops+dispatches OUR foreign
                        // node (passes the self-skip guard, [node+40]=1 -> probe),
                        // NOT the sentinel. This is the SH11 sequencing lever: stable
                        // drain while placing, force-pop only after placement.
                        let arm = [0x102856f4cu64, 0x102856f7cu64];
                        for a in arm {
                            let p = a & !0xfff;
                            unsafe {
                                libc::mprotect(p as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_WRITE);
                            }
                            let before = unsafe { *(a as *const u32) };
                            let word = if a == 0x102856f7c { 0xd503_201fu32 /* NOP */ } else { 0x5280_0018u32 /* mov w24,#1 */ };
                            unsafe { *(a as *mut u32) = word };
                            unsafe {
                                libc::mprotect(p as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_EXEC);
                            }
                            eprintln!(
                                "[elfjit:deque-node-live] ARMED force-pop {:#x} (was {before:08x}) -> {word:08x}",
                                a
                            );
                        }
                        // The drain body was already compiled (unpatched) into the
                        // block cache; drop those entries so the dispatcher
                        // recompiles it from the now-patched guest bytes on the
                        // next re-entry (otherwise the force-pop has no effect).
                        arm64jit::jit::block_cache_drop_region(0x102856e40, 0x1028570c0);
                        eprintln!(
                            "[elfjit:deque-node-live] dropped cached drain blocks [0x102856e40,0x1028570c0) — pop-loop will recompile patched"
                        );
                        eprintln!(
                            "[elfjit:deque-node-live] INJECTED node 0x{np:x} packed=0x{packed:x} into headcell 0x{headcell:x} (tag {tag:#x}) — awaiting pop by live drainer"
                        );
                    }
                }
                eprintln!("[elfjit:deque-node-live] gave up after 400 ticks");
            });
        }
        // --taskv4-seed <probe|guest-hex>: populate the dispatcher's TYPE-4
        // popped-task handler vector at guest BSS 0x106829ea8
        // (dispatcher 0x10285371c w4=4 path: `adrp x8,6829000; ldr x3,[x8,#3752];
        // br x3` at file 0x2853788/0x28537b8). During headless boot this vector is
        // 0 (a NULL .bss function ptr a real framework producer would install), so
        // the drain's type-4 dispatch of ANY popped task node returns at
        // 0x285378c->0x2853af0 doing nothing — the exact mechanical reason no
        // injected/foreign node can drive the engine toward a frame. Seeding it
        // with a registered HOST-THUNK probe (or a chosen guest fn) lets a
        // sentinel-vtable node pop through the REAL dispatcher w4=4 plane and hit
        // our vector, proving the plane is dispatchable when the slot is live.
        if let Some(spec) = std::env::args()
            .position(|a| a == "--taskv4-seed")
            .and_then(|i| std::env::args().nth(i + 1))
        {
            const TASKV4: u64 = 0x106829ea8;
            use std::sync::atomic::{AtomicU64, Ordering};
            static V4: AtomicU64 = AtomicU64::new(0);
            extern "C" fn v4probe(a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, _a6: u64, _a7: u64) -> u64 {
                let c = V4.fetch_add(1, Ordering::Relaxed) + 1;
                if c <= 3 || c % 5000 == 0 {
                    eprintln!(
                        "[elfjit:taskv4] type-4 task handler #{c}: fnarg0(x0)={a0:#x} arg1={a1:#x} arg2={a2:#x} node={a3:#x} w4={a4} x5={a5}"
                    );
                }
                0
            }
            let seed = if spec == "probe" {
                arm64jit::jit::register_host_call_auto(v4probe)
            } else {
                u64::from_str_radix(spec.trim_start_matches("0x"), 16).expect("--taskv4-seed needs 'probe' or a hex guest fn addr")
            };
            unsafe {
                *(TASKV4 as *mut u64) = seed;
                eprintln!(
                    "[elfjit:taskv4] seeded dispatcher type-4 vector [0x{TASKV4:x}] = {seed:#x}{}",
                    if seed != 0 { " — a popped task node reaching w4=4 will now call it" } else { " (cleared)" }
                );
            }
        }
        // --deque-probe: convert the forced-pop sentinel fault into a CONTROLLED
        // type-4 dispatch the SH7b frontier demanded. The engine's real pop-loop
        // (0x2856f94) pops the head node and dispatches
        //   [node+112]&~0x3f -> vt; handler = [vt+40]; if [node+40]!=0 && handler!=0
        //   then handler([vt+16], x19=consumer, [node+32]&~1, node, w4=4, x5=0)
        // During idle the head node is the SENTINEL (the drain struct itself),
        // whose [node+112]=0x106829f00 -> [vt+40]=0x10285371c (the engine's own
        // dispatcher), which walks the sentinel's garbage task content and
        // strlen-faults (exit 134, the current unstable state). Instead of racing
        // a foreign node into the deque ahead of the fault, REPOINT the sentinel's
        // live [node+112] at a vtable WE control whose [vt+40] is a registered
        // host-thunk probe. Then every forced pop dispatches OUR probe with the
        // real engine ABI args (vt+16 / consumer / node+32 / node / w4=4 / 0),
        // stably, capturing the discriminate type-4 dispatch. Opt-in; default
        // --deque-node-live and plain --drain-force-pop unchanged.
        // --deque-probe <ctx-qw-hex> writes that qword to the sentinel's [node+32]
        // (the ABI arg passed as x2, &~1) so the probe proves which node road it.
        if std::env::args().any(|a| a == "--deque-probe") {
            let ctx = std::env::args()
                .position(|a| a == "--deque-probe")
                .and_then(|i| std::env::args().nth(i + 1))
                .map(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                .flatten();
            const DRAIN_LO: u64 = 0x102856e40;
            const DRAIN_HI: u64 = 0x1028570a4;
            use std::sync::atomic::{AtomicU64, Ordering};
            static PROBE_COUNT: AtomicU64 = AtomicU64::new(0);
            extern "C" fn probe(a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, _a6: u64, _a7: u64) -> u64 {
                let c = PROBE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if c == 1 || c % 10000 == 0 {
                    eprintln!(
                        "[elfjit:deque-probe] type-4 dispatch #{c}: x0(vt+16)={a0:#x} x1(consumer)={a1:#x} x2(node+32&~1)={a2:#x} x3(node)={a3:#x} w4={a4} x5={a5}"
                    );
                }
                0
            }
            // Allocate a guest-visible fake vtable: [vt+16] = ctx marker,
            // [vt+40] = probe host-thunk address (JIT routes guest `blr` to it).
            let vt = unsafe { libc::calloc(1, 8 * 8) as *mut u8 };
            let probe_addr = arm64jit::jit::register_host_call_auto(probe);
            let ctxv = ctx.unwrap_or(0);
            unsafe {
                (vt as *mut u64).add(2).write_volatile(ctxv); // [vt+16] (a0)
                // Handler slot is [vt+40] = byte 40 = u64 index 5 (same fix as the
                // --deque-node-live probe; writing index 4 reads 0 at [vt+40]).
                (vt as *mut u64).add(5).write_volatile(probe_addr); // [vt+40] (handler)
            }
            let vtaddr = vt as u64;
            std::thread::spawn(move || {
                eprintln!(
                    "[elfjit:deque-probe] probing sentinel dispatch (vt 0x{vtaddr:x}, probe 0x{probe_addr:x} -> [vt+40], ctx {ctxv:#x})"
                );
                let mut repointed: Vec<u64> = Vec::new();
                for it in 0..900 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    let is_ptr = |p: u64| p >= 0x100000000 && p >> 56 == 0 && p & 7 == 0;
                    let snaps = arm64jit::jit::snapshot_threads();
                    let mut roots: Vec<u64> = Vec::new();
                    for t in &snaps {
                        for r in [t.x20, t.x19] {
                            if is_ptr(r) && r >= 0x100000000 {
                                roots.push(r);
                            }
                        }
                        if t.lr == 0x10284d134 && is_ptr(t.sp) {
                            let r = unsafe { *(t.sp as *const u64).add(8) };
                            if is_ptr(r) {
                                roots.push(r);
                            }
                        }
                    }
                    roots.sort_unstable();
                    roots.dedup();
                    for rr in roots {
                        let headcell = unsafe { *(rr as *const u64) };
                        if !is_ptr(headcell) {
                            continue;
                        }
                        let head = unsafe { *(headcell as *const u64) };
                        let sentinel = head & 0xffff_ffff_ffff;
                        if !is_ptr(sentinel) || repointed.contains(&sentinel) {
                            continue;
                        }
                        let cur_v112 = unsafe { *((sentinel as *const u64).add(112 / 8)) };
                        if cur_v112 == 0x106829f00 {
                            unsafe {
                                (sentinel as *mut u64).add(112 / 8).write_volatile(vtaddr);
                            }
                            repointed.push(sentinel);
                            eprintln!(
                                "[elfjit:deque-probe] REPOINTED sentinel 0x{sentinel:x} (root 0x{rr:x}, headcell 0x{headcell:x}): [node+112] 0x{cur_v112:x}->0x{vtaddr:x}"
                            );
                        }
                    }
                    let cnt = PROBE_COUNT.load(Ordering::Relaxed);
                    if cnt >= 5 && it % 40 == 0 {
                        eprintln!(
                            "[elfjit:deque-probe] CONFIRMED {cnt} controlled type-4 dispatches through our vtable"
                        );
                    }
                }
                eprintln!("[elfjit:deque-probe] gave up (probe count={}, repointed={})", PROBE_COUNT.load(Ordering::Relaxed), repointed.len());
            });
        }
        // Disable the gate-2 re-arm store: the owner's cond-wait loop at
    // 0x102b4cd50/0x102b4cd84 re-parks while *x19==1 and, on seeing that
    // pred has become 0, RE-ARMS it back to 1 (`mov x8,#1; str x8,[x19]` at
    // 0x102b4cdb0/0x102b4cdb4) so the terminal value driven from the host
    // never sticks. NOP the re-arm store so our value persists. JIT_DRIVE_*
    // mode only.
    if std::env::var_os("JIT_DRIVE_LIFECYCLE").is_some() {
        let rearm = el.guest_of(0x102b4cdb4);
        let page = rearm & !0xfff;
        if unsafe { libc::mprotect(page as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_WRITE) } == 0 {
            unsafe { *(rearm as *mut u32) = 0xd503_201fu32 }; // NOP
            unsafe { libc::mprotect(page as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_EXEC) };
            eprintln!("[kernel:NOP re-arm store 0x{rearm:x} (gate-2) under JIT_DRIVE_LIFECYCLE");
        }
    }
    // --drain-poll <ms>: force the engine idle-task-deque consumer's drain
    // (0x2856e40) to use a FINITE wait timeout instead of the infinite -1 it
    // blocks on during idle. The parked threads deadlock because
    // `mov x2,x22` (0x2856f40, x22=drain timeout arg = -1) hands generic-wait
    // 0x284d014 an infinite timeout -> it parks in a bare futex forever, so the
    // drain's pop-loop at 0x2856f94 (reached ONLY when the wait returns
    // timed-out w0=1 AND the version matches) never runs. Patching that copy to
    // a finite ms value makes the wait time out, the drain reach the pop-loop,
    // find a host-placed task node in [headcell+0], and dispatch [node+112]->[vt+40].
    // Patch the guest IMAGE before jit_run so the drain block compiles with it.
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--drain-poll") {
            let ms: u32 = args
                .get(i + 1)
                .expect("--drain-poll <ms>")
                .parse()
                .expect("--drain-poll needs integer ms");
            assert!(ms < 4096, "--drain-poll ms must be < 4096 (imm12)");
            // Patch the guest image (identity host mapping) BEFORE jit_run so the
            // drain block compiles with the finite timeout. The parked threads'
            // lr=0x10284d134 shows true guest addrs are in 0x1028xxxx, so the
            // instruction's true guest==host addr is 0x102856f40 directly (NOT
            // re-mapped via guest_of, which double-shifts to 0x202856f40).
            let insn_addr: u64 = 0x102856f40;
            let patch: u32 = 0xd280_0002 | (ms << 5); // mov x2, #ms (imm12<4096)
            let page = insn_addr & !0xfff;
            eprintln!("[elfjit:drain-poll] base_load=0x{:x} base_addr=0x{:x} guest_of(0x102856f40)=0x{:x}; read now={:08x}",
                el.info.base_load_addr, el.base_addr, el.guest_of(0x102856f40),
                unsafe { *(insn_addr as *const u32) });
            if unsafe { libc::mprotect(page as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_WRITE) } == 0 {
                unsafe { *(insn_addr as *mut u32) = patch };
                unsafe { libc::mprotect(page as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_EXEC) };
                eprintln!("[elfjit:drain-poll] patched 0x{insn_addr:x} -> mov x2,#{ms}ms (0x{patch:08x})");
            } else {
                eprintln!("[elfjit:drain-poll] WARN mprotect RW failed at 0x{page:x} errno={}", std::io::Error::last_os_error());
            }
            // SH7's --drain-poll claimed the finite timeout alone makes the
            // pop-loop run, but that is WRONG (corrected here): generic-wait
            // 0x284d014 maps the host futex's ETIMEDOUT (-110) return into w0=0
            // ("woken"), because `cmn x0,#1` (0x284d0a4) only treats an EXACT
            // x0==-1 as a timeout-under-deadline; -110 falls through to
            // 0x284d0ec and returns 0. So the drain's `tbz w24,#0` (0x2856f7c)
            // always re-loops and the pop-loop 0x2856f94 never runs (measured:
            // 0 hits / 128k drain branches). Forcing the pop-loop itself (the
            // real crossing) needs the drain's wait-result latch AND the tbz:
            // `mov w24,w0` at 0x102856f4c -> mov w24,#1, and NOP the tbz
            // 0x102856f7c so the drain falls through to the version-check and
            // the pop-loop, which then CAS-pops and dispatches a placed node.
            // This reaches previously-dead code and faults on dispatch of a
            // non-real task node (the "controlled first crossing"), so it is
            // opt-in via --drain-force-pop; plain --drain-poll keeps its
            // documented stable (finite-timeout maintenance heartbeat) behavior.
            let force = std::env::args().any(|a| a == "--drain-force-pop");
            // If --deque-node-live is also present, DEFER the force-pop patches to
            // the injector thread (see its "arm force-pop" step): patching here at
            // startup makes the drain pop the SENTINEL as the first task and fault
            // (~200ms) before any injected node can land. Left unpatched here, the
            // drain stays stable (never pops) while we place our node, then the
            // injector arms the pop-loop so the FIRST forced pop takes OUR foreign
            // node (passes the self-node-skip guard, [node+40]=1) and dispatches it.
            let deferred = std::env::args().any(|a| a == "--deque-node-live");
            if force && !deferred {
            let latch_addr: u64 = 0x102856f4c; // mov w24,w0 (=0x2a0003f8)
            let _latch_patch: u32 = 0x52800018; // mov w24,#1 (MOVZ W24,#1)
            let tbz_addr: u64 = 0x102856f7c;
            let tbz_page = tbz_addr & !0xfff;
            for (a, name) in [(latch_addr, "w24"), (tbz_addr, "tbz")] {
                let p = a & !0xfff;
                if unsafe { libc::mprotect(p as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_WRITE) } != 0 {
                    eprintln!("[elfjit:drain-poll] WARN mprotect RW failed at {name} 0x{p:x} errno={}", std::io::Error::last_os_error());
                    continue;
                }
                let before = unsafe { *(a as *const u32) };
                let patch_word: u32 = if a == tbz_addr { 0xd503_201f /* NOP */ } else { 0x5280_0018 /* mov w24,#1 */ };
                unsafe { *(a as *mut u32) = patch_word };
                let _ = unsafe { libc::mprotect(p as *mut libc::c_void, 4096, libc::PROT_READ | libc::PROT_EXEC) };
                eprintln!("[elfjit:drain-poll] FORCE pop-loop: patched {name} 0x{a:x} (was {before:08x}) -> {patch_word:08x}");
            }
            let _ = tbz_page;
            }
        }
    }

    // JIT_FRAMEWORK_DUMP: StartApp's jit_run below parks the main thread in the
    // engine main loop and never returns, so a post-run sampler would never
    // run. Instead spawn a detached host thread that samples the framework-built
    // globals (guest==host addressing) every ~500 ms while StartApp initializes
    // and parks, so we learn whether the render-init context (0x1067d16f0) or
    // the deque-maintenance forward-edges (0x1068262e8/300/308) get POPULATED
    // at runtime — i.e. whether driving the real render-init after warm-up runs.
    if std::env::var_os("JIT_FRAMEWORK_DUMP").is_some() {
        std::thread::spawn(|| {
            let dw = |a: u64| -> u64 {
                if a >= 0x100000000 && a >> 56 == 0 && a & 7 == 0 {
                    unsafe { *(a as *const u64) }
                } else {
                    0
                }
            };
            for _ in 0..60 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let ctx = dw(0x1067d16f0);
                // Type-4 (popped task-node) dispatch vector: the dispatcher
                // 0x10285371c does `cmp w4,#4; ... adrp x8,6829000; ldr x3,[x8,#3752];
                // br x3` (file 0x2853788/0x28537b8) — the function pointer our injected
                // task nodes ACTUALLY call when popped. If it is 0 the type-4 path
                // returns at 0x285378c->0x2853af0 doing nothing. Also dump the
                // type-0/2 backup vector at [0x6826000+800]=0x106826320 (`br x4`).
                eprintln!(
                    "[elfjit:fw] render-ctx 0x1067d16f0={:#x} | deque-fwd 0x1068262e8={:#x} 0x106826300={:#x} 0x106826308={:#x} | task-v4 [0x106829ea8]={:#x} v0/2 [0x106826320]={:#x} | [*ctx]={:#x}",
                    ctx,
                    dw(0x1068262e8),
                    dw(0x106826300),
                    dw(0x106826308),
                    dw(0x106829ea8),
                    dw(0x106826320),
                    if ctx != 0 && ctx >> 56 == 0 { dw(ctx) } else { 0 },
                );
            }
        });
    }

    // --renderinit <link-addr>: after StartApp's init has populated the framework/
    // render context global 0x1067d16f0 (verified live 0x562a.. — SH14's
    // "statically 0, framework-gated, not drivable" is WRONG at runtime),
    // drive the engine's REAL EGL render-init (SH14 pinned eglGetDisplay->
    // eglInitialize->eglCreateContext->eglCreateWindowSurface->eglMakeCurrent at
    // fn 0x105b3a2d8 / thunk 0x105b3a280) directly. Runs on a DETACHED host
    // thread because StartApp's main-thread jit_run parks in the idle futex and
    // never returns; it sleeps `warmup` ms first so StartApp populates the
    // context. clear_block_cache on its top-level entry is SAFE (JitBlocks leak,
    // never munmap), so StartApp's parked threads just recompile on wake.
    let renderinit_args: Vec<String> = std::env::args().collect();
    // Clone the full arg list again for the opt-in --renderframe sub-mode (drives
    // the render-init THUNK then the swap fn to actually present a buffer).
    let renderframe_args: Vec<String> = std::env::args().collect();
    if let Some(i) = renderinit_args.iter().position(|a| a == "--renderinit") {
        let rhex = renderinit_args
            .get(i + 1)
            .cloned()
            .expect("--renderinit needs a link-addr hex");
        let link = u64::from_str_radix(rhex.trim_start_matches("0x"), 16)
            .unwrap_or_else(|_| panic!("bad --renderinit hex"));
        // NOTE: like the `disasm` example, the render-init addresses in the SH14
        // records are GUEST addresses (0x105b3a2d8 already includes the segment
        // base 0x100000000). Pass through directly — DO NOT `el.guest_of()` (that
        // would double-map to 0x205b3a2d8, outside the image, and jit_run would
        // reject it).
        let render_init = link;
        let warmup_ms = std::env::var("RENDERINIT_WARMUP_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5000);
        let (ibase, ilen, isp) = (base, len, st.x[31]);
        let tpidr = arm64jit::jit::current_guest_tp();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(warmup_ms));
            let iimg: &[u8] =
                unsafe { std::slice::from_raw_parts(ibase as *const u8, ilen) };
            let mut s3 = arm64jit::jit::CpuState::new();
            s3.tpidr = tpidr;
            s3.x[31] = isp;
            // render-init's prologue writes a resolved global ptr through its x0
            // param (real caller passes `[parent+344]`; a fresh call leaves x0=0
            // -> NULL store -> SIGSEGV). Point x0 at a guest-writable leaked
            // buffer so the first store lands and we reach the EGL sequence.
            let scratch = Box::leak(vec![0u8; 4096].into_boxed_slice());
            // --renderthunk (opt-in, must accompany --renderinit): drive the render-init
            // THUNK 0x105b3a280 instead of the inner fn, to recover the engine's REAL
            // ctx object. SH17's record "DON'T drive the thunk (SIGSEGV)" is WRONG —
            // the crash was from misplacing the harness args. Disasm of v2.738.1397:
            //   thunk(x0, x1):  x21=x0; x20=x1; x19=alloc_big(0x48);
            //                   inner(x19, x2?=x1=x21, x2=x20); ret x0=x19
            // i.e. thunk(win, parent) -> inner(alloc_ctx, win, parent) and returns the
            // real 0x48-byte guest ctx in x0 (engine's callers 0x5b2b214/0x5b2ea90 do
            // `bl 0x105b3a280` then `ldr x8,[x0]; ldr x8,[x8,#16]; blr x8` vtable-
            // dispatch). The engine's own frame-render path consumes THIS ctx, so
            // recovering it is the bridge to frontier lever (2) (drive the engine's
            // own frame-render machinery with a coherent renderer). The old harness
            // passed scratch as x0 -> inner took win=scratch (not the XID) and the
            // surface create rejected it. Correct drive: thunk(x0=win=XID, x1=parent=0).
            let render_thunk = renderframe_args.iter().any(|a| a == "--renderthunk");
            let xid = arm64jit::shims::anativewindow_xid();
            // Scratch is still needed: render-init's prologue stores the resolved
            // parent-global ptr through x0 only for the inner-fn path; the thunk's
            // inner call gets its OWN freshly-allocated ctx as x0, so it never touches
            // scratch — we keep it solely to pin guest-arena-visible RW backing and as
            // the fallback driver buffer if --renderthunk isn't set.
            s3.x[0] = if render_thunk { xid } else { scratch.as_ptr() as u64 };
            // Real caller (0x105b2ea98) passes x1 = the ANativeWindow (loaded from
            // [parent+352] into x22 -> stored to [ctx+24] -> eglCreateWindowSurface's
            // native-window arg). For the thunk, x1 is the (optional) share/ parent
            // context (0 = fresh, no sharing) — the window rides in x0 for the thunk
            // (it forwards x0 into inner's x1, i.e. the win). Mesa's x11 EGL platform
            // wants the X11 Window XID as its native window.
            s3.x[1] = if render_thunk { 0 } else { xid };
            let got = if ibase >= 0x100000000 && ibase >> 56 == 0 {
                unsafe { *(0x1067d16f0u64 as *const u64) }
            } else {
                0
            };
            eprintln!(
                "[elfjit:renderinit] driving {}{render_init:#x} after {warmup_ms}ms warm-up (ctx 0x1067d16f0={got:#x}, x0={:#x}, x1={:#x})",
                if render_thunk { "THUNK " } else { "" }, s3.x[0], s3.x[1],
            );
            let swap_result = arm64jit::jit::jit_run(iimg, ibase, render_init, &mut s3 as *mut CpuState);
            let rv = match swap_result {
                Err(e) => {
                    eprintln!("[elfjit:renderinit] stopped: {e}");
                    return;
                }
                Ok(r) => r,
            };
            eprintln!("[elfjit:renderinit] returned Ok({rv:#x})");
            // When driving the THUNK, x0's return value IS the engine's real ctx
            // (guest-addressable 0x48-byte object with its own vtable at [ctx+0] =
            // 0x106731ae0). Range-check it (>= some guest base, < 2^48, mapped) and
            // note that the engine's own frame callers deref it. Keep the swap/sclear
            // levers operating on THIS ctx (its [ctx+32]/[+40]/[+48] hold the live
            // EGL display/surface/context the inner fn stored).
            let real_ctx = if render_thunk {
                let c = rv;
                if c >= 0x100000000 && c >> 56 == 0 {
                    let vt = unsafe { *(c as *const u64) };
                    eprintln!(
                        "[elfjit:renderthunk] REAL ctx 0x{c:x} vtable=0x{vt:x} egl: display=0x{:x} surface=0x{:x} context=0x{:x}",
                        unsafe { *(c as *const u64).add(4) },
                        unsafe { *(c as *const u64).add(5) },
                        unsafe { *(c as *const u64).add(6) },
                    );
                    // Dump the live vtable slots (engine-populated at runtime, no
                    // static relocs). The engine's frame-render callers
                    // (0x5b2b214/0x5b2ea90) do `ldr x8,[ctx]; ldr x8,[x8,#16]; blr
                    // x8` — slot [vt+16] (index 2) is the method a real frame
                    // dispatch reaches. Read the first 5 table entries live.
                    if vt >= 0x100000000 && vt >> 56 == 0 {
                        let slots: Vec<String> = (0..5)
                            .map(|i| unsafe { *(vt as *const u64).add(i) })
                            .map(|v| format!("{v:#x}"))
                            .collect();
                        eprintln!(
                            "[elfjit:renderthunk] ctx vtable[0..5] = {} — [vt+16](idx2)=disp target",
                            slots.join(" ")
                        );
                    }
                    c
                } else {
                    eprintln!("[elfjit:renderthunk] thunk return 0x{c:x} not a guest ctx; falling back to scratch");
                    scratch.as_ptr() as u64
                }
            } else {
                scratch.as_ptr() as u64
            };
            let _ = &real_ctx;
            // --renderframe (opt-in, must accompany --renderinit): after the real
            // render-init ran, present a buffer through the engine's LIVE EGL
            // context. Reverse from the real binary (SH17 disasm): the direct
            // drive of the render-init inner fn (0x105b3a2d8) wrote the live EGL
            // handles into our `scratch` buffer — [scratch+32]=eglDisplay,
            // [scratch+40]=surface, [scratch+48]=context (str x0,[x19,#32] /
            // str x1,[x19,#40] / str x0,[x19,#48], x19=ctx=the fn's x0 param).
            // The swap fn 0x105b3b408 is a tail thunk `ldp x8,x1,[x0,#32]; mov
            // x0,x8; b eglSwapBuffers` — i.e. eglSwapBuffers([x0+32],[x0+40]).
            // Passing x0=scratch (the SAME buffer render-init wrote) makes the
            // engine's own swap path present the current surface headlessly
            // (llvmpipe+Xvfb), WITHOUT re-running the init (which crashes because
            // the thunk re-drive shifts the parent/window args). Same host thread
            // so the EGL context stays current.
            if renderframe_args.iter().any(|a| a == "--renderframe") {
                // --renderbind (opt-in): drive the engine's OWN make-current method
                // (ctx vtable [vt+16] = 0x105b3b358) before presenting, instead of
                // relying on the render-init having left the context current. This
                // is the exact code the engine's frame-render callers dispatch
                // (0x5b2b214 -> [vt+16] -> blr) when they (re)bind the GL context
                // before a swap/draw: it reads eglGetCurrentContext, and when not
                // already == [ctx+48] calls eglMakeCurrent([+32]display,
                // [+40]surface, [+40]surface, [+48]context). Driving it proves the
                // engine's own rebind path executes on the recovered ctx (the same
                // host thread keeps the context current afterwards so the following
                // swap/draw land on it).
                if renderframe_args.iter().any(|a| a == "--renderbind") {
                    let mut sb = arm64jit::jit::CpuState::new();
                    sb.tpidr = tpidr;
                    sb.x[31] = isp;
                    sb.x[0] = real_ctx; // method: this = ctx
                    match arm64jit::jit::jit_run(iimg, ibase, 0x105b3b358, &mut sb as *mut CpuState) {
                        Err(e) => eprintln!("[elfjit:renderbind] stopped: {e}"),
                        Ok(ok) => eprintln!(
                            "[elfjit:renderbind] engine make-current method returned Ok({ok:#x}) on ctx {real_ctx:#x} (eglMakeCurrent)"
                        ),
                    }
                }
                let swap_thunk = renderframe_args
                    .iter()
                    .position(|a| a == "--renderframe")
                    .and_then(|i| renderframe_args.get(i + 1).cloned())
                    .and_then(|h| u64::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0x105b3b408);
                unsafe {
                    eprintln!(
                        "[elfjit:renderframe] ctx={real_ctx:#x} [+32]=display {:#x} [+40]=surface {:#x} [+48]=context {:#x}",
                        *(real_ctx as *const u64).add(4),
                        *(real_ctx as *const u64).add(5),
                        *(real_ctx as *const u64).add(6),
                    );
                    *(real_ctx as *mut u64) = 0;
                }
                let mut s5 = arm64jit::jit::CpuState::new();
                s5.tpidr = tpidr;
                s5.x[31] = isp;
                s5.x[0] = real_ctx; // swap fn reads [x0+32]/[x0+40]
                match arm64jit::jit::jit_run(iimg, ibase, swap_thunk, &mut s5 as *mut CpuState) {
                    Err(e) => eprintln!("[elfjit:renderframe] swap stopped: {e}"),
                    Ok(ok) => eprintln!("[elfjit:renderframe] swap returned Ok({ok:#x}) (eglSwapBuffers)"),
                }
                // --renderframe-drive: probe how FAR the engine's OWN frame-render fn
                // 0x105b32c00 gets when driven on the real ctx with fabricated
                // renderer/view objects. This is frontier lever (2) — replacing the
                // harness's force-driven glClearColor/glClear with the engine's real
                // frame code. From SH18 disasm the fn is
                //   frame(renderer=x0, view=x1, w2, w3, x4, x5):
                //     [renderer+16]=1; x0=[renderer+24]; bl 0x5b2e98c   (find/dispatch)
                //     glBindFramebuffer(0x8d40, [view+140])  -> glGetError (cmp 0x505)
                //     glViewport(0,0,[view+128],[view+132])
                //     [renderer+24]->[+552]: if 0 skip clear path
                //     [renderer+40]->[+140]: if 0 skip clear path
                //     clear via glClearColor/glColorMask/glClearDepthf/...
                // We fabricate: renderer (with +16 set, +24->objA[+552]=1,
                // +40->objB[+140]=1), view (+128,+132 = 1280x720, +140 framebuffer 0).
                // Same host thread, context already current (renderbind/renderinit).
                if renderframe_args.iter().any(|a| a == "--renderframe-drive") {
                    // Guest-visible scratch for the objects (guest==host, low48).
                    // The engine writes deep into these (objA[+552/608],
                    // renderer[+224/232/236/238], view[+124..140]) — MUST be large
                    // enough that every fabricated struct (renderer/base, objA +0x100,
                    // objB +0x200, view +0x300, plus engine writes past those) stays
                    // inside the allocation, else the drive heap-corrupts at shutdown
                    // ("free(): invalid next size").
                    let objs = Box::leak(vec![0u8; 8192].into_boxed_slice());
                    let base = objs.as_ptr() as u64;
                    // view: +128=w(1280) +132=h(720) +140=default framebuffer(0)
                    unsafe {
                        *(base as *mut u64) = 0; // renderer[+0] reserved (obj not vt)
                        // renderer fields at +16,+24,+40
                        let renderer = base;
                        let objA = base + 0x100; // [renderer+24]->[+552]
                        let objB = base + 0x200; // [renderer+40]->[+140]
                        let view = base + 0x300;
                        // [renderer+16]=1 (set by fn anyway), [renderer+24]=objA
                        *(renderer.wrapping_add(16) as *mut u8) = 1;
                        *(renderer.wrapping_add(24) as *mut u64) = objA;
                        *(renderer.wrapping_add(40) as *mut u64) = objB;
                        // objA[+552]=1 (nonzero -> clear path enabled)
                        *(objA.wrapping_add(552) as *mut u8) = 1;
                        // 0x5b2e98c is a list-find: it reads [objA+368] first and
                        // returns immediately when [objA+368]==view(arg1), else walks
                        // an intrusive list [objA+384]..[objA+392] (empty => returns at
                        // the head==tail check). Set [objA+368]=view so it bails on the
                        // first cmp (clean return, no list walk that could fault on a
                        // 0 head). Also seed both list bounds to 0 = empty list.
                        *(objA.wrapping_add(368) as *mut u64) = view;
                        *(objA.wrapping_add(384) as *mut u64) = 0;
                        *(objA.wrapping_add(392) as *mut u64) = 0;
                        // objB[+140]=1, [+124]=1 (nonzero flags)
                        *(objB.wrapping_add(140) as *mut u32) = 0; // 0 -> glDrawBuffers(1,{GL_BACK}) for the default FB
                        *(objB.wrapping_add(124) as *mut u32) = 1;
                        // view: [+128]=w=[+132]=h, [+140]=framebuffer id 0
                        *(view.wrapping_add(128) as *mut u32) = 1280;
                        *(view.wrapping_add(132) as *mut u32) = 720;
                        *(view.wrapping_add(140) as *mut u32) = 0;
                        // Clear color: the engine's frame-fn takes the clear-color
                        // object as its 5th arg x4 (0x105b32c30 `mov x20,x4`), passed
                        // as the clear-state sub-fn 0x105b32e08's x2 (its prologue
                        // `mov x21,x2`), which reads the RGBA float4 from
                        // [obj+4],[obj+8],[obj+12],[obj+16] (LDP s0,s1,[x21,#4] /
                        // LDP s2,s3,[x21,#12]). The sub-fn call is gated on
                        // x4!=0 AND [x4]!=0 (0x105b32d44 cbz x20 / 0x105b32d4c cbz
                        // [x20]). SH20 left x4=0 so the engine's own clear never
                        // fired and the window stayed black. Fix: fabricate a
                        // clear-state object (nonzero [+0] flag + RGBA float4 at
                        // [+4..16]) and pass it as x4 (optionally recolored via
                        // --renderframe-color r,g,b,a).
                        let default_cc = [0.40f32, 0.20f32, 0.95f32, 1.0f32];
                        let mut cc = default_cc;
                        if let Some(i) = renderframe_args
                            .iter()
                            .position(|a| a == "--renderframe-color")
                        {
                            let csv = renderframe_args.get(i + 1).cloned().unwrap_or_default();
                            let vals: Vec<f32> = csv
                                .split(',')
                                .filter_map(|x| x.parse::<f32>().ok())
                                .collect();
                            if vals.len() >= 4 {
                                cc = [vals[0], vals[1], vals[2], vals[3]];
                            }
                        }
                        let clearobj = base + 0x400; // frame-fn x4 = clear-state obj
                        *(clearobj as *mut u32) = 0xF; // [obj+0]: clear-buffer bitmask (w20); 0xF=all 4
                        for (k, v) in cc.iter().enumerate() {
                            *(clearobj.wrapping_add(4 + (k as u64) * 4) as *mut f32) = *v;
                        }
                        // Frame-fn 6th arg x5 -> x22 (0x105b32c28 `mov x22,x5`), the
                        // main-fn's second clear-source object (0x105b32d5c cbz x22 /
                        // ldr q0,[x22] copies [+0..16] vec; gated on [x22]!=0). The
                        // harness left x5=0 so this path was skipped too.
                        let ccobj = base + 0x500;
                        *(ccobj.wrapping_add(0) as *mut f32) = cc[0];
                        *(ccobj.wrapping_add(4) as *mut f32) = cc[1];
                        *(ccobj.wrapping_add(8) as *mut f32) = cc[2];
                        *(ccobj.wrapping_add(12) as *mut f32) = cc[3];
                        eprintln!(
                            "[elfjit:renderframe-drive] clear-color x5 obj 0x{ccobj:x} (RGBA {cc:?} at [+0..16])"
                        );
                        // SH19 diagnostic: dump the 8 engine-GLES dispatch slots the
                        // frame's clear path `br`-stubs read. The stubs 0x5b3a1c0..
                        // (with a 0x10 stride) do `adrp x8, 6d3b000; ldr x2,[x8,#752]`
                        // + 8*N, i.e. slot N at guest 0x106d3b2f0 + 8*N. Each holds a
                        // function pointer the engine's own GLES-table init is
                        // supposed to populate (it never does under our headless
                        // drive), so an unset slot makes the clear path `br` into
                        // garbage. Since guest==host these vaddrs are dereferenceable.
                        let mut vals = [0u64; 8];
                        for i in 0..8 {
                            let slot_v = 0x106d3b2f0u64 + i * 8;
                            let val = unsafe { *(slot_v as *const u64) };
                            vals[i as usize] = val;
                            eprintln!(
                                "[elfjit:renderframe-drive] gles-dispatch slot {i} guest {slot_v:#x} = {val:#x}"
                            );
                        }
                        eprintln!(
                            "[elfjit:renderframe-drive] gles-dispatch values = {vals:?}"
                        );
                        // --renderframe-seedgles (opt-in): overwrite the 8 engine
                        // GLES dispatch slots (BSS 0x106d3b2f0..0x106d3b328) with
                        // OUR host-thunk GLES bridge slots (resolve_gles_mixed) so
                        // the frame clear path's `br`-stubs dispatch through the
                        // bridge (float/texture interception) instead of jumping to
                        // raw Mesa (out-of-image). The engine's real GL-init fills
                        // these with raw Mesa addresses (SH19); seeding proves the
                        // bridge takes over. Names are per-slot guesses from the
                        // clear-path usage; refine by reading which slot the engine
                        // needs once the drive passes the current stop.
                        // Slot->function names corrected by disassembly (SH22): the clear
                        // path dispatches slot0 as glDrawBuffers (builds
                        // {GL_COLOR_ATTACHMENT0..3} / {GL_BACK} buf arrays) and slot2 as
                        // glClearBufferfv (per-buffer clear loop uses GL_COLOR=0x1800 /
                        // GL_DEPTH=0x1801 buffer enums, drawbuffer in w1, value ptr in x2).
                        // The SH19-21 "glClearColor"+"glClearDepthf" guesses mis-routed
                        // those dispatches (glClearDepthf bridge ignored the int/ptr args
                        // and cleared nothing -> black window).
                        // The engine's GLES dispatch table is 16 slots at BSS
                        // 0x106d3b2f0 (stub 0x5b3a1c0+0xc*N does adrp 6d3b000; ldr
                        // xK,[x8,#752+8*N]; br xK). Slots 0-7 are the clear path
                        // (SH22-corrected names below). Slots 8-15 are the GEOMETRY
                        // draw path: the draw wrapper 0x5b35288 dispatches slot 9 as
                        // glDrawElements (indexed draw, 0x5b352f4 bl 0x5b3a22c) and
                        // slot 10 as glDrawArrays (array draw, 0x5b35368 bl 0x5b3a238)
                        // after the primitive-setup fn 0x5b353d0 binds buffers +
                        // sets up vertex attrib pointers (glBindBuffer/
                        // glEnableVertexAttribArray/glVertexAttribPointer direct @plt).
                        // Seeding slots 9/10 too means a real geometry draw (reaching
                        // the RENDERER C++ object reverse) dispatches through the
                        // bridge instead of jumping to a raw Mesa addr (SH19 class).
                        // Seed EVERY dispatch slot explicitly by (slot, name). Slots
                        // 0-7 are the clear path (SH22-corrected names below). The
                        // real geometry draw dispatches slot 9 as glDrawElements
                        // (indexed draw, wrapper 0x5b35288 @0x5b352f4 bl 0x5b3a22c)
                        // and slot 10 as glDrawArrays (array draw @0x5b35368 bl
                        // 0x5b3a238), after primitive-setup 0x5b353d0 binds buffers +
                        // sets vertex attrib pointers via direct @plt. Seeding 9/10
                        // means a real geometry draw dispatches through the bridge
                        // instead of jumping to a raw Mesa addr (SH19 class).
                        let seed_slots: [(usize, &str); 10] = [
                            (0, "glDrawBuffers"),
                            (1, "glClearBufferiv"),
                            (2, "glClearBufferfv"),
                            (3, "glClearBufferfi"),
                            (4, "glColorMask"),
                            (5, "glDepthMask"),
                            (6, "glStencilMask"),
                            (7, "glViewport"),
                            (9, "glDrawElements"),
                            (10, "glDrawArrays"),
                        ];
                        if renderframe_args.iter().any(|a| a == "--renderframe-seedgles") {
                            // Diagnostic: dump the 16 raw slot values the ENGINE left in the
                            // dispatch table (before we overwrite) and dladdr-resolve each host
                            // address to a symbol. Pins the real slot->function mapping
                            // (0-7 clear, 9/10 draw, 11-15 texture/uniform/shader) without code
                            // archaeology, IF the engine's GL-init has filled them.
                            eprintln!("[elfjit:renderframe-seedgles] raw slot snapshot (before seed):");
                            for (i, _n) in [(0usize, "x"), (1, "y"), (2, "z"), (3, "w"), (4, "q"), (5, "r"), (6, "s"), (7, "t"), (8, "a"), (9, "b"), (10, "c"), (11, "d"), (12, "e"), (13, "f"), (14, "g"), (15, "h")] {
                                let sv = 0x106d3b2f0 + (i as u64) * 8;
                                let raw = unsafe { *(sv as *const u64) };
                                let sym = unsafe {
                                    let mut dli = std::mem::zeroed::<libc::Dl_info>();
                                    if libc::dladdr(raw as *const libc::c_void, &mut dli) != 0
                                        && !dli.dli_sname.is_null()
                                    {
                                        std::ffi::CStr::from_ptr(dli.dli_sname)
                                            .to_string_lossy()
                                            .into_owned()
                                    } else {
                                        String::new()
                                    }
                                };
                                eprintln!("[elfjit:renderframe-seedgles]   slot {i:2} = {raw:#018x}  {sym}");
                            }
                            for (i, name) in seed_slots {
                                let slot_v = 0x106d3b2f0u64 + (i as u64) * 8;
                                // Mixed (float) ABI first; fall back to int ABI for
                                // glClear/glColorMask/glViewport etc.
                                let slot = arm64jit::resolver::resolve_gles_mixed(
                                    format!("{name}\0").as_bytes(),
                                )
                                .or_else(|| {
                                    arm64jit::resolver::resolve_gles_int(
                                        format!("{name}\0").as_bytes(),
                                    )
                                });
                                match slot {
                                    Some(bridge_slot) => {
                                        unsafe { *(slot_v as *mut u64) = bridge_slot };
                                        eprintln!(
                                            "[elfjit:renderframe-seedgles] slot {i} ({name}) <- bridge {bridge_slot:#x}"
                                        );
                                    }
                                    None => eprintln!(
                                        "[elfjit:renderframe-seedgles] slot {i} ({name}) NOT resolvable"
                                    ),
                                }
                            }
                        }
                        eprintln!(
                            "[elfjit:renderframe-drive] fabricated renderer 0x{renderer:x} (+16=1,+24->0x{objA:x}[+552]=1,+40->0x{objB:x}[+140]=1) view 0x{view:x} ([+128]=1280 [+132]=720 [+140]=0)"
                        );
                        // --renderframe-loop <N>: repeat the engine's OWN recipe
                        // (bind already done by renderbind -> the real frame-fn
                        // 0x105b32c00 -> post-frame swap via real ctx) N times to
                        // prove the render path is reentrant/sustainable. Default 1.
                        // --rendersustain <fps>: instead of a bounded loop, run the
                        // engine's OWN recipe CONTINUOUSLY at ~fps on this detached
                        // host thread while StartApp's main-loop jit_run idles
                        // concurrently on the main thread — a live animated render
                        // loop (the shape the engine needs to drive frames from its
                        // own thread). Each frame cycles the clear color through a
                        // small palette so a capture proves every frame is a fresh
                        // render, not a static buffer.
                        let loop_n: usize = renderframe_args
                            .iter()
                            .position(|a| a == "--renderframe-loop")
                            .and_then(|i| renderframe_args.get(i + 1))
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(1);
                        let sustain_fps: Option<f64> = renderframe_args
                            .iter()
                            .position(|a| a == "--rendersustain")
                            .and_then(|i| renderframe_args.get(i + 1))
                            .and_then(|v| v.parse().ok());
                        let palette: [[f32; 4]; 5] = [
                            [0.40, 0.20, 0.95, 1.0],
                            [0.10, 0.70, 0.05, 1.0],
                            [0.90, 0.15, 0.10, 1.0],
                            [0.05, 0.60, 0.90, 1.0],
                            [1.00, 0.82, 0.05, 1.0],
                        ];
                        let mut iter: u64 = 0;
                        loop {
                            // Per-frame color: sustain mode cycles the palette (so a
                            // capture proves fresh renders); bounded --renderframe-loop
                            // keeps the --renderframe-color (or default).
                            let cur_color = match sustain_fps {
                                Some(_) => palette[(iter as usize) % palette.len()],
                                None => cc,
                            };
                            // Re-write both clear-color sources each iteration.
                            unsafe {
                                for (k, v) in cur_color.iter().enumerate() {
                                    *(clearobj.wrapping_add(4 + (k as u64) * 4) as *mut f32) = *v;
                                    *(ccobj.wrapping_add((k as u64) * 4) as *mut f32) = *v;
                                }
                            }
                            eprintln!(
                                "[elfjit:renderframe-drive] === frame iteration {iter} color {:?} ===",
                                cur_color
                            );
                            let mut sd = arm64jit::jit::CpuState::new();
                            sd.tpidr = tpidr;
                            sd.x[31] = isp;
                            sd.x[0] = renderer;
                            sd.x[1] = view;
                            sd.x[2] = view; // 3rd arg (w2, unused by main fn path)
                            sd.x[4] = clearobj; // 5th arg -> x20 -> clear-state sub-fn x2
                            sd.x[5] = ccobj; // 6th arg -> x22 -> color-source object
                            match arm64jit::jit::jit_run(
                                iimg, ibase, 0x105b32c00, &mut sd as *mut CpuState,
                            ) {
                                Err(e) => eprintln!("[elfjit:renderframe-drive] frame-fn stopped: {e}"),
                                Ok(ok) => eprintln!(
                                    "[elfjit:renderframe-drive] engine frame-fn 0x105b32c00 returned Ok({ok:#x})"
                                ),
                            }
                            // Then present whatever the frame-fn did on the real ctx.
                            let mut se = arm64jit::jit::CpuState::new();
                            se.tpidr = tpidr;
                            se.x[31] = isp;
                            se.x[0] = real_ctx;
                            match arm64jit::jit::jit_run(iimg, ibase, swap_thunk, &mut se as *mut CpuState) {
                                Err(e) => eprintln!("[elfjit:renderframe-drive] swap stopped: {e}"),
                                Ok(ok) => eprintln!(
                                    "[elfjit:renderframe-drive] post-frame swap returned Ok({ok:#x})"
                                ),
                            }
                            iter += 1;
                            if let Some(fps) = sustain_fps {
                                if fps > 0.0 {
                                    std::thread::sleep(std::time::Duration::from_secs_f64(1.0 / fps));
                                }
                            } else if (iter as usize) >= loop_n {
                                break;
                            }
                        }
                        // --renderframe-drawprobe: drive the engine's REAL geometry
                        // draw wrapper 0x5b35288 (the fn that calls primitive-setup
                        // 0x5b353d0 then dispatches the indexed/array draw through
                        // GLES dispatch-table slots 9/10). Fabricate a minimal
                        // coherent renderer: empty primitive list ([container+72]==
                        // [container+80]==0 -> 0x5b353d0 returns mask 0 fast), but a
                        // NONZERO [renderer+120] index-buffer object + nonzero count
                        // arg (w5) so the wrapper takes the INDEXED path and
                        // dispatches slot 9 (glDrawElements) through the bridge.
                        // This proves the geometry draw dispatch reaches a real
                        // glDrawElements (currently the recorded clear-state maxes
                        // out before any gl*Draw*).
                        if renderframe_args.iter().any(|a| a == "--renderframe-drawprobe") {
                            let objs = Box::leak(vec![0u8; 8192].into_boxed_slice());
                            let base = objs.as_ptr() as u64;
                            let renderer = base;
                            let container = base + 0x100;
                            let ibo = base + 0x200;
                            unsafe {
                                // renderer[56] = container (0x5b353f0 ldr x25,[x0,#56])
                                *(renderer.wrapping_add(56) as *mut u64) = container;
                                // renderer[120] = index-buffer object (nonzero -> indexed path)
                                *(renderer.wrapping_add(120) as *mut u64) = ibo;
                                // renderer[142] u16 element count (w8 in wrapper 0x5b352b8)
                                *(renderer.wrapping_add(142) as *mut u16) = 3;
                                // container[72]/[80] = begin/end primitive list, empty (equal)
                                *(container.wrapping_add(72) as *mut u64) = 0;
                                *(container.wrapping_add(80) as *mut u64) = 0;
                                // ibo[72] = element-buffer id (bound by 0x5b353d0's tail)
                                *(ibo.wrapping_add(72) as *mut u32) = 0;
                            }
                            eprintln!(
                                "[elfjit:renderframe-drawprobe] fabricated renderer 0x{renderer:x} ([+56]->cont, [+120]=ibo, [+142]=3, empty prim list)"
                            );
                            let mut sd = arm64jit::jit::CpuState::new();
                            sd.tpidr = tpidr;
                            sd.x[31] = isp;
                            sd.x[0] = renderer;
                            sd.x[1] = 0; // w22: draw-mode table index (GL_TRIANGLES-ish)
                            sd.x[2] = 0; // w23: stride multiplier
                            sd.x[3] = 0; // -> w1 for primitive-setup
                            sd.x[4] = 0; // w20: offset/count arg
                            sd.x[5] = 3; // w21: count (nonzero -> indexed path w/ slot 9)
                            match arm64jit::jit::jit_run(
                                iimg, ibase, 0x105b35288, &mut sd as *mut CpuState,
                            ) {
                                Err(e) => eprintln!(
                                    "[elfjit:renderframe-drawprobe] geometry wrapper stopped: {e}"
                                ),
                                Ok(ok) => eprintln!(
                                    "[elfjit:renderframe-drawprobe] geometry wrapper 0x5b35288 returned Ok({ok:#x})"
                                ),
                            }
                            // --renderframe-triangle: fabricate a COHERENT renderer — a
                            // real 1-primitive list, a real vertex-descriptor table, a
                            // real stride table, a real IBO, real vertex/index buffers
                            // (created + uploaded through the JIT bridge) and a real
                            // compiled+linked shader program — then drive the engine's
                            // own geometry wrapper 0x5b35288. Its primitive-setup
                            // 0x5b353d0 runs its REAL loop (bind ARRAY_BUFFER, enable
                            // attrib 0, glVertexAttribPointer at the format table) and
                            // the wrapper then dispatches a REAL indexed glDrawElements
                            // through GLES dispatch-table slot 9 with count=3, drawing
                            // an actual visible triangle (proving real geometry, not
                            // just the clear path, renders through the bridge).
                            if renderframe_args.iter().any(|a| a == "--renderframe-triangle") {
                                // GL enums used below.
                                const GL_ARRAY_BUFFER: u64 = 0x8892;
                                const GL_ELEMENT_ARRAY_BUFFER: u64 = 0x8893;
                                const GL_STATIC_DRAW: u64 = 0x88e4;
                                const GL_FLOAT: u64 = 0x1406;
                                const GL_VERTEX_SHADER: u64 = 0x8b31;
                                const GL_FRAGMENT_SHADER: u64 = 0x8b30;
                                const GL_COMPILE_STATUS: u64 = 0x8b81;
                                const GL_COLOR_BUFFER_BIT: u64 = 0x4000;
                                // GLES PLT stubs (verified against the real binary).
                                let plt_clear = 0x1062d7740u64;
                                let plt_clearcolor = 0x1062d7710u64;
                                let plt_viewport = 0x1062d75c0u64;
                                let plt_scissor = 0x1062d75d0u64;
                                let plt_genbuffers = 0x1062d77c0u64;
                                let plt_bindbuffer = 0x1062d77b0u64;
                                let plt_buffdata = 0x1062d77d0u64;
                                let plt_createshader = 0x1062d7880u64;
                                let plt_shadersource = 0x1062d7890u64;
                                let plt_compileshader = 0x1062d78a0u64;
                                let plt_getshaderiv = 0x1062d78b0u64;
                                // Texture/uniform GLES PLT stubs (verified against the real binary
                                // .plt: guest = file vaddr + 0x100000000).
                                let plt_active_texture = 0x1062d75e0u64;
                                let plt_bind_texture = 0x1062d75f0u64;
                                let plt_get_uniform_location = 0x1062d7900u64;
                                let plt_uniform_1i = 0x1062d7910u64;
                                let plt_tex_parameteri = 0x1062d7960u64;
                                let plt_gen_textures = 0x1062d7980u64;
                                let plt_tex_image_2d = 0x1062d79a0u64;
                                let plt_compressed_tex_image_2d = 0x1062d7990u64;
                                    let plt_getprogramiv = 0x1062d77f0u64;
                                    let plt_readpixels = 0x1062d7940u64;
                                    let plt_attachshader = 0x1062d78d0u64;
                                let plt_linkprogram = 0x1062d78e0u64;
                                let plt_bindattrib = 0x1062d78f0u64;
                                let plt_useprogram = 0x1062d75a0u64;
                                let plt_enableattrib = 0x1062d7850u64;
                                let plt_attribptr = 0x1062d7860u64;
                                let plt_dewelem = 0x1062d7830u64; // glDrawElements@plt (direct, not slot9)
                                // Helper: drive a single guest PLT stub via jit_run and
                                // return its x0 (the int-bridge HostCall returns via x0).
                                let mut gcall = |addr: u64,
                                                 a0: u64,
                                                 a1: u64,
                                                 a2: u64,
                                                 a3: u64,
                                                 a4: u64,
                                                 a5: u64|
                                                 -> Result<u64, String> {
                                    let mut s = arm64jit::jit::CpuState::new();
                                    s.tpidr = tpidr;
                                    s.x[31] = isp;
                                    s.x[0] = a0;
                                    s.x[1] = a1;
                                    s.x[2] = a2;
                                    s.x[3] = a3;
                                    s.x[4] = a4;
                                    s.x[5] = a5;
                                    let r = arm64jit::jit::jit_run(iimg, ibase, addr, &mut s as *mut CpuState)?;
                                    Ok(s.x[0])
                                };
                                let objs = Box::leak(vec![0u8; 16384].into_boxed_slice());
                                let base = objs.as_ptr() as u64;
                                unsafe {
                                    // Clear the framebuffer first so the triangle is
                                    // visible against a known background. glClearColor is
                                    // a FLOAT-ABI bridge (reads guest s0..s3 = v[0],v[2],
                                    // v[4],v[6] low lanes), so set the SIMD lanes not x-regs.
                                    let mut sc = arm64jit::jit::CpuState::new();
                                    sc.tpidr = tpidr;
                                    sc.x[31] = isp;
                                    sc.v[0] = (0.0f32).to_bits() as u64;
                                    sc.v[2] = (0.0f32).to_bits() as u64;
                                    sc.v[4] = (0.3f32).to_bits() as u64;
                                    sc.v[6] = (1.0f32).to_bits() as u64;
                                    let _ = arm64jit::jit::jit_run(
                                        iimg,
                                        ibase,
                                        plt_clearcolor,
                                        &mut sc as *mut CpuState,
                                    );
                                    let _ = gcall(plt_clear, GL_COLOR_BUFFER_BIT, 0, 0, 0, 0, 0);
                                    // Set the viewport + scissor to the window size so
                                    // the rasterizer has a drawable region. The SH22
                                    // frame-fn sets these; a 0-size stale viewport from
                                    // context creation silently rasterizes nothing.
                                    let _ = gcall(plt_viewport, 0, 0, 1280, 720, 0, 0);
                                    let _ = gcall(plt_scissor, 0, 0, 1280, 720, 0, 0);
                                    // Vertex shader: pass clip-space position straight
                                    // through (data is already in NDC).
                                    let vs_src = b"attribute vec4 aPos;\nvoid main(){ gl_Position = aPos; }\n\0";
                                    // --renderframe-tex: prove the GLES texture/uniform/shader
                                    // bridge path renders a TEXTURED draw through the engine's
                                    // own geometry wrapper. The fragment shader samples a 2x2 RGBA
                                    // checkerboard via a UV computed from gl_FragCoord (so the
                                    // single-attrib coherent renderer stays unchanged — no second
                                    // UV vertex attrib). floor/texture2D/gl_FragCoord are all GLSL
                                    // ES 1.00. Three interior probes then read back three DIFFERENT
                                    // texel colors, which no constant/solid shader can produce.
                                    let tex_mode = renderframe_args.iter().any(|a| a == "--renderframe-tex");
                                    // --renderframe-etc: like --renderframe-tex but uploads the 2x2
                                    // checkerboard as a REAL compressed ETC1 texture (4 solid
                                    // 4x4 blocks = 8x8) through glCompressedTexImage2D
                                    // (GL_ETC1_RGB8_OES=0x8d64). Proves the compressed-texture
                                    // interception live: the bridge decodes ETC1->RGBA and
                                    // uploads via glTexImage2D. Same FS + readback as tex_mode.
                                    let etc_mode = renderframe_args.iter().any(|a| a == "--renderframe-etc");
                                    // --renderframe-etc2: same compressed path but internalformat
                                    // GL_COMPRESSED_RGB8_ETC2 (0x9274 — the actual Android Roblox
                                    // ETC2 format). Modes 1/2 of ETC2 RGB are bit-identical to ETC1
                                    // individual/differential, so the same crafted blocks are valid
                                    // ETC2 blocks; decode_etc2_rgb must yield the same colors.
                                    let etc2_mode = renderframe_args.iter().any(|a| a == "--renderframe-etc2");
                                    let comp_mode = etc_mode || etc2_mode;
                                    let fs_src: &[u8] = if tex_mode || comp_mode {
                                        // 2x2 texels RED,GREEN,BLUE,WHITE. UV = floor(frag/640,360)
                                        // picks a quadrant, (uv+0.5)*0.5 samples its texel center
                                        // under NEAREST. centroid(640,360)->(1,1)->WHITE; (900,150)
                                        // ->(1,0)->GREEN; (300,150)->(0,0)->RED.
                                        b"precision mediump float;\nuniform sampler2D uTex;\nvoid main(){ vec2 uv = floor(gl_FragCoord.xy / vec2(640.0,360.0)); uv = (uv + 0.5) * 0.5; gl_FragColor = texture2D(uTex, uv); }\n\0"
                                    } else {
                                        // Fragment shader: solid red.
                                        b"void main(){ gl_FragColor = vec4(1.0,0.0,0.0,1.0); }\n\0"
                                    };
                                    let vs_ptr = objs.as_ptr() as u64 + 0x400;
                                    let fs_ptr = objs.as_ptr() as u64 + 0x800;
                                    std::ptr::copy_nonoverlapping(
                                        vs_src.as_ptr(),
                                        vs_ptr as *mut u8,
                                        vs_src.len(),
                                    );
                                    std::ptr::copy_nonoverlapping(
                                        fs_src.as_ptr(),
                                        fs_ptr as *mut u8,
                                        fs_src.len(),
                                    );
                                    // src[] arrays: 1 string pointer each, NULL lengths.
                                    let vs_ary = objs.as_ptr() as u64 + 0xa00;
                                    let fs_ary = objs.as_ptr() as u64 + 0xa10;
                                    *(vs_ary as *mut u64) = vs_ptr;
                                    *(fs_ary as *mut u64) = fs_ptr;
                                    // Triangle vertices (NDC, 3 x vec4). Fill most of the frame so the
                                    // rendered footprint is easy to measure for scaling.
                                    let verts: [f32; 12] = [
                                        -0.95, -0.95, 0.0, 1.0, // v0
                                        0.95, -0.95, 0.0, 1.0, // v1
                                        0.0, 0.95, 0.0, 1.0, // v2
                                    ];
                                    let vbo_data = objs.as_ptr() as u64 + 0xc00;
                                    // Indices: 3 (u32).
                                    let idx: [u32; 3] = [0, 1, 2];
                                    let ebo_data = objs.as_ptr() as u64 + 0xd00;
                                    std::ptr::copy_nonoverlapping(
                                        verts.as_ptr() as *const u8,
                                        vbo_data as *mut u8,
                                        std::mem::size_of_val(&verts),
                                    );
                                    std::ptr::copy_nonoverlapping(
                                        idx.as_ptr() as *const u8,
                                        ebo_data as *mut u8,
                                        std::mem::size_of_val(&idx),
                                    );
                                    let shader_id_slot = objs.as_ptr() as u64 + 0xe00;
                                    let _ = shader_id_slot;
                                    // Compile vertex shader (glCreateShader returns id in x0).
                                    let vs_shader = gcall(plt_createshader, GL_VERTEX_SHADER, 0, 0, 0, 0, 0)
                                        .unwrap_or(0)
                                        & 0xffff_ffff;
                                    let _ = gcall(plt_shadersource, vs_shader, 1, vs_ary, 0, 0, 0);
                                    let _ = gcall(plt_compileshader, vs_shader, 0, 0, 0, 0, 0);
                                    // Compile fragment shader.
                                    let fs_shader = gcall(plt_createshader, GL_FRAGMENT_SHADER, 0, 0, 0, 0, 0)
                                        .unwrap_or(0)
                                        & 0xffff_ffff;
                                    let _ = gcall(plt_shadersource, fs_shader, 1, fs_ary, 0, 0, 0);
                                    let _ = gcall(plt_compileshader, fs_shader, 0, 0, 0, 0, 0);
                                    eprintln!(
                                        "[elfjit:renderframe-triangle] compiled vs={vs_shader:#x} fs={fs_shader:#x}"
                                    );
                                    // Create + link program (id returned in x0).
                                    let program = gcall(0x1062d78c0, 0, 0, 0, 0, 0, 0) // glCreateProgram@plt
                                        .unwrap_or(0)
                                        & 0xffff_ffff;
                                    let _ = gcall(plt_attachshader, program, vs_shader, 0, 0, 0, 0);
                                    let _ = gcall(plt_attachshader, program, fs_shader, 0, 0, 0, 0);
                                    // Bind attrib location 0 = aPos BEFORE link.
                                    let loc_name = objs.as_ptr() as u64 + 0xd20;
                                    std::ptr::copy_nonoverlapping(
                                        b"aPos\0".as_ptr(),
                                        loc_name as *mut u8,
                                        5,
                                    );
                                    let _ = gcall(plt_bindattrib, program, 0, loc_name, 0, 0, 0);
                                    let _ = gcall(plt_linkprogram, program, 0, 0, 0, 0, 0);
                                    let _ = gcall(plt_useprogram, program, 0, 0, 0, 0, 0);
                                    eprintln!(
                                        "[elfjit:renderframe-triangle] linked program={program:#x} current"
                                    );
                                    // --renderframe-tex: create + upload a 2x2 RGBA checkerboard
                                    // texture and assign it to the program's uTex sampler (unit 0),
                                    // all through the GLES bridge (@plt). Every call here exercises
                                    // the texture/uniform/shader bridge surface the engine's real
                                    // textured draws will need. glTexImage2D has 9 args (pixels on
                                    // the guest stack), so drive it with a dedicated CpuState whose
                                    // sp=tex_sp points at a slot holding the pixels pointer.
                                    if tex_mode || comp_mode {
                                                                            const GL_TEXTURE0: u64 = 0x84c0;
                                                                            const GL_TEXTURE_2D: u64 = 0x0de1;
                                                                            const GL_RGBA: u64 = 0x1908;
                                                                            const GL_UNSIGNED_BYTE: u64 = 0x1401;
                                                                            const GL_NEAREST: u64 = 0x2600;
                                                                            const GL_TEXTURE_MIN_FILTER: u64 = 0x2801;
                                                                            const GL_TEXTURE_MAG_FILTER: u64 = 0x2800;
                                                                            const GL_TEX_DATA: u64 = 0xf60;
                                                                            // Shared: create + bind the texture on unit 0, NEAREST filtering.
                                                                            let tex_id_slot = base + 0xfd0;
                                                                            let _ = gcall(plt_gen_textures, 1, tex_id_slot, 0, 0, 0, 0);
                                                                            let tex_id = *(tex_id_slot as *const u32) as u64;
                                                                            let _ = gcall(plt_active_texture, GL_TEXTURE0, 0, 0, 0, 0, 0);
                                                                            let _ = gcall(plt_bind_texture, GL_TEXTURE_2D, tex_id, 0, 0, 0, 0);
                                                                            let _ = gcall(plt_tex_parameteri, GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST, 0, 0, 0);
                                                                            let _ = gcall(plt_tex_parameteri, GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST, 0, 0, 0);
                                                                            if tex_mode {
                                                                                // RGBA 2x2 checkerboard via 9-arg glTexImage2D. pixels (the 9th arg) rides the
                                                                                // guest stack at [sp+0]; the PLT stub is a leaf (adrp/ldr/add/br, never pushes sp),
                                                                                // so a fake sp whose [0] holds the pixels ptr is read by the bridge's gs_stack.
                                                                                const TEX: [u8; 16] = [
                                                                                    255, 0, 0, 255, // texel(0,0) RED
                                                                                    0, 255, 0, 255, // texel(1,0) GREEN
                                                                                    0, 0, 255, 255, // texel(0,1) BLUE
                                                                                    255, 255, 255, 255, // texel(1,1) WHITE
                                                                                ];
                                                                                std::ptr::copy_nonoverlapping(TEX.as_ptr(), (base + GL_TEX_DATA) as *mut u8, 16);
                                                                                let tex_sp = base + 0xf80;
                                                                                *(tex_sp as *mut u64) = base + GL_TEX_DATA;
                                                                                let mut stex = arm64jit::jit::CpuState::new();
                                                                                stex.tpidr = tpidr;
                                                                                stex.x[31] = tex_sp;
                                                                                stex.x[0] = GL_TEXTURE_2D;
                                                                                stex.x[1] = 0; // level
                                                                                stex.x[2] = GL_RGBA; // internalformat
                                                                                stex.x[3] = 2; // width
                                                                                stex.x[4] = 2; // height
                                                                                stex.x[5] = 0; // border
                                                                                stex.x[6] = GL_RGBA; // format
                                                                                stex.x[7] = GL_UNSIGNED_BYTE; // type
                                                                                let _ = arm64jit::jit::jit_run(iimg, ibase, plt_tex_image_2d, &mut stex as *mut CpuState);
                                                                            } else {
                                                                                // ETC1 compressed-texture interception live-path: upload a REAL 8x8 ETC1 texture
                                                                                // (4 solid 4x4 blocks = 32 bytes) via glCompressedTexImage2D (GL_ETC1_RGB8_OES).
                                                                                // The bridge decodes ETC1->RGBA (texture-codec) and re-uploads via glTexImage2D.
                                                                                // All 8 args fit x0-x7 (no stack arg). Each block: individual mode, table codeword
                                                                                // 0, all selectors 0 -> decoded color = (c*0x11)+2 per channel, clamped.
                                                                                const GL_ETC1_RGB8_OES: u64 = 0x8d64;
                                                                                const GL_COMPRESSED_RGB8_ETC2: u64 = 0x9274;
                                                                                // ETC2 mode 1/2 are bit-identical to ETC1 individual/differential, so the
                                                                                // same blocks are valid ETC2-RGB; just relabel the internalformat to prove
                                                                                // decode_etc2_rgb (the real Android Roblox path) handles them.
                                                                                let comp_fmt = if etc2_mode { GL_COMPRESSED_RGB8_ETC2 } else { GL_ETC1_RGB8_OES };
                                                                                let enc = |t: i32| -> u8 { let c = ((t - 2).clamp(0, 240) >> 4) as u8; (c << 4) | c };
                                                                                let blk = |r: u8, g: u8, b: u8| -> [u8; 8] { [r, g, b, 0, 0, 0, 0, 0] };
                                                                                // 8x8 ETC1: 4 blocks row-major top-first -> (255,2,2) red,(2,255,2) green,
                                                                                // (2,2,255) blue,(255,255,255) white.
                                                                                let etc_data: [u8; 32] = {
                                                                                    let mut d = [0u8; 32];
                                                                                    let red = blk(enc(255), enc(2), enc(2));
                                                                                    let grn = blk(enc(2), enc(255), enc(2));
                                                                                    let blu = blk(enc(2), enc(2), enc(255));
                                                                                    let wht = blk(enc(255), enc(255), enc(255));
                                                                                    d[0..8].copy_from_slice(&red);
                                                                                    d[8..16].copy_from_slice(&grn);
                                                                                    d[16..24].copy_from_slice(&blu);
                                                                                    d[24..32].copy_from_slice(&wht);
                                                                                    d
                                                                                };
                                                                                std::ptr::copy_nonoverlapping(etc_data.as_ptr(), (base + GL_TEX_DATA) as *mut u8, 32);
                                                                                let mut sce = arm64jit::jit::CpuState::new();
                                                                                sce.tpidr = tpidr;
                                                                                sce.x[31] = isp;
                                                                                sce.x[0] = GL_TEXTURE_2D;
                                                                                sce.x[1] = 0; // level
                                                                                sce.x[2] = comp_fmt; // internalformat
                                                                                sce.x[3] = 8; // width
                                                                                sce.x[4] = 8; // height
                                                                                sce.x[5] = 0; // border
                                                                                sce.x[6] = 32; // imageSize
                                                                                sce.x[7] = base + GL_TEX_DATA; // data
                                                                                let _ = arm64jit::jit::jit_run(iimg, ibase, plt_compressed_tex_image_2d, &mut sce as *mut CpuState);
                                                                            }
                                                                            // Shared: uTex sampler = texture unit 0.
                                                                            let uni = objs.as_ptr() as u64 + 0xe20;
                                                                            std::ptr::copy_nonoverlapping(b"uTex\0".as_ptr(), uni as *mut u8, 5);
                                                                            let ploc = gcall(plt_get_uniform_location, program, uni, 0, 0, 0, 0).unwrap_or(0) & 0xffff_ffff;
                                                                            let _ = gcall(plt_uniform_1i, ploc, 0, 0, 0, 0, 0);
                                                                            eprintln!("[elfjit:renderframe-tex] texture tex_id={tex_id:#x} bound+uploaded uTex loc={ploc:#x}<-unit0");
                                    }
                                    // Diagnostics: real compile/link status. Reading a
                                    // GL int from a shifted-out 32-bit slot requires a
                                    // predictable result location — use glGetShaderiv/
                                    // glGetProgramiv writing a real int result slot.
                                    let int_slot0 = objs.as_ptr() as u64 + 0xf20; // vs compile
                                    let int_slot1 = objs.as_ptr() as u64 + 0xf24; // fs compile
                                    let int_slot2 = objs.as_ptr() as u64 + 0xf28; // link
                                    *(int_slot0 as *mut u32) = 0xdeadbeef;
                                    *(int_slot1 as *mut u32) = 0xdeadbeef;
                                    *(int_slot2 as *mut u32) = 0xdeadbeef;
                                    let _ = gcall(
                                        plt_getshaderiv,
                                        vs_shader,
                                        0x8b81, // GL_COMPILE_STATUS
                                        int_slot0,
                                        0,
                                        0,
                                        0,
                                    );
                                    let _ = gcall(
                                        plt_getshaderiv,
                                        fs_shader,
                                        0x8b81,
                                        int_slot1,
                                        0,
                                        0,
                                        0,
                                    );
                                    let _ = gcall(
                                        plt_getprogramiv,
                                        program,
                                        0x8b82, // GL_LINK_STATUS
                                        int_slot2,
                                        0,
                                        0,
                                        0,
                                    );
                                    unsafe {
                                        eprintln!(
                                            "[elfjit:renderframe-triangle] compile_status vs=0x{:x} fs=0x{:x} link_status=0x{:x}",
                                            *(int_slot0 as *const u32),
                                            *(int_slot1 as *const u32),
                                            *(int_slot2 as *const u32)
                                        );
                                    }
                                    // Debug: if a shader/program failed, dump its info log (via the
                                    // int bridge — glGetShaderInfoLog / glGetProgramInfoLog resolve
                                    // through the same resolve_gles_int the seedgles uses).
                                    {
                                        let logbuf = objs.as_ptr() as u64 + 0xfb0;
                                        let logslot: Option<u64> = arm64jit::resolver::resolve_gles_int(b"glGetShaderInfoLog\0");
                                        let plogslot: Option<u64> = arm64jit::resolver::resolve_gles_int(b"glGetProgramInfoLog\0");
                                        unsafe {
                                            let bad_fs = *(int_slot1 as *const u32) == 0;
                                            let bad_vs = *(int_slot0 as *const u32) == 0;
                                            let bad_link = *(int_slot2 as *const u32) == 0;
                                            if (bad_fs || bad_vs) && let Some(slot) = logslot {
                                                for (what, sh) in [("fs", fs_shader), ("vs", vs_shader)] {
                                                    if !(if what == "fs" { bad_fs } else { bad_vs }) { continue; }
                                                    let mut ls = arm64jit::jit::CpuState::new();
                                                    ls.tpidr = tpidr;
                                                    ls.x[31] = isp;
                                                    ls.x[0] = sh as u64;
                                                    ls.x[1] = 2048;
                                                    ls.x[2] = 0;
                                                    ls.x[3] = logbuf;
                                                    let _ = arm64jit::jit::jit_run(iimg, ibase, slot, &mut ls as *mut CpuState);
                                                    let cstr = std::ffi::CStr::from_ptr(logbuf as *const libc::c_char);
                                                    eprintln!("[elfjit:renderframe-triangle] {what} info-log: {cstr:?}");
                                                }
                                            }
                                            if bad_link && let Some(slot) = plogslot {
                                                let mut ls = arm64jit::jit::CpuState::new();
                                                ls.tpidr = tpidr;
                                                ls.x[31] = isp;
                                                ls.x[0] = program as u64;
                                                ls.x[1] = 2048;
                                                ls.x[2] = 0;
                                                ls.x[3] = logbuf;
                                                let _ = arm64jit::jit::jit_run(iimg, ibase, slot, &mut ls as *mut CpuState);
                                                let cstr = std::ffi::CStr::from_ptr(logbuf as *const libc::c_char);
                                                eprintln!("[elfjit:renderframe-triangle] program info-log: {cstr:?}");
                                            }
                                        }
                                    }
                                    // Create + fill the VBO (ARRAY_BUFFER) with verts.
                                    // glGenBuffers writes the generated id to its out
                                    // pointer — use a DEDICATED slot, never the data
                                    // buffer (aliasing would clobber the vertices).
                                    let vbo_id_slot = objs.as_ptr() as u64 + 0xf00;
                                    let ebo_id_slot = objs.as_ptr() as u64 + 0xf10;
                                    let _ = gcall(plt_genbuffers, 1, vbo_id_slot, 0, 0, 0, 0);
                                    let vbo = *(vbo_id_slot as *const u32) as u64;
                                    let _ = gcall(plt_bindbuffer, GL_ARRAY_BUFFER, vbo, 0, 0, 0, 0);
                                    let _ = gcall(
                                        plt_buffdata,
                                        GL_ARRAY_BUFFER,
                                        std::mem::size_of_val(&verts) as u64,
                                        vbo_data,
                                        GL_STATIC_DRAW,
                                        0,
                                        0,
                                    );
                                    // Create + fill the EBO (ELEMENT_ARRAY_BUFFER) idx.
                                    let _ = gcall(plt_genbuffers, 1, ebo_id_slot, 0, 0, 0, 0);
                                    let ebo = *(ebo_id_slot as *const u32) as u64;
                                    let _ = gcall(
                                        plt_bindbuffer,
                                        GL_ELEMENT_ARRAY_BUFFER,
                                        ebo,
                                        0,
                                        0,
                                        0,
                                        0,
                                    );
                                    let _ = gcall(
                                        plt_buffdata,
                                        GL_ELEMENT_ARRAY_BUFFER,
                                        std::mem::size_of_val(&idx) as u64,
                                        ebo_data,
                                        GL_STATIC_DRAW,
                                        0,
                                        0,
                                    );
                                    eprintln!(
                                        "[elfjit:renderframe-triangle] vbo={vbo:#x} ebo={ebo:#x} uploaded"
                                    );
                                    // REFERENCE DRAW (opt-in: SH25_REF=1): drive the
                                    // draw directly (not through the engine wrapper) with
                                    // our own glVertexAttribPointer, to cross-check the
                                    // engine-path result. Now that the engine wrapper's
                                    // primitive-setup renders the full triangle (format
                                    // index fixed 5->3 = GL_FLOAT), the reference is
                                    // redundant; default OFF (SH25_REF=1 re-enables).
                                    if std::env::var("SH25_REF").map(|v| v == "1").unwrap_or(false) {
                                    // With a VBO bound, the attrib pointer's 6th arg is a
                                    // byte OFFSET (0 = start of the buffer), not a host
                                    // pointer — a wrong value silently collapses geometry.
                                    {
                                        let _ = gcall(plt_bindbuffer, GL_ARRAY_BUFFER, vbo, 0, 0, 0, 0);
                                        let _ = gcall(plt_attribptr, 0, 4, GL_FLOAT, 0, 16, 0);
                                        let _ = gcall(plt_enableattrib, 0, 0, 0, 0, 0, 0);
                                        let _ = gcall(
                                            plt_bindbuffer,
                                            GL_ELEMENT_ARRAY_BUFFER,
                                            ebo,
                                            0,
                                            0,
                                            0,
                                            0,
                                        );
                                        eprintln!(
                                            "[elfjit:renderframe-triangle] reference draw: attrib0(4xfloat,stride16,off0) + EBO bound"
                                        );
                                        // glDrawElements signature: (mode, count, type,
                                        // indices-offset) -> (x0,x1,x2,x3).
                                        let mut sd = arm64jit::jit::CpuState::new();
                                        sd.tpidr = tpidr;
                                        sd.x[31] = isp;
                                        sd.x[0] = 4; // GL_TRIANGLES
                                        sd.x[1] = 3; // count
                                        sd.x[2] = 0x1405; // GL_UNSIGNED_INT
                                        sd.x[3] = 0; // indices offset in EBO
                                        let _ = arm64jit::jit::jit_run(
                                            iimg,
                                            ibase,
                                            plt_dewelem,
                                            &mut sd as *mut CpuState,
                                        );
                                        eprintln!(
                                            "[elfjit:renderframe-triangle] reference glDrawElements issued"
                                        );
                                    }
                                    }
                                    // ---- Fabricate the COHERENT renderer ----
                                    // renderer[+56]=container ; [renderer+0x48]=the 16-byte
                                    // vertex-descriptor table base (entry[vb] @ +vb*16).
                                    // container[+72]=begin,[+80]=end primitive list;
                                    // container[+96]=stride table base ([cb+96+vb*8]).
                                    // descriptor obj: [desc+72]=ARRAY_BUFFER id.
                                    // IBO: renderer[+120]=ibo obj; [ibo+72]=EBO id.
                                    // renderer[+142](u16)=element count.
                                    let renderer = base;
                                    let container = base + 0x100;
                                    let desc = base + 0x200; // vertex descriptor obj
                                    let stride_tbl = base + 0x300; // u64 tbl [vb]
                                    let prim = base + 0x400;
                                    let ibo = base + 0x500;
                                    let fmt_index: u32 = 3; // format[3]={size4, GL_FLOAT=0x1406} (table @0x100cecf8c). NOT format[5] which is {4, GL_SHORT=0x1402} — GL_SHORT misreads float verts -> degenerate.
                                    // descriptor[+72] = vbo id (the ARRAY_BUFFER we created)
                                    *(desc.wrapping_add(72) as *mut u32) = vbo as u32;
                                    // stride table[vb=0] = 16 (tight vec4)
                                    *(stride_tbl as *mut u64) = 16;
                                    // container
                                    *(renderer.wrapping_add(56) as *mut u64) = container;
                                    *(container.wrapping_add(72) as *mut u64) = prim;
                                    *(container.wrapping_add(80) as *mut u64) = prim + 0x18; // 1 prim (stride 0x18)
                                    *(container.wrapping_add(96) as *mut u64) = stride_tbl;
                                    // descriptor table is INLINE at renderer+0x48: entry[vb] @ +vb*16 is the
                                    // descriptor pointer (5b3546c ldr x11,[sp,#16] with
                                    // sp+16=renderer+0x48; 5b3547c ldr x10,[x11, w9<<4]).
                                    // vb=0 -> the desc ptr lives at renderer+0x48.
                                    *(renderer.wrapping_add(0x48) as *mut u64) = desc;
                                    // primitive: [+0]=vb idx(w9=0), [+4]=offset(w21=0),
                                    // [+8]=format idx(w28=fmt_index), [+12]=type(w22=0 ->
                                    // attrib index 0), [+16]=base(0).
                                    *(prim as *mut u32) = 0;
                                    *(prim.wrapping_add(4) as *mut u32) = 0;
                                    *(prim.wrapping_add(8) as *mut u32) = fmt_index;
                                    *(prim.wrapping_add(12) as *mut u32) = 0;
                                    *(prim.wrapping_add(16) as *mut u32) = 0;
                                    // IBO: renderer[+120]=ibo ; [ibo+72]=EBO id
                                    *(renderer.wrapping_add(120) as *mut u64) = ibo;
                                    *(ibo.wrapping_add(72) as *mut u32) = ebo as u32;
                                    // renderer[+142] u16 element count = 3
                                    *(renderer.wrapping_add(142) as *mut u16) = 3;
                                    eprintln!(
                                        "[elfjit:renderframe-triangle] coherent renderer 0x{renderer:x}: container 0x{container:x} prim 0x{prim:x} desc 0x{desc:x} desc_tbl@renderer+0x48 stride 0x{stride_tbl:x} ibo 0x{ibo:x}"
                                    );
                                    // Drive the engine's OWN geometry wrapper.
                                    let mut sw = arm64jit::jit::CpuState::new();
                                    sw.tpidr = tpidr;
                                    sw.x[31] = isp;
                                    sw.x[0] = renderer;
                                    sw.x[1] = 0; // w22: draw-mode table index (0=GL_TRIANGLES)
                                    sw.x[2] = 0; // w23: stride multiplier
                                    sw.x[3] = 0; // -> w1 for primitive-setup
                                    sw.x[4] = 3; // w20 -> glDrawElements count (wrapper `mov w1,w20`)
                                    sw.x[5] = 3; // w21: nonzero -> indexed path selection
                                    match arm64jit::jit::jit_run(
                                        iimg,
                                        ibase,
                                        0x105b35288,
                                        &mut sw as *mut CpuState,
                                    ) {
                                        Err(e) => eprintln!(
                                            "[elfjit:renderframe-triangle] geometry wrapper stopped: {e}"
                                        ),
                                        Ok(ok) => eprintln!(
                                            "[elfjit:renderframe-triangle] geometry wrapper 0x5b35288 returned Ok({ok:#x}) (real indexed glDrawElements drawn)"
                                        ),
                                    }
                                    // glReadPixels readback: verify the triangle
                                    // actually drew. Center (0,0 NDC -> ~639,360) should
                                    // be RED; top-left corner should be background.
                                    // glReadPixels verification: 3 probes — triangle centroid interior, left
                                    // background, right background. Proves real drawn
                                    // geometry landed at the expected sub-frame spots.
                                    {
                                        let mut sp = arm64jit::jit::CpuState::new();
                                        sp.tpidr = tpidr;
                                        sp.x[31] = isp;
                                        sp.x[0] = 640;
                                        sp.x[1] = 360;
                                        sp.x[2] = 1;
                                        sp.x[3] = 1;
                                        sp.x[4] = 0x1908; // GL_RGBA
                                        sp.x[5] = 0x1401; // GL_UNSIGNED_BYTE
                                        sp.x[6] = objs.as_ptr() as u64 + 0xf40;
                                        let _ = arm64jit::jit::jit_run(
                                            iimg,
                                            ibase,
                                            plt_readpixels,
                                            &mut sp as *mut CpuState,
                                        );
                                        let _ = sp;
                                    }
                                    let mut pb = arm64jit::jit::CpuState::new();
                                    pb.tpidr = tpidr;
                                    pb.x[31] = isp;
                                    pb.x[0] = 1200;
                                    pb.x[1] = 20;
                                    pb.x[2] = 1;
                                    pb.x[3] = 1;
                                    pb.x[4] = 0x1908;
                                    pb.x[5] = 0x1401;
                                    pb.x[6] = objs.as_ptr() as u64 + 0xf44;
                                    let _ = arm64jit::jit::jit_run(
                                        iimg,
                                        ibase,
                                        plt_readpixels,
                                        &mut pb as *mut CpuState,
                                    );
                                    let pc2 = objs.as_ptr() as u64 + 0xf48;
                                    let mut pc3 = arm64jit::jit::CpuState::new();
                                    pc3.tpidr = tpidr;
                                    pc3.x[31] = isp;
                                    pc3.x[0] = 60;
                                    pc3.x[1] = 20;
                                    pc3.x[2] = 1;
                                    pc3.x[3] = 1;
                                    pc3.x[4] = 0x1908;
                                    pc3.x[5] = 0x1401;
                                    pc3.x[6] = pc2;
                                    let _ = arm64jit::jit::jit_run(
                                        iimg,
                                        ibase,
                                        plt_readpixels,
                                        &mut pc3 as *mut CpuState,
                                    );
                                    unsafe {
                                        let c = |p: u64| -> String {
                                            format!(
                                                "RGBA({},{},{},{})",
                                                *(p as *const u8),
                                                *(p as *const u8).add(1),
                                                *(p as *const u8).add(2),
                                                *(p as *const u8).add(3)
                                            )
                                        };
                                        eprintln!(
                                            "[elfjit:renderframe-triangle] readback: centroid(640,360)={} top-left-bg(60,20)={} top-right-bg(1200,20)={}",
                                            c(objs.as_ptr() as u64 + 0xf40),
                                            c(pc2),
                                            c(pb.x[6])
                                        );
                                    }
                                    // --renderframe-tex readback: 3 on-triangle quadrant probes
                                    // must yield three DIFFERENT texel colors (WHITE/GREEN/RED),
                                    // which a constant shader cannot produce -> proves the sampled
                                    // texture actually rendered.
                                    if tex_mode || comp_mode {
                                        let probes: [(u32, u32, &str, u64); 3] = [
                                            (640, 360, "centroid(WHITE)", 0xf50),
                                            (900, 150, "quad-(1,0)(GREEN)", 0xf54),
                                            (300, 150, "quad-(0,0)(RED)", 0xf58),
                                        ];
                                        for (px, py, label, slot) in probes {
                                            let mut pp = arm64jit::jit::CpuState::new();
                                            pp.tpidr = tpidr;
                                            pp.x[31] = isp;
                                            pp.x[0] = px as u64;
                                            pp.x[1] = py as u64;
                                            pp.x[2] = 1;
                                            pp.x[3] = 1;
                                            pp.x[4] = 0x1908; // GL_RGBA
                                            pp.x[5] = 0x1401; // GL_UNSIGNED_BYTE
                                            pp.x[6] = objs.as_ptr() as u64 + slot;
                                            let _ = arm64jit::jit::jit_run(
                                                iimg,
                                                ibase,
                                                plt_readpixels,
                                                &mut pp as *mut CpuState,
                                            );
                                            unsafe {
                                                let p = objs.as_ptr() as u64 + slot;
                                                let c_ = format!(
                                                    "RGBA({},{},{},{})",
                                                    *(p as *const u8),
                                                    *(p as *const u8).add(1),
                                                    *(p as *const u8).add(2),
                                                    *(p as *const u8).add(3)
                                                );
                                                eprintln!(
                                                    "[elfjit:renderframe-tex] readback {label} @({px},{py}) = {c_}"
                                                );
                                            }
                                        }
                                    }
                                    // --renderframe-triangle-loop <N>: SUSTAINABLE real-
                                    // geometry rendering. Re-run clear (cycling the clear
                                    // color through a palette so a recording proves a
                                    // fresh frame each iteration) -> re-drive the engine's
                                    // own geometry wrapper (same coherent renderer) ->
                                    // swap. Geometry analog of SH23's --rendersustain.
                                    let tri_loop_n: usize = renderframe_args
                                        .iter()
                                        .position(|a| a == "--renderframe-triangle-loop")
                                        .and_then(|i| renderframe_args.get(i + 1))
                                        .and_then(|v| v.parse().ok())
                                        .unwrap_or(0);
                                    if tri_loop_n > 1 {
                                        let palette: [[f32; 3]; 5] = [
                                            [0.05, 0.05, 0.05],
                                            [0.20, 0.05, 0.05],
                                            [0.05, 0.20, 0.05],
                                            [0.05, 0.05, 0.20],
                                            [0.18, 0.10, 0.04],
                                        ];
                                        let mut iter: u64 = 0;
                                        while iter < tri_loop_n as u64 {
                                            let bg = palette[(iter as usize) % palette.len()];
                                            let mut scn = arm64jit::jit::CpuState::new();
                                            scn.tpidr = tpidr;
                                            scn.x[31] = isp;
                                            scn.v[0] = bg[0].to_bits() as u64;
                                            scn.v[2] = bg[1].to_bits() as u64;
                                            scn.v[4] = bg[2].to_bits() as u64;
                                            scn.v[6] = (1.0f32).to_bits() as u64;
                                            let _ = arm64jit::jit::jit_run(
                                                iimg,
                                                ibase,
                                                plt_clearcolor,
                                                &mut scn as *mut CpuState,
                                            );
                                            let _ = gcall(plt_clear, GL_COLOR_BUFFER_BIT, 0, 0, 0, 0, 0);
                                            let mut swn = arm64jit::jit::CpuState::new();
                                            swn.tpidr = tpidr;
                                            swn.x[31] = isp;
                                            swn.x[0] = renderer;
                                            swn.x[1] = 0;
                                            swn.x[2] = 0;
                                            swn.x[3] = 0;
                                            swn.x[4] = 3;
                                            swn.x[5] = 3;
                                            match arm64jit::jit::jit_run(
                                                iimg,
                                                ibase,
                                                0x105b35288,
                                                &mut swn as *mut CpuState,
                                            ) {
                                                Err(e) => eprintln!(
                                                    "[elfjit:renderframe-triangle-loop] iter {iter} wrapper stopped: {e}"
                                                ),
                                                Ok(_) => (),
                                            }
                                            let mut sen = arm64jit::jit::CpuState::new();
                                            sen.tpidr = tpidr;
                                            sen.x[31] = isp;
                                            sen.x[0] = real_ctx;
                                            match arm64jit::jit::jit_run(
                                                iimg,
                                                ibase,
                                                swap_thunk,
                                                &mut sen as *mut CpuState,
                                            ) {
                                                Err(e) => eprintln!(
                                                    "[elfjit:renderframe-triangle-loop] iter {iter} swap stopped: {e}"
                                                ),
                                                Ok(ok) => eprintln!(
                                                    "[elfjit:renderframe-triangle-loop] iter {iter} drew+swap Ok({ok:#x}) bg={bg:?} (fresh real-geometry frame)"
                                                ),
                                            }
                                            iter += 1;
                                            std::thread::sleep(std::time::Duration::from_millis(350));
                                        }
                                    }
                                }
                                // Present the drawn frame.
                                let mut se = arm64jit::jit::CpuState::new();
                                se.tpidr = tpidr;
                                se.x[31] = isp;
                                se.x[0] = real_ctx;
                                match arm64jit::jit::jit_run(iimg, ibase, swap_thunk, &mut se as *mut CpuState) {
                                    Err(e) => eprintln!("[elfjit:renderframe-triangle] swap stopped: {e}"),
                                    Ok(ok) => eprintln!(
                                        "[elfjit:renderframe-triangle] post-draw swap returned Ok({ok:#x})"
                                    ),
                                }
                            }

                            let mut se = arm64jit::jit::CpuState::new();
                            se.tpidr = tpidr;
                            se.x[31] = isp;
                            se.x[0] = real_ctx;
                            match arm64jit::jit::jit_run(iimg, ibase, swap_thunk, &mut se as *mut CpuState) {
                                Err(e) => eprintln!("[elfjit:renderframe-drawprobe] swap stopped: {e}"),
                                Ok(ok) => eprintln!(
                                    "[elfjit:renderframe-drawprobe] post-draw swap returned Ok({ok:#x})"
                                ),
                            }
                        }
// --renderframe-quad: scale the (now fully reversed) coherent renderer onto a
                        // real TWO-ATTRIB textured QUAD — the shape of real Roblox geometry.
                        // primitive-setup 0x5b353d0 loops the primitive list, ONE vertex attrib
                        // per primitive (via a slice at 0x5b35420). Two primitives -> two attribs:
                        //   primitive[0]: vb=0, offset=0,  format[3]={size4,GL_FLOAT}, attrib=0 (aPos)
                        //   primitive[1]: vb=0, offset=16, format[1]={size2,GL_FLOAT}, attrib=1 (aUV)
                        // The VBO is interleaved [pos.xyzw, uv.xy] per vertex (stride 24). The
                        // fragment shader samples a 2x2 texture at the REAL interpolated vertex UV
                        // (not gl_FragCoord) — proving per-texel UV mapping, which no single-attrib
                        // draw path can. Readback: the 4 quadrants read the 4 texel colors.
                        if renderframe_args.iter().any(|a| a == "--renderframe-quad") {
                            // --renderframe-etc2a: like the quad RGBA path but upload the
                            // texture as a REAL ETC2-RGBA8/EAC texture (0x9278 — the real
                            // Android RGBA-EAC format) through glCompressedTexImage2D, and
                            // map the DECODED ALPHA to the fragment RGB. The 4 quadrant
                            // readbacks then read the 4 distinct EAC block alphas as gray
                            // levels (255/190/128/64) — robust proof the EAC alpha
                            // sub-block decodes live (window framebuffers often discard
                            // alpha, so the gray-scale mapping makes it window-capturable).
                            let etc2a_mode = renderframe_args.iter().any(|a| a == "--renderframe-etc2a");
                            // --renderframe-astc: like --renderframe-etc2a but upload the texture
                            // as a REAL ASTC 4x4 LDR void-extent texture (0x93B0 — the load-bearing
                            // Android format desktop GL cannot native-decode, so our interception is
                            // REQUIRED there). Same gray-scale alpha->RGB proof: the 4 blocks' EAC-free
                            // ASTC void-extent alphas (255/190/128/64) render as 4 gray lobes.
                            let astc_mode = renderframe_args.iter().any(|a| a == "--renderframe-astc");
                            let comp_gray = etc2a_mode || astc_mode;
                            // --renderframe-quad-loop <N>: SUSTAINABLE textured-quad rendering —
                            // after the single proof frame, re-drive clear(cycling bg) ->
                            // engine geometry wrapper -> swap N times on the detached host thread,
                            // so a recording proves a fresh textured geometry render every frame
                            // (the last property a real main-loop frame drive needs for the
                            // textured/mesh path; geometry analog of SH25b's triangle-loop).
                            let quad_loop_n: Option<u32> = renderframe_args
                                .iter()
                                .position(|a| a == "--renderframe-quad-loop")
                                .and_then(|i| renderframe_args.get(i + 1))
                                .and_then(|s| s.parse().ok())
                                .or_else(|| {
                                    renderframe_args.iter().any(|a| a == "--renderframe-quad-loop").then_some(6)
                                });
                            // --renderframe-grid <N>: scale the coherent renderer onto a REAL
                            // larger mesh — an NxN grid of textured quads (N>1 => (N+1)^2
                            // verts, 6*N^2 indices, one distinct texel color per cell drawn
                            // at the real interpolated UV). Proves the engine's OWN geometry
                            // wrapper + primitive-setup loop render a mesh of real topology
                            // (many verts/indices), not just a single 4-vert quad (SH25-33).
                            // Readback probes each cell center, which must read that cell's
                            // distinct texel — per-cell UV->texel mapping across the mesh.
                            let grid_n: Option<u32> = renderframe_args
                                .iter()
                                .position(|a| a == "--renderframe-grid")
                                .and_then(|i| renderframe_args.get(i + 1))
                                .and_then(|s| s.parse().ok())
                                .filter(|&n| n >= 2 && n <= 8);
                            const GL_ARRAY_BUFFER: u64 = 0x8892;
                            const GL_ELEMENT_ARRAY_BUFFER: u64 = 0x8893;
                            const GL_STATIC_DRAW: u64 = 0x88e4;
                            const GL_FLOAT: u64 = 0x1406;
                            const GL_VERTEX_SHADER: u64 = 0x8b31;
                            const GL_FRAGMENT_SHADER: u64 = 0x8b30;
                            const GL_TEXTURE0: u64 = 0x84c0;
                            const GL_TEXTURE_2D: u64 = 0x0de1;
                            const GL_RGBA: u64 = 0x1908;
                            const GL_UNSIGNED_BYTE: u64 = 0x1401;
                            const GL_NEAREST: u64 = 0x2600;
                            const GL_COLOR_BUFFER_BIT: u64 = 0x4000;
                            let pb = |a: u64| -> Result<u64, String> {
                                let mut s = arm64jit::jit::CpuState::new();
                                s.tpidr = tpidr; s.x[31] = isp;
                                s.x[0] = a;
                                arm64jit::jit::jit_run(iimg, ibase, a, &mut s as *mut CpuState).map(|_| s.x[0])
                            };
                            let gcall = |addr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64| -> Result<u64, String> {
                                let mut s = arm64jit::jit::CpuState::new();
                                s.tpidr = tpidr; s.x[31] = isp;
                                s.x[0]=a0; s.x[1]=a1; s.x[2]=a2; s.x[3]=a3; s.x[4]=a4; s.x[5]=a5;
                                arm64jit::jit::jit_run(iimg, ibase, addr, &mut s as *mut CpuState).map(|_| s.x[0])
                            };
                            let objs = Box::leak(vec![0u8; 32768].into_boxed_slice());
                            let base = objs.as_ptr() as u64;
                            unsafe {
                                // Clear + viewport.
                                {
                                    let mut sc = arm64jit::jit::CpuState::new();
                                    sc.tpidr = tpidr; sc.x[31] = isp;
                                    sc.v[0] = (0.0f32).to_bits() as u64;
                                    sc.v[2] = (0.0f32).to_bits() as u64;
                                    sc.v[4] = (0.3f32).to_bits() as u64;
                                    sc.v[6] = (1.0f32).to_bits() as u64;
                                    let _ = arm64jit::jit::jit_run(iimg, ibase, 0x1062d7710, &mut sc as *mut CpuState);
                                }
                                let _ = gcall(0x1062d7740, GL_COLOR_BUFFER_BIT, 0,0,0,0,0);
                                let _ = gcall(0x1062d75c0, 0,0,1280,720,0,0); // glViewport
                                let _ = gcall(0x1062d75d0, 0,0,1280,720,0,0); // glScissor
                                // Shaders with a real UV varying.
                                let vs_src = b"attribute vec4 aPos;\nattribute vec2 aUV;\nvarying vec2 vUV;\nvoid main(){ vUV = aUV; gl_Position = aPos; }\n\0";
                                let fs_src: &[u8] = if comp_gray {
                                    // Output the DECODED ALPHA as RGB gray-scale (alpha=A->RGB)
                                    // so the EAC/ASTC alpha sub-block is window-capturable even on an
                                    // alpha-less window framebuffer.
                                    b"precision mediump float;\nuniform sampler2D uTex;\nvarying vec2 vUV;\nvoid main(){ vec4 t = texture2D(uTex, vUV); gl_FragColor = vec4(t.aaa, 1.0); }\n\0"
                                } else {
                                    b"precision mediump float;\nuniform sampler2D uTex;\nvarying vec2 vUV;\nvoid main(){ gl_FragColor = texture2D(uTex, vUV); }\n\0"
                                };
                                let vs_ptr = base + 0x400;
                                let fs_ptr = base + 0x800;
                                std::ptr::copy_nonoverlapping(vs_src.as_ptr(), vs_ptr as *mut u8, vs_src.len());
                                std::ptr::copy_nonoverlapping(fs_src.as_ptr(), fs_ptr as *mut u8, fs_src.len());
                                let vs_ary = base + 0xa00;
                                let fs_ary = base + 0xa10;
                                *(vs_ary as *mut u64) = vs_ptr;
                                *(fs_ary as *mut u64) = fs_ptr;
                                let vs = gcall(0x1062d7880, GL_VERTEX_SHADER, 0,0,0,0,0).unwrap_or(0) & 0xffff_ffff;
                                let _ = gcall(0x1062d7890, vs, 1, vs_ary, 0,0,0); // glShaderSource
                                let _ = gcall(0x1062d78a0, vs, 0,0,0,0,0);      // glCompileShader
                                let fs = gcall(0x1062d7880, GL_FRAGMENT_SHADER, 0,0,0,0,0).unwrap_or(0) & 0xffff_ffff;
                                let _ = gcall(0x1062d7890, fs, 1, fs_ary, 0,0,0);
                                let _ = gcall(0x1062d78a0, fs, 0,0,0,0,0);
                                let prog = gcall(0x1062d78c0, 0,0,0,0,0,0).unwrap_or(0) & 0xffff_ffff; // glCreateProgram
                                let _ = gcall(0x1062d78d0, prog, vs, 0,0,0,0); // glAttachShader
                                let _ = gcall(0x1062d78d0, prog, fs, 0,0,0,0);
                                let pos_name = base + 0xd00; std::ptr::copy_nonoverlapping(b"aPos\0".as_ptr(), pos_name as *mut u8, 5);
                                let uv_name = base + 0xd20; std::ptr::copy_nonoverlapping(b"aUV\0".as_ptr(), uv_name as *mut u8, 5);
                                let _ = gcall(0x1062d78f0, prog, 0, pos_name, 0,0,0); // glBindAttribLocation aPos->0
                                let _ = gcall(0x1062d78f0, prog, 1, uv_name, 0,0,0);  // glBindAttribLocation aUV->1
                                let _ = gcall(0x1062d78e0, prog, 0,0,0,0,0);          // glLinkProgram
                                let _ = gcall(0x1062d75a0, prog, 0,0,0,0,0);          // glUseProgram
                                // Texture: default = 2x2 RGBA checkerboard RED/GREEN/BLUE/WHITE;
                                // --renderframe-etc2a = a REAL 8x8 ETC2-RGBA8/EAC texture (0x9278)
                                // via glCompressedTexImage2D (the last compressed format with an
                                // unimplemented live-path prove). The bridge decodes ETC2-RGBA8 and
                                // re-uploads, so the EAC alpha + RGB both reach the quad.
                                let tex_data = base + 0x6000;
                                let tex_sp = base + 0xf80;
                                let tex_id_slot = base + 0xfd0;
                                if comp_gray {
                                    // ETC2-RGBA8 8x8 = 4 x 16-byte blocks; ASTC 4x4 8x8 = 4 x 16-byte
                                    // LDR void-extent blocks. Both encode a solid color+alpha per
                                    // 4x4 block, so the 4 blocks give 4 distinct alphas (255/190/
                                    // 128/64). The ETC2-RGBA8 RGB is the SH29-proven ETC2 color; the
                                    // ASTC RGB=alpha. FS maps alpha->RGB so the quadrant readbacks
                                    // read those 4 gray levels.
                                    let (comp_fmt, ctex): (u64, [u8; 64]) = if astc_mode {
                                        // ASTC LDR void-extent: bytes 9/11/13/15 = UNORM16 high bytes
                                        // of R/G/B/A (Khronos void-extent block, buf[0]=0xFC).
                                        let ve = |g: u8| -> [u8; 16] {
                                            let mut d = [0u8; 16];
                                            d[0] = 0xFC; d[1] = 0x01;
                                            d[9] = g; d[11] = g; d[13] = g; d[15] = g;
                                            d
                                        };
                                        let mut d = [0u8; 64];
                                        d[0..16].copy_from_slice(&ve(255));
                                        d[16..32].copy_from_slice(&ve(190));
                                        d[32..48].copy_from_slice(&ve(128));
                                        d[48..64].copy_from_slice(&ve(64));
                                        (0x93B0u64, d)
                                    } else {
                                        const GL_COMPRESSED_RGBA8_ETC2_EAC: u64 = 0x9278;
                                        let enc = |t: i32| -> u8 { let c = ((t - 2).clamp(0, 240) >> 4) as u8; (c << 4) | c };
                                        let blk = |r: u8, g: u8, b: u8| -> [u8; 8] { [r, g, b, 0, 0, 0, 0, 0] };
                                        let rgba8 = |a: u8, rgb: [u8; 8]| -> [u8; 16] {
                                            let mut d = [0u8; 16];
                                            d[0] = a; d[1] = 0;
                                            d[8..16].copy_from_slice(&rgb);
                                            d
                                        };
                                        let mut d = [0u8; 64];
                                        d[0..16].copy_from_slice(&rgba8(255, blk(enc(255), enc(2), enc(2))));   // block0
                                        d[16..32].copy_from_slice(&rgba8(190, blk(enc(2), enc(255), enc(2))));  // block1
                                        d[32..48].copy_from_slice(&rgba8(128, blk(enc(2), enc(2), enc(255))));  // block2
                                        d[48..64].copy_from_slice(&rgba8(64, blk(enc(255), enc(255), enc(255)))); // block3
                                        (GL_COMPRESSED_RGBA8_ETC2_EAC, d)
                                    };
                                    std::ptr::copy_nonoverlapping(ctex.as_ptr(), tex_data as *mut u8, 64);
                                    let _ = gcall(0x1062d7980, 1, tex_id_slot, 0,0,0,0); // glGenTextures
                                    let _ = gcall(0x1062d75e0, GL_TEXTURE0, 0,0,0,0,0);     // glActiveTexture
                                    let _ = gcall(0x1062d75f0, GL_TEXTURE_2D, *(tex_id_slot as *const u32) as u64, 0,0,0,0); // glBindTexture
                                    let _ = gcall(0x1062d7960, GL_TEXTURE_2D, 0x2801, GL_NEAREST, 0,0,0); // MIN
                                    let _ = gcall(0x1062d7960, GL_TEXTURE_2D, 0x2800, GL_NEAREST, 0,0,0); // MAG
                                    let mut sce = arm64jit::jit::CpuState::new();
                                    sce.tpidr = tpidr; sce.x[31] = isp;
                                    sce.x[0] = GL_TEXTURE_2D; sce.x[1] = 0; sce.x[2] = comp_fmt;
                                    sce.x[3] = 8; sce.x[4] = 8; sce.x[5] = 0; sce.x[6] = 64; sce.x[7] = tex_data;
                                    let what = if astc_mode { "ASTC 4x4 (0x93B0) LDR void-extent" } else { "ETC2-RGBA8 (0x9278)" };
                                    eprintln!("[elfjit:renderframe] uploading 8x8 {what} 4-block via glCompressedTexImage2D");
                                    let _ = arm64jit::jit::jit_run(iimg, ibase, 0x1062d7990, &mut sce as *mut CpuState); // glCompressedTexImage2D
                                } else {
                                    // Grid mode: an NxN RGBA texture, one DISTINCT solid color
                                    // per texel (gi,gj). Each grid cell samples exactly one texel
                                    // (all 4 of its verts share the texel-center UV, so the whole
                                    // cell renders flat) -> a readback at any cell center must
                                    // read that texel's unique color. Single-quad mode keeps the
                                    // 2x2 checkerboard. tex_w/tex_h/tex_len below are used for the
                                    // glTexImage2D dims + data length.
                                    let (tex_w, tex_h, tex_len) = match grid_n {
                                        Some(n) => (n, n, (n * n) as usize * 4),
                                        None => (2, 2, 16),
                                    };
                                    let mut tex: Vec<u8> = vec![0u8; tex_len];
                                    if let Some(n) = grid_n {
                                        for gj in 0..n {
                                            for gi in 0..n {
                                                let i = (gj * n + gi) as usize * 4;
                                                tex[i] = (gi as f32 / (n - 1) as f32 * 255.0) as u8;
                                                tex[i + 1] = (gj as f32 / (n - 1) as f32 * 255.0) as u8;
                                                tex[i + 2] = 64;
                                                tex[i + 3] = 255;
                                            }
                                        }
                                    } else {
                                        tex.copy_from_slice(&[255,0,0,255, 0,255,0,255, 0,0,255,255, 255,255,255,255]);
                                    }
                                    std::ptr::copy_nonoverlapping(tex.as_ptr(), tex_data as *mut u8, tex_len);
                                    *(tex_sp as *mut u64) = tex_data;
                                    let _ = gcall(0x1062d7980, 1, tex_id_slot, 0,0,0,0); // glGenTextures
                                    let _ = gcall(0x1062d75e0, GL_TEXTURE0, 0,0,0,0,0);     // glActiveTexture
                                    let _ = gcall(0x1062d75f0, GL_TEXTURE_2D, *(tex_id_slot as *const u32) as u64, 0,0,0,0); // glBindTexture
                                    let _ = gcall(0x1062d7960, GL_TEXTURE_2D, 0x2801, GL_NEAREST, 0,0,0); // MIN
                                    let _ = gcall(0x1062d7960, GL_TEXTURE_2D, 0x2800, GL_NEAREST, 0,0,0); // MAG
                                    let mut steg = arm64jit::jit::CpuState::new();
                                    steg.tpidr = tpidr; steg.x[31] = tex_sp;
                                    steg.x[0]=GL_TEXTURE_2D; steg.x[1]=0; steg.x[2]=GL_RGBA; steg.x[3]=tex_w as u64; steg.x[4]=tex_h as u64; steg.x[5]=0; steg.x[6]=GL_RGBA; steg.x[7]=GL_UNSIGNED_BYTE;
                                    let _ = arm64jit::jit::jit_run(iimg, ibase, 0x1062d79a0, &mut steg as *mut CpuState); // glTexImage2D
                                    let _ = tex_sp;
                                }
                                let tex_id = *(tex_id_slot as *const u32) as u64;
                                let uni = base + 0xe20; std::ptr::copy_nonoverlapping(b"uTex\0".as_ptr(), uni as *mut u8, 5);
                                let ploc = gcall(0x1062d7900, prog, uni, 0,0,0,0).unwrap_or(0) & 0xffff_ffff;
                                let _ = gcall(0x1062d7910, ploc, 0,0,0,0,0); // glUniform1i(uTex,0)
                                eprintln!("[elfjit:renderframe-quad] program={prog:#x} compiled+linked; texture tex_id={tex_id:#x} uTex={ploc:#x}");
                                // Mesh construction: single quad (SH25-33) vs an NxN grid
                                                                // (--renderframe-grid). The grid uses INDEPENDENT per-cell
                                                                // quads (4 verts + 6 idx each), every vertex of a cell
                                                                // sharing that cell's texel-center UV, so each cell renders
                                                                // flat with its own distinct texel color -> a readback at any
                                                                // cell center must read that texel. Grid = a REAL mesh:
                                                                // (4*N*N) verts + (6*N*N) idx through the engine's own
                                                                // primitive-setup + draw wrapper.
                                                                let (n, nv, ni) = match grid_n {
                                                                    Some(n) => {
                                                                        let u = n as usize;
                                                                        (u, 4 * u * u, 6 * u * u)
                                                                    }
                                                                    None => (1usize, 4, 6),
                                                                };
                                                                let mut verts: Vec<f32> = Vec::with_capacity(nv * 6);
                                                                let mut idxs: Vec<u32> = Vec::with_capacity(ni);
                                                                let cell = 1.8f32 / n as f32;
                                                                if grid_n.is_some() {
                                                                    // Grid mode: each cell's 4 verts share the texel-center UV
                                                                    // so the whole cell renders flat with ITS distinct texel.
                                                                    for gj in 0..n {
                                                                        for gi in 0..n {
                                                                            let x0 = -0.9f32 + gi as f32 * cell;
                                                                            let y0 = -0.9f32 + gj as f32 * cell;
                                                                            let (x1, y1) = (x0 + cell, y0 + cell);
                                                                            let u = (gi as f32 + 0.5) / n as f32;
                                                                            let v = (gj as f32 + 0.5) / n as f32;
                                                                            let base = (gj * n + gi) as u32 * 4;
                                                                            verts.extend_from_slice(&[x0,y0,0.0,1.0, u,v]); // 0 bl
                                                                            verts.extend_from_slice(&[x1,y0,0.0,1.0, u,v]); // 1 br
                                                                            verts.extend_from_slice(&[x1,y1,0.0,1.0, u,v]); // 2 tr
                                                                            verts.extend_from_slice(&[x0,y1,0.0,1.0, u,v]); // 3 tl
                                                                            idxs.extend_from_slice(&[base, base+1, base+2, base, base+2, base+3]);
                                                                        }
                                                                    }
                                                                } else {
                                                                    // Single-quad mode (SH25-33): per-corner UVs map the 2x2
                                                                    // checkerboard (RED/GREEN/BLUE/WHITE) to the 4 quadrants —
                                                                    // keep the original corner UVs so the 4 readbacks stay distinct.
                                                                    verts.extend_from_slice(&[-0.9,-0.9,0.0,1.0, 0.0,0.0]);
                                                                    verts.extend_from_slice(&[0.9,-0.9,0.0,1.0, 1.0,0.0]);
                                                                    verts.extend_from_slice(&[0.9,0.9,0.0,1.0, 1.0,1.0]);
                                                                    verts.extend_from_slice(&[-0.9,0.9,0.0,1.0, 0.0,1.0]);
                                                                    idxs.extend_from_slice(&[0,1,2, 0,2,3]);
                                                                }
                                                                let vbo_bytes = (nv * 6 * 4) as u64;
                                                                let ebo_bytes = (ni * 4) as u64;
                                let vbo_data = base + 0x2000;
                                let ebo_data = base + 0x4000;
                                                                std::ptr::copy_nonoverlapping(verts.as_ptr() as *const u8, vbo_data as *mut u8, vbo_bytes as usize);
                                                                std::ptr::copy_nonoverlapping(idxs.as_ptr() as *const u8, ebo_data as *mut u8, ebo_bytes as usize);
                                                                let vbo_slot = base + 0xf00;
                                                                let ebo_slot = base + 0xf10;
                                                                let _ = gcall(0x1062d77c0, 1, vbo_slot, 0,0,0,0); // glGenBuffers
                                                                let vbo = *(vbo_slot as *const u32) as u64;
                                                                let _ = gcall(0x1062d77b0, GL_ARRAY_BUFFER, vbo, 0,0,0,0);
                                                                let _ = gcall(0x1062d77d0, GL_ARRAY_BUFFER, vbo_bytes, vbo_data, GL_STATIC_DRAW, 0,0); // glBufferData
                                                                let _ = gcall(0x1062d77c0, 1, ebo_slot, 0,0,0,0);
                                                                let ebo = *(ebo_slot as *const u32) as u64;
                                                                let _ = gcall(0x1062d77b0, GL_ELEMENT_ARRAY_BUFFER, ebo, 0,0,0,0);
                                                                let _ = gcall(0x1062d77d0, GL_ELEMENT_ARRAY_BUFFER, ebo_bytes, ebo_data, GL_STATIC_DRAW, 0,0);
                                                                eprintln!("[elfjit:renderframe-quad] vbo={vbo:#x} ebo={ebo:#x} {nv} interleaved verts stride24 + {ni} idx uploaded ({n}x{n} grid)");
                                // Coherent renderer with TWO primitives -> TWO attribs.
                                let renderer = base;
                                let container = base + 0x100;
                                let desc = base + 0x200;
                                let stride_tbl = base + 0x300;
                                let prim0 = base + 0x400;
                                let prim1 = base + 0x418;
                                let ibo = base + 0x500;
                                *(desc.wrapping_add(72) as *mut u32) = vbo as u32;
                                *(stride_tbl as *mut u64) = 24;
                                *(renderer.wrapping_add(56) as *mut u64) = container;
                                *(container.wrapping_add(72) as *mut u64) = prim0;
                                *(container.wrapping_add(80) as *mut u64) = prim1 + 0x18;
                                *(container.wrapping_add(96) as *mut u64) = stride_tbl;
                                *(renderer.wrapping_add(0x48) as *mut u64) = desc;
                                // prim0: vb0 off0 fmt3(size4 float) attrib0 = aPos
                                *(prim0 as *mut u32) = 0; *(prim0.wrapping_add(4) as *mut u32)=0; *(prim0.wrapping_add(8) as *mut u32)=3; *(prim0.wrapping_add(12) as *mut u32)=0; *(prim0.wrapping_add(16) as *mut u32)=0;
                                // prim1: vb0 off16 fmt1(size2 float) attrib1 = aUV
                                *(prim1 as *mut u32) = 0; *(prim1.wrapping_add(4) as *mut u32)=16; *(prim1.wrapping_add(8) as *mut u32)=1; *(prim1.wrapping_add(12) as *mut u32)=1; *(prim1.wrapping_add(16) as *mut u32)=0;
                                *(renderer.wrapping_add(120) as *mut u64) = ibo;
                                                                *(ibo.wrapping_add(72) as *mut u32) = ebo as u32;
                                                                *(renderer.wrapping_add(142) as *mut u16) = ni as u16;
                                                                eprintln!("[elfjit:renderframe-quad] coherent renderer: 2-prim list (aPos+aUV) + {ni}-idx EBO fabricated");
                                                                // Drive the engine's OWN geometry wrapper.
                                                                let mut sw = arm64jit::jit::CpuState::new();
                                                                sw.tpidr = tpidr; sw.x[31] = isp;
                                                                sw.x[0] = renderer;
                                                                sw.x[1] = 0; sw.x[2] = 0; sw.x[3] = 0;
                                                                sw.x[4] = ni as u64; // glDrawElements count
                                                                sw.x[5] = 3; // nonzero -> indexed
                                                                let shape = if grid_n.is_some() { format!("{n}x{n} MESH drawn") } else { "textured QUAD drawn".to_string() };
                                                                match arm64jit::jit::jit_run(iimg, ibase, 0x105b35288, &mut sw as *mut CpuState) {
                                                                    Err(e) => eprintln!("[elfjit:renderframe-quad] geometry wrapper stopped: {e}"),
                                                                    Ok(ok) => eprintln!("[elfjit:renderframe-quad] geometry wrapper 0x5b35288 returned Ok({ok:#x}) ({shape})"),
                                                                }
                                // Readback: grid mode probes EVERY cell center (must read that cell's
                                // distinct texel); single-quad mode keeps the 4 fixed
                                // texel-corner probes (SH30).
                                if let Some(gn) = grid_n {
                                    for gj in 0..gn {
                                        for gi in 0..gn {
                                            let cndc_x = -0.9f32 + gi as f32 * cell + cell / 2.0;
                                            let cndc_y = -0.9f32 + gj as f32 * cell + cell / 2.0;
                                            let px = ((cndc_x + 1.0) * 640.0) as u32;
                                            let py = ((cndc_y + 1.0) * 360.0) as u32;
                                            let slot = 0xf40 + (((gj * gn + gi) % 8) as u64) * 4;
                                            let mut pr = arm64jit::jit::CpuState::new();
                                            pr.tpidr = tpidr; pr.x[31] = isp;
                                            pr.x[0]=px as u64; pr.x[1]=py as u64; pr.x[2]=1; pr.x[3]=1; pr.x[4]=0x1908; pr.x[5]=0x1401; pr.x[6]=base+slot;
                                            let _ = arm64jit::jit::jit_run(iimg, ibase, 0x1062d7940, &mut pr as *mut CpuState);
                                            let b = base + slot;
                                            let c = format!("RGBA({},{},{},{})", *(b as *const u8), *(b as *const u8).add(1), *(b as *const u8).add(2), *(b as *const u8).add(3));
                                            eprintln!("[elfjit:renderframe-quad] cell({gi},{gj})@({px},{py}) readback={c} (expect r={:.0} g={:.0})", gi as f32/(gn-1) as f32*255.0, gj as f32/(gn-1) as f32*255.0);
                                        }
                                    }
                                } else {
                                    let probes: [(u32,u32,&str,u64);4] = [
                                        (320,180,"BL-red(0,0)",0xf40), (960,180,"BR-green(1,0)",0xf44),
                                        (960,540,"TR-white(1,1)",0xf48), (320,540,"TL-blue(0,1)",0xf4c)];
                                    for (px,py,label,slot) in probes {
                                        let mut pr = arm64jit::jit::CpuState::new();
                                        pr.tpidr = tpidr; pr.x[31] = isp;
                                        pr.x[0]=px as u64; pr.x[1]=py as u64; pr.x[2]=1; pr.x[3]=1; pr.x[4]=0x1908; pr.x[5]=0x1401; pr.x[6]=base+slot;
                                        let _ = arm64jit::jit::jit_run(iimg, ibase, 0x1062d7940, &mut pr as *mut CpuState);
                                    }
                                    {
                                        let p = |slot:u64| -> String { let b=base+slot; format!("RGBA({},{},{},{})",*(b as *const u8),*(b as *const u8).add(1),*(b as *const u8).add(2),*(b as *const u8).add(3)) };
                                        eprintln!("[elfjit:renderframe-quad] readback: BL={} BR={} TR={} TL={}", p(0xf40), p(0xf44), p(0xf48), p(0xf4c));
                                    }
                                }
                                // Swap.
                                let mut se = arm64jit::jit::CpuState::new();
                                se.tpidr = tpidr; se.x[31] = isp; se.x[0] = real_ctx;
                                match arm64jit::jit::jit_run(iimg, ibase, swap_thunk, &mut se as *mut CpuState) {
                                    Err(e) => eprintln!("[elfjit:renderframe-quad] swap stopped: {e}"),
                                    Ok(ok) => eprintln!("[elfjit:renderframe-quad] post-draw swap returned Ok({ok:#x})"),
                                }
                                // --renderframe-quad-loop <N>: SUSTAINABLE textured-quad frames.
                                // The single proof frame is done above (including the readback).
                                // Now re-drive clear(cycling bg) -> wrapper -> swap N times so a
                                // recording proves a FRESH textured render every iteration.
                                if let Some(n) = quad_loop_n {
                                    let bgs: [[f32; 4]; 5] = [
                                        [0.9, 0.1, 0.1, 1.0], [0.1, 0.9, 0.1, 1.0], [0.1, 0.1, 0.9, 1.0],
                                        [0.9, 0.9, 0.1, 1.0], [0.9, 0.1, 0.9, 1.0],
                                    ];
                                    for iter in 0..n {
                                        let bg = bgs[(iter as usize) % 5];
                                        let mut sc = arm64jit::jit::CpuState::new();
                                        sc.tpidr = tpidr; sc.x[31] = isp;
                                        sc.v[0] = bg[0].to_bits() as u64;
                                        sc.v[2] = bg[1].to_bits() as u64;
                                        sc.v[4] = bg[2].to_bits() as u64;
                                        sc.v[6] = bg[3].to_bits() as u64;
                                        let _ = arm64jit::jit::jit_run(iimg, ibase, 0x1062d7710, &mut sc as *mut CpuState); // glClearColor
                                        let _ = gcall(0x1062d7740, GL_COLOR_BUFFER_BIT, 0, 0, 0, 0, 0);                    // glClear
                                        let mut swn = arm64jit::jit::CpuState::new();
                                        swn.tpidr = tpidr; swn.x[31] = isp;
                                        swn.x[0] = renderer; swn.x[1] = 0; swn.x[2] = 0; swn.x[3] = 0;
                                        swn.x[4] = ni as u64; swn.x[5] = 3;
                                        if let Err(e) = arm64jit::jit::jit_run(iimg, ibase, 0x105b35288, &mut swn as *mut CpuState) {
                                            eprintln!("[elfjit:renderframe-quad-loop] iter {iter} wrapper stopped: {e}");
                                            continue;
                                        }
                                        let mut sen = arm64jit::jit::CpuState::new();
                                        sen.tpidr = tpidr; sen.x[31] = isp; sen.x[0] = real_ctx;
                                        match arm64jit::jit::jit_run(iimg, ibase, swap_thunk, &mut sen as *mut CpuState) {
                                            Err(e) => eprintln!("[elfjit:renderframe-quad-loop] iter {iter} swap stopped: {e}"),
                                            Ok(ok) => eprintln!("[elfjit:renderframe-quad-loop] iter {iter} drew+swap Ok({ok:#x}) bg={bg:?} (fresh textured-quad frame)"),
                                        }
                                        std::thread::sleep(std::time::Duration::from_millis(350));
                                    }
                                }
                            }
                        }

                    }
                }
                // --renderclear <r,g,b,a>: draw an actual colored clear through the
                // JIT's GLES float bridge on this live context, then swap again, so
                // the presented frame is non-black (the idle main loop never issues
                // glClearColor itself). We drive the guest PLT entries directly:
                // glClearColor@plt 0x1062d7710 (float args in s0..s3, i.e. v[0..6]
                // low lanes) then glClear@plt 0x1062d7740 (GL_COLOR_BUFFER_BIT=0x4000
                // in x0), each through jit_run -> plt stub `br`s to the host GLES
                // bridge -> real Mesa on the already-current context.
                if renderframe_args.iter().any(|a| a == "--renderclear") {
                    let cc: Vec<f32> = renderframe_args
                        .iter()
                        .position(|a| a == "--renderclear")
                        .and_then(|i| renderframe_args.get(i + 1).cloned())
                        .map(|h| {
                            h.split(',')
                                .filter_map(|x| x.parse::<f32>().ok())
                                .collect()
                        })
                        .unwrap_or_else(|| vec![0.2, 0.6, 1.0, 1.0]);
                    let (mut cr, mut cg, mut cb, mut ca) = (0.2f32, 0.6f32, 1.0f32, 1.0f32);
                    if cc.len() >= 4 {
                        cr = cc[0];
                        cg = cc[1];
                        cb = cc[2];
                        ca = cc[3];
                    }
                    let mut s6 = arm64jit::jit::CpuState::new();
                    s6.tpidr = tpidr;
                    s6.x[31] = isp;
                    s6.v[0] = cr.to_bits() as u64;
                    s6.v[2] = cg.to_bits() as u64;
                    s6.v[4] = cb.to_bits() as u64;
                    s6.v[6] = ca.to_bits() as u64;
                    match arm64jit::jit::jit_run(iimg, ibase, 0x1062d7710, &mut s6 as *mut CpuState) {
                        Err(e) => eprintln!("[elfjit:renderclear] glClearColor stopped: {e}"),
                        Ok(_) => eprintln!("[elfjit:renderclear] glClearColor via bridge Ok"),
                    }
                    let mut s7 = arm64jit::jit::CpuState::new();
                    s7.tpidr = tpidr;
                    s7.x[31] = isp;
                    s7.x[0] = 0x4000; // GL_COLOR_BUFFER_BIT
                    match arm64jit::jit::jit_run(iimg, ibase, 0x1062d7740, &mut s7 as *mut CpuState) {
                        Err(e) => eprintln!("[elfjit:renderclear] glClear stopped: {e}"),
                        Ok(_) => eprintln!("[elfjit:renderclear] glClear via bridge Ok"),
                    }
                    let mut s8 = arm64jit::jit::CpuState::new();
                    s8.tpidr = tpidr;
                    s8.x[31] = isp;
                    s8.x[0] = real_ctx;
                    match arm64jit::jit::jit_run(iimg, ibase, swap_thunk, &mut s8 as *mut CpuState) {
                        Err(e) => eprintln!("[elfjit:renderclear] swap stopped: {e}"),
                        Ok(ok) => eprintln!("[elfjit:renderclear] swap returned Ok({ok:#x}) (eglSwapBuffers after clear)"),
                    }
                }
                // --renderframe-progbin: prove the SH35-sealed GLES3 pipeline slots are
                // genuinely FUNCTIONAL (not just resolvable) by driving the engine's OWN
                // dispatch stubs 0x5b3a1c0+0xc*N (the exact `adrp x8,6d3b000; ldr x3,[x8,
                // #752+8N]; br x3` mechanism a real session's frame dispatches through)
                // with real guest-ABI args on the live context. Covers the three modern
                // pipelines SH35 bridged:
                //   A. program-binary round-trip   (slots 13/14 glGetProgramBinary/glProgramBinary,
                //                                   slot 15 glProgramParameteri retrievable hint)
                //   B. UBO bind-through-slot       (slot 5  glBindBufferBase GL_UNIFORM_BUFFER)
                //   C. instanced draw dispatch     (slot 10 glDrawArraysInstanced, count=0 no-op)
                // Each drives a SEALED bridge slot exactly as the engine would; a mis-bridged
                // or crash-prone slot surfaces as a Mesa GL error or a crash (exit != 124).
                if renderframe_args.iter().any(|a| a == "--renderframe-progbin") {
                    unsafe {
                    // Guest addresses of the engine's slot stubs (bl-targets in its clear/
                    // draw code; a guest `br` to these re-dispatches through slot N's bridge).
                    const SLOT4: u64 = 0x105b3a1f0; // glUniformBlockBinding
                    const SLOT5: u64 = 0x105b3a1fc; // glBindBufferBase
                    const SLOT7: u64 = 0x105b3a214; // glGetUniformBlockIndex
                    const SLOT10: u64 = 0x105b3a238; // glDrawArraysInstanced
                    const SLOT13: u64 = 0x105b3a25c; // glGetProgramBinary
                    const SLOT14: u64 = 0x105b3a268; // glProgramBinary
                    const SLOT15: u64 = 0x105b3a274; // glProgramParameteri
                    // GLES PLT stubs for the setup that must NOT go through the sealed slots
                    // (shader compile, buffer create) — same addrs the triangle harness uses.
                    let plt_createshader = 0x1062d7880u64;
                    let plt_shadersource = 0x1062d7890u64;
                    let plt_compileshader = 0x1062d78a0u64;
                    let plt_attachshader = 0x1062d78d0u64;
                    let plt_linkprogram = 0x1062d78e0u64;
                    let plt_bindattrib = 0x1062d78f0u64;
                    let plt_genbuffers = 0x1062d77c0u64;
                    let plt_bindbuffer = 0x1062d77b0u64;
                    let plt_buffdata = 0x1062d77d0u64;
                    let plt_geterror = 0x1062d7580u64; // glGetError
                    let plt_createprogram = 0x1062d78c0u64;
                    let scratch = Box::leak(vec![0u8; 8192].into_boxed_slice());
                    let sc_base = scratch.as_ptr() as u64;
                    // Drive a bridge slot stub (a bare `br` to the sealed slot) or a PLT stub.
                    let mut gslot = |addr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64| {
                        let mut s = arm64jit::jit::CpuState::new();
                        s.tpidr = tpidr;
                        s.x[31] = isp;
                        s.x[0] = a0;
                        s.x[1] = a1;
                        s.x[2] = a2;
                        s.x[3] = a3;
                        s.x[4] = a4;
                        s.x[5] = a5;
                        let r = arm64jit::jit::jit_run(iimg, ibase, addr, &mut s as *mut CpuState);
                        (r, s.x[0])
                    };
                    // curl clear-error helper
                    let geterr = || gslot(plt_geterror, 0, 0, 0, 0, 0, 0).1 as u32;
                    // Compile+link a minimal program (pass-through VS, red FS).
                    let vs_src = b"attribute vec4 aPos;\nvoid main(){ gl_Position = vec4(aPos.xyz,1.0); }\n\0";
                    let fs_src = b"void main(){ gl_FragColor = vec4(1.0,0.2,0.2,1.0); }\n\0";
                    std::ptr::copy_nonoverlapping(vs_src.as_ptr(), (sc_base + 0x400) as *mut u8, vs_src.len());
                    std::ptr::copy_nonoverlapping(fs_src.as_ptr(), (sc_base + 0x800) as *mut u8, fs_src.len());
                    let vs_ary = sc_base + 0xa00;
                    let fs_ary = sc_base + 0xa10;
                    *(vs_ary as *mut u64) = sc_base + 0x400;
                    *(fs_ary as *mut u64) = sc_base + 0x800;
                    let vs_id = gslot(plt_createshader, 0x8B31, 0, 0, 0, 0, 0).1 & 0xffff_ffff;
                    let _ = gslot(plt_shadersource, vs_id, 1, vs_ary, 0, 0, 0);
                    let _ = gslot(plt_compileshader, vs_id, 0, 0, 0, 0, 0);
                    let fs_id = gslot(plt_createshader, 0x8B30, 0, 0, 0, 0, 0).1 & 0xffff_ffff;
                    let _ = gslot(plt_shadersource, fs_id, 1, fs_ary, 0, 0, 0);
                    let _ = gslot(plt_compileshader, fs_id, 0, 0, 0, 0, 0);
                    let prog = gslot(plt_createprogram, 0, 0, 0, 0, 0, 0).1 & 0xffff_ffff;
                    let _ = gslot(plt_attachshader, prog, vs_id, 0, 0, 0, 0);
                    let _ = gslot(plt_attachshader, prog, fs_id, 0, 0, 0, 0);
                    let loc_name = sc_base + 0xd20;
                    std::ptr::copy_nonoverlapping(b"aPos\0".as_ptr(), loc_name as *mut u8, 5);
                    let _ = gslot(plt_bindattrib, prog, 0, loc_name, 0, 0, 0);
                    // A: slot15 = glProgramParameteri(prog, GL_PROGRAM_BINARY_RETRIEVABLE_HINT=0x8257, 1)
                    // BEFORE linking so Mesa will emit a retrievable binary.
                    let a15 = gslot(SLOT15, prog, 0x8257, 1, 0, 0, 0);
                    eprintln!("[elfjit:progbin] slot15 glProgramParameteri(retrievable hint) -> {a15:?} err={:#x}", geterr());
                    let _ = gslot(plt_linkprogram, prog, 0, 0, 0, 0, 0);
                    eprintln!("[elfjit:progbin] linked prog={prog:#x} err={:#x}", geterr());
                    // glGetProgramBinary(prog, 4096, &length, &format, &binary) through slot13.
                    let len_slot = sc_base + 0xe00;
                    let fmt_slot = sc_base + 0xe10;
                    let bin_slot = sc_base + 0xe40;
                    *(len_slot as *mut u32) = 0;
                    *(fmt_slot as *mut u32) = 0;
                    let a13 = gslot(SLOT13, prog, 4096, len_slot, fmt_slot, bin_slot, 0);
                    let len = *(len_slot as *const u32);
                    let fmt = *(fmt_slot as *const u32);
                    eprintln!("[elfjit:progbin] slot13 glGetProgramBinary -> {a13:?} length={len} format={fmt:#x} err={:#x}", geterr());
                    assert!(len > 0 && fmt != 0, "glGetProgramBinary through sealed slot13 must return a real binary");
                    // Re-upload through slot14: glProgramBinary(prog, fmt, bin, len).
                    let a14 = gslot(SLOT14, prog, fmt as u64, bin_slot, len as u64, 0, 0);
                    eprintln!("[elfjit:progbin] slot14 glProgramBinary(re-upload) -> {a14:?} err={:#x}", geterr());
                    // B: UBO bind through slot5. Gen a real buffer, fill 64B, bind to UBO index 0.
                    let buf_id_slot = sc_base + 0xf00;
                    let _ = gslot(plt_genbuffers, 1, buf_id_slot, 0, 0, 0, 0);
                    let ubo_id = *(buf_id_slot as *const u32) as u64;
                    let ubodata = sc_base + 0xf40;
                    for i in 0..16 {
                        *(ubodata as *mut u8).add(i) = (i as u8) << 4;
                    }
                    let _ = gslot(plt_bindbuffer, 0x8A11 /*GL_UNIFORM_BUFFER*/, ubo_id, 0, 0, 0, 0);
                    let _ = gslot(plt_buffdata, 0x8A11, 64, ubodata, 0x88E8 /*GL_DYNAMIC_DRAW*/, 0, 0);
                    let a5 = gslot(SLOT5, 0x8A11, 0, ubo_id, 0, 0, 0);
                    eprintln!("[elfjit:progbin] slot5 glBindBufferBase(UBO,0,{ubo_id}) -> {a5:?} err={:#x}", geterr());
                    // C: instanced draw through slot10. Slot10 currently seeds as glDrawArrays
                    // (the coherent path uses it for the array draw); RE-point it to the sealed
                    // glDrawArraysInstanced bridge slot, drive a count=0 no-op, then restore.
                    let slot10_addr = 0x106d3b2f0u64 + 8 * 10;
                    let saved_slot10 = unsafe { *(slot10_addr as *const u64) };
                    if let Some(inst) = arm64jit::resolver::resolve_gles_int(b"glDrawArraysInstanced\0") {
                        unsafe { *(slot10_addr as *mut u64) = inst };
                        let a10 = gslot(SLOT10, 0x0004 /*GL_TRIANGLES*/, 0, 0, 3, 0, 0);
                        eprintln!("[elfjit:progbin] slot10 glDrawArraysInstanced(TRIANGLES,0,0,3) -> {a10:?} err={:#x}", geterr());
                        unsafe { *(slot10_addr as *mut u64) = saved_slot10 };
                    } else {
                        eprintln!("[elfjit:progbin] glDrawArraysInstanced NOT resolvable (skipping C)");
                    }
                    eprintln!("[elfjit:progbin] GLES3 pipeline slot probe done (final err={:#x})", geterr());
                    } // unsafe
                }
            }
        });
    }

    match arm64jit::jit::jit_run(image, base, start_app, &mut s2 as *mut CpuState) {
            Err(e) => eprintln!("[elfjit] StartApp stopped: {e}"),
            Ok(r) => eprintln!("[elfjit] StartApp returned Ok({r:#x})"),
        }
        // Let any game-start workers run before exiting (or rather: keep the
        // process alive long enough for a real main loop to iterate/block).
        for _ in 0..4000 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if std::env::var_os("JIT_STATS").is_some() {
                let (c, h) = arm64jit::jit::block_cache_stats();
                // Compiles growing = StartApp is advancing through new init code;
                // flat compiles + rising hits = it is recycling cached hot blocks.
                eprintln!("[elfjit] stats: compiles={c} hits={h}");
            }
            // Periodic guest-thread state sampler (JIT_THREADS=1): while the
            // engine parks in the lifecycle-await, dump each registered guest
            // thread's live registers — its hostcall slot (pc), the guest call
            // site (lr = x30), and the wait-object args (x0..x2) — so the
            // boot wall is pinned to a precise guest function & release path.
            // JIT_FRAMEWORK_DUMP: read the framework-built globals the render
            // path and the task-deque maintenance forward-edges rely on, live
            // from this process (guest==host addressing, so a guest bss/heaplow
            // address is a valid host pointer). Tells us whether StartApp's
            // initialization actually POPULATED the render-init context
            // (0x1067d16f0 = [render-init+0x3a300] ldr x25,[x25,#222*8]) or the
            // deque maintenance dispatch globals before the main loop parks —
            // i.e. whether driving the real render-init after warm-up is viable.
            if std::env::var_os("JIT_FRAMEWORK_DUMP").is_some() {
                let dw = |a: u64| -> u64 {
                    if a >= 0x100000000 && a >> 56 == 0 && a & 7 == 0 {
                        unsafe { *(a as *const u64) }
                    } else {
                        0
                    }
                };
                let ctx = dw(0x1067d16f0);
                eprintln!(
                    "[elfjit:fw] render-ctx 0x1067d16f0={:#x} | deque-fwd 0x1068262e8={:#x} 0x106826300={:#x} 0x106826308={:#x} | task-v4 [0x106829ea8]={:#x} v0/2 [0x106826320]={:#x} | render-ctx+0 [*ctx]={:#x}",
                    ctx,
                    dw(0x1068262e8),
                    dw(0x106826300),
                    dw(0x106826308),
                    dw(0x106829ea8),
                    dw(0x106826320),
                    if ctx != 0 && ctx >> 56 == 0 { dw(ctx) } else { 0 },
                );
            }
            if std::env::var_os("JIT_THREADS").is_some() {
                let snaps = arm64jit::jit::snapshot_threads();
                let mut lines = format!("[elfjit] guest threads {}", snaps.len());
                for t in &snaps {
                    let at = arm64jit::resolver::name_of_call_addr(t.pc)
                        .unwrap_or_else(|| format!("{:#x}", t.pc));
                    lines.push_str(&format!(
                        "\n  host_tid={} guest_tid={} pc={at} lr={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x} x5={:#x} x19={:#x}[*={:#x}] x20={:#x} x21={:#x} x29={:#x} sp={:#x}",
                        t.host_tid, t.guest_tid, t.lr, t.x0, t.x1, t.x2, t.x3, t.x5, t.x19,
                        if t.x19 >= 0x100000000 && t.x19 >> 56 == 0 && t.x19 & 7 == 0 { unsafe { *(t.x19 as *const u64) } } else { 0 },
                        t.x20, t.x21, t.x29, t.sp
                    ));
                }
                eprintln!("{lines}");
            }
            if std::env::var_os("ELFJIT_EXIT_WHEN_IDLE").is_some()
                && arm64jit::jit::active_guest_threads() <= baseline
            {
                eprintln!("[elfjit] guest idle; exiting");
                break;
            }
        }
    }
    let (compiles, hits) = arm64jit::jit::block_cache_stats();
    if compiles > 0 || hits > 0 {
        eprintln!("[elfjit] block-cache: {compiles} compiles / {hits} hits");
    }
}
