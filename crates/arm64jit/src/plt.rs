//! Fold the import resolver + host shims into the loaded-image boot path.
//!
//! `bind_image_plt` walks a loaded ELF's `DT_JMPREL` JUMP_SLOT relocations and
//! patches each GOT slot to the matching host thunk, so translated Roblox code
//! can `blr` to libc/libm/host-shim functions in-process. This is the JIT
//! equivalent of the dynamic linker working through `libloader`'s in-memory
//! image: after this, every import the guest references resolves to a real host
//! x86-64 routine (or a benign graphics/audio/media fallback stub).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use libloader::elf::LoadedElf;

/// Build a combined guest symbol scope across a module chain: maps every *global
/// defined* symbol (`st_shndx != SHN_UNDEF`, `STB_GLOBAL`/`STB_WEAK` binding) of
/// each loaded module to its guest/runtime address (== host address under
/// `libloader`'s guest==host mapping). The main image's definitions win
/// (first-wins insertion in load order), so a dependency later in `els` only
/// fills a name no earlier module already defines — the ELF program-then-libs
/// interposition rule.
///
/// This lets one module's `adrp;ldr x,[GOT];blr x` import dispatch into another
/// module's *exported* function when `bind_image_plt` is given the scope for a
/// whole `DT_NEEDED` chain: a symbol the host resolver can't satisfy resolves to
/// the defining module's guest address, which the JIT then compiles from the
/// shared image slice.
#[allow(unused)]
pub fn build_export_scope(els: &[&LoadedElf]) -> HashMap<Vec<u8>, u64> {
    const DT_NULL: i64 = 0;
    const PT_DYNAMIC: u8 = 2;
    const DT_SYMTAB: i64 = 6;
    const DT_STRTAB: i64 = 5;
    const DT_HASH: i64 = 4;
    const SHN_UNDEF: u16 = 0;
    const STB_LOCAL: u8 = 0;

    #[inline]
    fn rd64(p: usize) -> u64 {
        unsafe { std::ptr::read_unaligned(p as *const u64) }
    }
    #[inline]
    fn rd32(p: usize) -> u32 {
        unsafe { std::ptr::read_unaligned(p as *const u32) }
    }
    #[inline]
    fn rd16(p: usize) -> u16 {
        unsafe { std::ptr::read_unaligned(p as *const u16) }
    }

    let mut scope: HashMap<Vec<u8>, u64> = HashMap::new();
    for el in els {
        let min_guest = el
            .segments
            .iter()
            .map(|s| s.guest_vaddr)
            .min()
            .unwrap_or(0);
        let host = |g: u64| -> usize { el.host_addr_of(g).unwrap_or(0) as usize };
        let ehdr = host(min_guest);
        if ehdr == 0 {
            continue;
        }
        let e_phoff = rd64(ehdr + 0x20) as usize;
        let e_phentsize = rd16(ehdr + 0x36) as usize;
        let e_phnum = rd16(ehdr + 0x38) as usize;

        let mut dyn_link = 0u64;
        for i in 0..e_phnum {
            let ph = ehdr + e_phoff + i * e_phentsize;
            if rd32(ph) == PT_DYNAMIC as u32 {
                dyn_link = rd64(ph + 0x10);
                break;
            }
        }
        if dyn_link == 0 {
            continue;
        }
        let dynp = host(el.guest_of(dyn_link));
        if dynp == 0 {
            continue;
        }
        let (mut symtab, mut strtab, mut hash) = (0u64, 0u64, 0u64);
        let mut i = 0usize;
        loop {
            let tag = rd64(dynp + i * 16) as i64;
            let val = rd64(dynp + i * 16 + 8);
            if tag == DT_NULL as i64 {
                break;
            }
            match tag {
                DT_SYMTAB => symtab = val,
                DT_STRTAB => strtab = val,
                DT_HASH => hash = val,
                _ => {}
            }
            i += 1;
            if i > 4096 {
                break;
            }
        }
        if symtab == 0 || strtab == 0 {
            continue;
        }
        let symtab_h = host(el.guest_of(symtab));
        let strtab_h = host(el.guest_of(strtab));

        // Upper bound of this module's guest==host mapped image (end of the
        // highest segment). GNU-hashed libs have no DT_HASH nchain, so the
        // scan must stop before reading past the module's actual data rather
        // than walk into unmapped memory.
        let lim = el
            .segments
            .iter()
            .map(|s| s.guest_vaddr.saturating_add(s.memsz))
            .max()
            .unwrap_or(0);

        // Number of symbol-table entries: from the DT_HASH nchain when present,
        // else a bounded scan (stop at an all-zero/unnamed undefined symbol).
        let nsyms: usize = if hash != 0 {
            let hash_h = host(el.guest_of(hash));
            if hash_h != 0 {
                rd32(hash_h + 4) as usize // nchain (offset +4)
            } else {
                0
            }
        } else {
            0
        };
        let scan_limit = if nsyms != 0 && nsyms < (1 << 20) {
            nsyms
        } else {
            (1 << 20)
        };

        for n in 1..scan_limit {
            let sym = symtab_h + n * 24;
            // Never read outside the module's mapped image (no DT_HASH → the
            // null-symbol sentinel can't be used; the null symbol IS index 0,
            // which we skip = start at n=1 — bound by the image end instead).
            if nsyms == 0 && sym + 24 > lim as usize {
                break;
            }
            let st_name = rd32(sym) as usize;
            let st_info = unsafe { *(sym as *const u8).add(4) };
            let st_shndx = rd16(sym + 6);
            let st_value = rd64(sym + 8);
            if st_shndx == SHN_UNDEF || (st_info >> 4) == STB_LOCAL {
                continue; // undefined (import) or local — not in the export scope
            }
            let mut name = Vec::new();
            {
                let mut p = strtab_h + st_name;
                let end = lim as usize;
                while p < end {
                    let c = unsafe { *(p as *const u8) };
                    if c == 0 {
                        break;
                    }
                    name.push(c);
                    p += 1;
                    if name.len() > 256 {
                        break;
                    }
                }
            }
            if name.is_empty() {
                continue;
            }
            let va = el.guest_of(st_value);
            if va != 0 {
                scope.entry(name).or_insert(va); // first (earliest-loaded) wins
            }
        }
    }
    scope
}

/// Resolve `name` against a cross-module scope, if one is provided.
fn scope_resolve(scope: Option<&HashMap<Vec<u8>, u64>>, name: &[u8]) -> Option<u64> {
    scope.and_then(|s| s.get(name)).copied()
}

/// Walk `DT_JMPREL` and bind every `R_AARCH64_JUMP_SLOT` GOT slot to a host
/// thunk guest address. Returns `(resolved, total_relocs)`.
///
/// Resolution order mirrors `resolveimports`: 1) native host symbol (libc/libm,
/// with an explicit `libm.so.6` fallback), 2) hand-written bionic shim
/// (`__errno`/`__strlen_chk`/`__android_log_print`/... ), 3) float-ABI bridge
/// (float64/float32), 4) catch-all graphics/audio/media fallback stub.
pub fn bind_image_plt(
    el: &LoadedElf,
    scope: Option<&HashMap<Vec<u8>, u64>>,
) -> (usize, usize) {
    const DT_NULL: i64 = 0;
    const PT_DYNAMIC: u8 = 2; // NOT 6 (PT_PHDR=6); 2 is the dynamic segment
    const DT_STRTAB: i64 = 5;
    const DT_SYMTAB: i64 = 6;
    const DT_JMPREL: i64 = 23;
    const DT_PLTRELSZ: i64 = 2;
    const R_AARCH64_JUMP_SLOT: u64 = 1026;

    // Ensure the hand-written host-side shims (bionic errno/strlens, the
    // __cxa_guard*/__cxa_atexit C++ static-init glue, Android asset/looper/
    // window stubs) are registered into the resolver before symbol scanning,
    // so an import that names one binds to the real host shim instead of
    // falling all the way to the NULL/0 graphics catch-all. register_* are
    // idempotent (register_named reuses an existing slot for a given name).
    crate::shims::register_shims();
    crate::shims::register_cxx_shims();
    // General-dynamic deps import `__tls_get_addr`; bind it to the host TLS
    // resolver so the JUMP_SLOT doesn't fall to the NULL/0 catch-all.
    ensure_tls_get_addr();

    let host = |g: u64| -> usize { el.host_addr_of(g).expect("guest not mapped") as usize };
    #[inline]
    fn rd64(p: usize) -> u64 {
        unsafe { std::ptr::read_unaligned(p as *const u64) }
    }
    #[inline]
    fn rd32(p: usize) -> u32 {
        unsafe { std::ptr::read_unaligned(p as *const u32) }
    }
    #[inline]
    fn wr64(p: usize, v: u64) {
        unsafe { std::ptr::write_unaligned(p as *mut u64, v) };
    }
    #[inline]
    fn rd16(p: usize) -> u16 {
        unsafe { std::ptr::read_unaligned(p as *const u16) }
    }

    // ELF header of the first (lowest) segment: file offset 0 maps there.
    let min_guest = el
        .segments
        .iter()
        .map(|s| s.guest_vaddr)
        .min()
        .expect("no segments");
    let ehdr = host(min_guest);
    let e_phoff = rd64(ehdr + 0x20) as usize;
    let e_phentsize = rd16(ehdr + 0x36) as usize;
    let e_phnum = rd16(ehdr + 0x38) as usize;

    let mut dyn_link = 0u64;
    for i in 0..e_phnum {
        let ph = ehdr + e_phoff + i * e_phentsize;
        if rd32(ph) == PT_DYNAMIC as u32 {
            dyn_link = rd64(ph + 0x10); // p_vaddr
            break;
        }
    }
    if dyn_link == 0 {
        // Static (no dynamic segment) or otherwise no imports: nothing to bind.
        return (0, 0);
    }

    let dynp = host(el.guest_of(dyn_link));
    let (mut jmprel, mut pltrelsz, mut symtab_ref, mut strtab_ref) = (0u64, 0u64, 0u64, 0u64);
    let mut i = 0usize;
    loop {
        let tag = rd64(dynp + i * 16) as i64;
        let val = rd64(dynp + i * 16 + 8);
        if tag == DT_NULL as i64 {
            break;
        }
        match tag {
            DT_JMPREL => jmprel = val,
            DT_PLTRELSZ => pltrelsz = val,
            DT_SYMTAB => symtab_ref = val,
            DT_STRTAB => strtab_ref = val,
            _ => {}
        }
        i += 1;
        if i > 4096 {
            break;
        }
    }
    if pltrelsz == 0 {
        // Dynamic segment present but no PLT/JUMP_SLOT relocations at all.
        // GLOB_DAT relocations may still exist in the main DT_RELA, so bind
        // them before bailing (a module with only exported-data globals has
        // zero PLT calls yet depends on the main GOT for correctness).
        let glob = bind_glob_dat(el, scope);
        eprintln!(
            "[plt] (no JUMP_SLOT) bound {} GLOB_DAT/ABS64, {} unresolved",
            glob.0, glob.1
        );
        return (0, 0);
    }

    let jmprel_h = host(el.guest_of(jmprel));
    let symtab_h = host(el.guest_of(symtab_ref));
    let strtab_h = host(el.guest_of(strtab_ref));

    let nsyms = (pltrelsz as usize) / 24;
    let (mut resolved, mut unresolved) = (0usize, 0usize);
    let mut pending: Vec<(Vec<u8>, u64)> = Vec::new(); // (name, guest r_offset)

    for n in 0..nsyms {
        let r = jmprel_h + n * 24;
        let r_offset = rd64(r);
        let r_info = rd64(r + 8);
        let stype = (r_info & 0xffff_ffff) as u32;
        if stype != R_AARCH64_JUMP_SLOT as u32 {
            continue;
        }
        let sym_idx = (r_info >> 32) as usize;
        let sym = symtab_h + sym_idx * 24;
        let st_name = rd32(sym) as usize;
        let mut name = Vec::new();
        {
            let mut p = strtab_h + st_name;
            for _ in 0..256 {
                let c = unsafe { *(p as *const u8) };
                if c == 0 {
                    break;
                }
                name.push(c);
                p += 1;
            }
        }
        // Self-import: if this relocation names a symbol the module itself
        // defines (st_shndx != SHN_UNDEF), the target is the module's own guest
        // address — NOT a host catch-all. A shared library that calls one of its
        // own exported functions goes through `@plt`; the dynamic symbol table
        // entry has st_shndx set and st_value = the definition's link address.
        // Without this, `init_value@plt` in real libroblox.so (or any -shared
        // module) binds to the NULL/0 graphics stub and the call diverts to a
        // host thunk instead of the real guest function. Bind it to the guest
        // address of the module's own definition.
        let st_shndx = rd16(sym + 6) as u32; // sym->st_shndx
        let st_value = rd64(sym + 8);
        if st_shndx != 0 {
            // SHN_UNDEF == 0; any real section index means local definition.
            wr64(host(el.guest_of(r_offset)), el.guest_of(st_value));
            resolved += 1;
            continue;
        }
        match (
            scope_resolve(scope, &name),
            crate::resolver::resolve(&name),
            crate::resolver::resolve_float(&name),
            crate::resolver::resolve_float32(&name),
            crate::resolver::resolve_egl(&name),
            crate::resolver::resolve_gles_int(&name),
            crate::resolver::resolve_gles_mixed(&name),
        ) {
            (Some(a), _, _, _, _, _, _)
            | (None, Some(a), _, _, _, _, _)
            | (None, None, Some(a), _, _, _, _)
            | (None, None, None, Some(a), _, _, _)
            | (None, None, None, None, Some(a), _, _)
            | (None, None, None, None, None, Some(a), _)
            | (None, None, None, None, None, None, Some(a)) => {
                wr64(host(el.guest_of(r_offset)), a);
                resolved += 1;
            }
            (None, None, None, None, None, None, None) => pending.push((name, r_offset)),
        }
    }

    if !pending.is_empty() {
        let names: Vec<&[u8]> = pending.iter().map(|(n, _)| n.as_slice()).collect();
        crate::shims::register_graphics_stubs(&names);
        for (name, r_offset) in &pending {
            if let Some(a) = crate::resolver::resolve(name) {
                wr64(host(el.guest_of(*r_offset)), a);
                resolved += 1;
            } else {
                unresolved += 1;
            }
        }
    }

    // __stack_chk_guard: the guest prologue (`adrp xN, …; ldr xN,[xN,#off]; ldr
    // …,[xN]; stur …,[x29,#-8]`) reads a *data* GOT slot for libc's canary. The
    // Android NDK build emits NO relocation for this slot (only JUMP_SLOTs — no
    // .rela.dyn / GLOB_DAT / RELATIVE), so it stays 0x0 and the guest null-faults
    // on its first `ldr x8,[x0]`. The QEMU path (jni_shim.c) fixed this by
    // writing a live canary address into the GOT; mirror it here.
    patch_stack_canary(el, &wr64);

    // Bind GLOB_DAT relocations in the main DT_RELA section. Jump-slot (PLT)
    // handling above only covers DT_JMPREL; exported-data / function-pointer
    // globals referenced from `-fPIC` module code go through the *main* GOT via
    // R_AARCH64_GLOB_DAT, and if left un-patched the guest `adrp;ldr x,[GOT]`
    // reads 0 and derefs/calls NULL. See bind_glob_dat for the exact semantics.
    let glob = bind_glob_dat(el, scope);
    eprintln!(
        "[plt] bound {resolved} JUMP_SLOT + {} GLOB_DAT/ABS64 ({} unresolved), {} unresolved",
        glob.0, unresolved, glob.1
    );

    (resolved, unresolved)
}

/// Bind `R_AARCH64_GLOB_DAT` (1025) relocations in the main `DT_RELA` section.
///
/// JUMP_SLOT (DT_JMPREL) covers PLT *function calls*; but `-fPIC` module code
/// that references an *exported* global (a data variable or a function pointer)
/// goes through the main GOT via GLOB_DAT: the compiler emits
/// `adrp x0,GOT; ldr x0,[x0,#off]` to fetch the symbol's *runtime address*,
/// then dereferences or calls through it. `R_AARCH64_GLOB_DAT` says
/// `*(r_offset) = S` (S = the symbol's load address). Left un-patched the guest
/// loads 0 and either calls address 0 or null-derefs.
///
/// The symbol's runtime value for *this* module = `el.guest_of(st_value)` (the
/// loader maps guest==host, so `ldr xN,[GOT]` then `[xN]`/`blr xN` resolves back
/// into the mapped image). For an *undefined/imported* symbol, the value is the
/// host address: for a data object use `dlsym` raw (guest==host addressable); for
/// a function/notype use the resolver's host-call thunk so a guest call to the
/// slot dispatches to the real host fn. Returns `(bound, unresolved)`.
fn bind_glob_dat(
    el: &LoadedElf,
    scope: Option<&HashMap<Vec<u8>, u64>>,
) -> (usize, usize) {
    const DT_NULL: i64 = 0;
    const PT_DYNAMIC: u8 = 2;
    const DT_RELA: i64 = 7;
    const DT_RELASZ: i64 = 8;
    const DT_SYMTAB: i64 = 6;
    const DT_STRTAB: i64 = 5;
    const DT_RELAENT: i64 = 9;
    // Android packed-relocation dynamic tags: `DT_ANDROID_RELA` /
    // `DT_ANDROID_RELASZ` carry the same GLOB_DAT/ABS64 relocations as a plain
    // `DT_RELA`, but APS2-packed, and a real libroblox.so ships ONLY the
    // packed `.rela.dyn` (no stock DT_RELA) — so without decoding it the
    // `__stack_chk_guard` data GOT slot (and any Android-packed global
    // import) stays 0 and the guest null-faults in its very first prologue.
    const DT_ANDROID_RELA: i64 = 0x6000_0011;
    const DT_ANDROID_RELASZ: i64 = 0x6000_0012;
    const R_AARCH64_GLOB_DAT: u64 = 1025;
    const R_AARCH64_ABS64: u64 = 257;
    const SHN_UNDEF: u16 = 0;
    // ELF symbol type bits (st_info & 0xf).
    const STT_OBJECT: u8 = 1;

    let host = |g: u64| -> usize { el.host_addr_of(g).expect("guest not mapped") as usize };
    #[inline]
    fn rd64(p: usize) -> u64 {
        unsafe { std::ptr::read_unaligned(p as *const u64) }
    }
    #[inline]
    fn rd32(p: usize) -> u32 {
        unsafe { std::ptr::read_unaligned(p as *const u32) }
    }
    #[inline]
    fn rd16(p: usize) -> u16 {
        unsafe { std::ptr::read_unaligned(p as *const u16) }
    }
    #[inline]
    fn wr64(p: usize, v: u64) {
        unsafe { std::ptr::write_unaligned(p as *mut u64, v) };
    }

    let min_guest = el
        .segments
        .iter()
        .map(|s| s.guest_vaddr)
        .min()
        .expect("no segments");
    let ehdr = host(min_guest);
    let e_phoff = rd64(ehdr + 0x20) as usize;
    let e_phentsize = rd16(ehdr + 0x36) as usize;
    let e_phnum = rd16(ehdr + 0x38) as usize;

    let mut dyn_link = 0u64;
    for i in 0..e_phnum {
        let ph = ehdr + e_phoff + i * e_phentsize;
        if rd32(ph) == PT_DYNAMIC as u32 {
            dyn_link = rd64(ph + 0x10); // p_vaddr
            break;
        }
    }
    if dyn_link == 0 {
        return (0, 0); // static ELF
    }

    let dynp = host(el.guest_of(dyn_link));
    let (mut rela, mut relasz, mut relaent, mut symtab_ref, mut strtab_ref) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut android_rela, mut android_relasz) = (0u64, 0u64);
    let mut i = 0usize;
    loop {
        let tag = rd64(dynp + i * 16) as i64;
        let val = rd64(dynp + i * 16 + 8);
        if tag == DT_NULL {
            break;
        }
        match tag {
            DT_RELA => rela = val,
            DT_RELASZ => relasz = val,
            DT_RELAENT => relaent = val,
            DT_SYMTAB => symtab_ref = val,
            DT_STRTAB => strtab_ref = val,
            DT_ANDROID_RELA => android_rela = val,
            DT_ANDROID_RELASZ => android_relasz = val,
            _ => {}
        }
        i += 1;
        if i > 4096 {
            break;
        }
    }
    if (rela == 0 || relasz == 0) && (android_rela == 0 || android_relasz == 0) || symtab_ref == 0
    {
        return (0, 0);
    }
    let entsz = if relaent != 0 { relaent as usize } else { 24 };

    // GLOB_DAT lives in the *main* reloc table, which also holds the RELATIVE
    // entries the loader already applied. We only process GLOB_DAT here.
    let rela_h = host(el.guest_of(rela));
    let symtab_h = host(el.guest_of(symtab_ref));
    let strtab_h = host(el.guest_of(strtab_ref));

    let mut resolve_entry = |r_offset: u64, r_info: u64, r_addend: i64| -> usize {
        // Returns 1 (bound) / 0 (unresolved) / -1 (not a GLOB_DAT/ABS64).
        let stype = (r_info & 0xffff_ffff) as u64;
        // R_AARCH64_GLOB_DAT (1025) and R_AARCH64_ABS64 (257) are both
        // "write the symbol's runtime address here". GLOB_DAT is the GOT-slot
        // form (addend 0), ABS64 the data-initializer form (addend = symbol
        // offset for a defined symbol). Both belong to the same resolver
        // family and are left unbound by the loader (which only applies
        // RELATIVE), so a guest global/function-pointer reads link-time
        // garbage. Handle both.
        if stype != R_AARCH64_GLOB_DAT && stype != R_AARCH64_ABS64 {
            return usize::MAX;
        }
        let sym_idx = (r_info >> 32) as usize;
        let sym = symtab_h + sym_idx * 24;
        let st_info = unsafe { *(sym as *const u8).add(4) };
        let st_shndx = rd16(sym + 6); // Elf64_Sym.st_shndx @ +6
        let st_value = rd64(sym + 8); // Elf64_Sym.st_value @ +8
        let st_name = rd32(sym) as usize;

        let slot_guest = el.guest_of(r_offset) as usize;
        if slot_guest == 0 {
            return 0; // unresolved
        }
        // Captured for the unresolved diagnostic below (the `name` Vec inside
        // the `value` if/else is scoped to that expression).
        let mut unresolved_name: Vec<u8> = Vec::new();

        let value: Option<u64> = if st_shndx != SHN_UNDEF {
            // Defined in this module: runtime address = guest addr of st_value
            // plus the addend (ABS64 has addend = offset to the symbol; GLOB_DAT
            // typically 0).
            let va = el.guest_of(st_value);
            if va == 0 { None } else { Some(va.wrapping_add(r_addend as u64)) }
        } else {
            // Imported symbol: resolve its host address.
            let mut name = Vec::new();
            {
                let mut p = strtab_h + st_name;
                for _ in 0..256 {
                    let c = unsafe { *(p as *const u8) };
                    if c == 0 {
                        break;
                    }
                    name.push(c);
                    p += 1;
                }
            }
            unresolved_name = name.clone();
            // A cross-module import: if a loaded dependency (or the main image)
            // defines this symbol, bind to its guest address so a guest deref /
            // blr on the GOT slot lands in the defining module, which the JIT
            // compiles from the shared image slice.
            if let Some(sa) = scope_resolve(scope, &name) {
                Some(sa.wrapping_add(r_addend as u64))
            } else if st_info & 0xf == STT_OBJECT {
                // Data object: dlsym gives the raw host (guest==host) addr.
                // `__stack_chk_guard` is read via `ldr x8,[x24]` where x24 =
                // the GOT slot holds the ADDRESS of the canary variable; glibc
                // doesn't export it to RTLD_DEFAULT, so fall back to a stable
                // static canary so the guest prologue's `ldr x8,[x24]` reads
                // a live value instead of a 0 slot deref.
                let fallback = if name == b"__stack_chk_guard" {
                    Some(stack_canary_addr())
                } else {
                    None
                };
                match fallback
                    .or_else(|| crate::resolver::resolve_android_data(&name))
                    .or_else(|| {
                        std::ffi::CString::new(name)
                            .ok()
                            .and_then(|c| {
                                let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) };
                                (!p.is_null()).then_some(p as u64)
                            })
                    }) {
                    Some(a) => Some(a.wrapping_add(r_addend as u64)),
                    None => None,
                }
            } else {
                // Function / notype import: host-call thunk (callable). Mirror
                // the JUMP_SLOT resolution chain (resolve -> float -> float32 ->
                // egl -> gles_int -> gles_mixed) so a `gl*`/`egl*` function-
                // pointer GLOB_DAT slot (e.g. glGetShaderInfoLog /
                // glGetProgramInfoLog in a function table) binds to real Mesa
                // instead of the benign NULL stub. Plain `resolve()` alone can't
                // see GLES names (libGLESv2 is RTLD_LOCAL and lazily loaded).
                [
                    crate::resolver::resolve(&name),
                    crate::resolver::resolve_float(&name),
                    crate::resolver::resolve_float32(&name),
                    crate::resolver::resolve_egl(&name),
                    crate::resolver::resolve_gles_int(&name),
                    crate::resolver::resolve_gles_mixed(&name),
                ]
                .into_iter()
                .flatten()
                .next()
                .map(|a| a.wrapping_add(r_addend as u64))
            }
        };

        match value {
            Some(v) => {
                wr64(slot_guest, v);
                1 // bound
            }
            None => {
                if std::env::var_os("JIT_TRACE").is_some() {
                    eprintln!(
                        "[plt:glob_dat] unresolved {:#x} sym={} shndx={}",
                        r_offset,
                        String::from_utf8_lossy(&unresolved_name),
                        st_shndx
                    );
                }
                // Leave data-object / canary-less slots as-is (0 is fine for a
                // guest that only reads them), BUT a *function* GLOB_DAT slot
                // that we can't resolve must not be left holding its original
                // stale data (`.dynstr` symbol-name pointer or garbage the
                // prior relocation wrote): the guest would `blr` through it and
                // jump into the symbol table. Bind it to a benign host-call
                // stub (zero-return, or handle-return for handle-like names) so
                // an indirect call dispatches to real host code instead of a
                // symbol string. Re-resolve to get the stub's registered slot.
                let is_func = st_shndx == SHN_UNDEF && st_info & 0xf != STT_OBJECT;
                if is_func {
                    let stub = crate::shims::register_fallback(&unresolved_name);
                    wr64(slot_guest, stub);
                    1 // bound to a safe stub
                } else {
                    0 // unresolved
                }
            }
        }
    };

    let (mut bound, mut unresolved) = (0usize, 0usize);

    // Pass 1: the stock DT_RELA table (if present).
    if rela != 0 && relasz != 0 {
        let n = (relasz as usize) / entsz;
        for k in 0..n {
            let r = rela_h + k * entsz;
            let r_offset = rd64(r);
            let r_info = rd64(r + 8);
            let r_addend = rd64(r + 16) as i64; // Elf64_Rela.r_addend @ +16
            match resolve_entry(r_offset, r_info, r_addend) {
                usize::MAX => {}
                1 => bound += 1,
                _ => unresolved += 1,
            }
        }
    }

    // Pass 2: the Android APS2-packed `.rela.dyn` (DT_ANDROID_RELA), which a
    // real libroblox.so ships INSTEAD of a stock DT_RELA. The loader's
    // relative-apply (elf.rs) already decodes it for RELATIVE entries via
    // `decode_aps2`; we re-decode here so the GLOB_DAT/ABS64 entries the
    // loader skipped (it only applies RELATIVE) get bound — exactly what
    // `__stack_chk_guard` needs on the boot path.
    if android_rela != 0 && android_relasz != 0 && android_relasz <= 128 * 1024 * 1024 {
        let srch = host(el.guest_of(android_rela));
        let stream =
            unsafe { std::slice::from_raw_parts(srch as *const u8, android_relasz as usize) };
        if let Ok(rels) = libloader::android_relocs::decode_aps2(stream) {
            for rel in &rels {
                match resolve_entry(rel.r_offset, rel.r_info, rel.r_addend) {
                    usize::MAX => {}
                    1 => bound += 1,
                    _ => unresolved += 1,
                }
            }
        }
    }
    (bound, unresolved)
}

/// Return the guest-visible host address of a stable `__stack_chk_guard`
/// canary: libc's real one when resolvable, else a process-static canary
/// pattern. The returned value is the *address* whose bytes are the canary
/// (the guest does `ldr x8,[xN]` to read the canary via the GOT slot, which
/// holds this address).
fn stack_canary_addr() -> u64 {
    static CANARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let cur = CANARY.load(std::sync::atomic::Ordering::Relaxed);
    if cur != 0 {
        return cur;
    }
    let libc_guard =
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, b"__stack_chk_guard\0".as_ptr() as *const _) };
    let addr = if !libc_guard.is_null() {
        libc_guard as u64
    } else {
        let canary = 0x2f_2a_1a_0a_0e_0f_10_11u64;
        CANARY.store(canary, std::sync::atomic::Ordering::Relaxed);
        &CANARY as *const _ as u64
    };
    CANARY.store(addr, std::sync::atomic::Ordering::Relaxed);
    addr
}

/// Write a live canary pointer into the guest `__stack_chk_guard` GOT slot.
///
/// The slot holds the *address* of the canary variable; the guest then does
/// `ldr x8,[xN]` to read the canary bytes, storing them on its stack to check
/// against a later `ldr`. We point it at libc's real `__stack_chk_guard` so the
/// value matches what `__stack_chk_fail` expects, falling back to a stable
/// static canary if the host symbol can't be resolved. The slot address is a
/// per-build constant (see caller notes); `LoadedElf` guards sanity.
fn patch_stack_canary(el: &LoadedElf, wr64: &impl Fn(usize, u64)) {
    // Guest vaddr of the `__stack_chk_guard` data GOT slot for the reference
    // build. Verified (Session xx): JNI_OnLoad's prologue
    //   adrp x24, 0x631a000 ; ldr x24,[x24,#2608] ; ldr x8,[x24] ; stur x8,[x29,#-8]
    // reads this slot, which is 0x0 in the file (no relocation). Note objdump
    // prints `#2608` in DECIMAL = 0xa30, so the slot is 0x631a000 + 0xa30.
    // For PIE the loaded guest address of a link-time slot is `guest_of(link)`;
    // because the runtime maps guest==host (contig), that value doubles as the
    // host addr.
    const CANARY_GOT_LINK: u64 = 0x631a000 + 0xa30; // = 0x631aa30
    // Only patch if the slot actually lands inside this module's mapped image.
    // For a small test .so linked at a low origin this link-time address maps
    // far outside the module's PT_LOADs (guest==host), so dereferencing it
    // would SIGSEGV; for the real libroblox.so it resolves to the real GOT slot.
    let Some(host_addr) = el.host_addr_of(el.guest_of(CANARY_GOT_LINK)) else {
        eprintln!("[plt] canary slot {CANARY_GOT_LINK:#x} not in this module's image; skipping");
        return;
    };
    let host_addr = host_addr as usize;
    let cur = unsafe { std::ptr::read_unaligned(host_addr as *const u64) };

    static CANARY: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    let canary_val = CANARY.load(std::sync::atomic::Ordering::Relaxed);
    let canary_addr = if canary_val != 0 {
        &canary_val as *const u64 as u64
    } else {
        // Prefer libc's real canary so __stack_chk_fail and our value agree.
        let libc_guard =
            unsafe { libc::dlsym(libc::RTLD_DEFAULT, b"__stack_chk_guard\0".as_ptr() as *const _) };
        let addr = if !libc_guard.is_null() {
            libc_guard as u64
        } else {
            // Static fallback: a stable non-zero canary byte pattern.
            let canary = 0x2f_2a_1a_0a_0e_0f_10_11u64;
            // If libc guard exists but reads 0, seed it too (mirror jni_shim).
            if !libc_guard.is_null() {
                unsafe { std::ptr::write_unaligned(libc_guard as *mut u64, canary) };
            }
            CANARY.store(canary, std::sync::atomic::Ordering::Relaxed);
            &CANARY as *const _ as u64
        };
        CANARY.store(addr, std::sync::atomic::Ordering::Relaxed);
        addr
    };

    if cur & !0x0000_0000_ffff_ffffu64 == 0 {
        // Only write when the slot doesn't already reference a real page, so we
        // never clobber a legitimately-bound canary or an unrelated GOT slot.
        wr64(host_addr, canary_addr);
        eprintln!("[plt] patched __stack_chk_guard GOT {:#x} ({:#x}) -> {:#x}", CANARY_GOT_LINK, cur, canary_addr);
    }
}

/// Host resolver installed in a TLSDESC descriptor: the guest does
/// `ldr x1, [desc]; add x0, <desc>; blr x1` then `mrs tpidr_el0; add x0, tp, x0`.
/// `a0` is the descriptor's (guest == host, since `libloader` maps guest==host)
/// address; the loader pre-writes the symbol's TP-relative offset into
/// descriptor[1] (at +8). Returning it makes `[TP + ret]` land on the variable.
extern "C" fn tlsdesc_resolver(
    a0: u64,
    _a1: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
    _a7: u64,
) -> u64 {
    unsafe { std::ptr::read_unaligned((a0 + 8) as *const u64) }
}

/// Register the TLSDESC resolver host call exactly once; return its guest
/// (host-thunk) address, written into every TLSDESC descriptor's slot[0].
fn tlsdesc_resolver_addr() -> u64 {
    static RES: OnceLock<u64> = OnceLock::new();
    *RES.get_or_init(|| crate::jit::register_host_call_auto(tlsdesc_resolver))
}

/// Process-wide TLS runtime state the classic general-dynamic
/// `__tls_get_addr(&tls_index{module, offset})` host call needs: the thread
/// pointer TP and each chain module's TP-relative block offset. Single-threaded
/// JIT, set once by `set_chain_tls` before `jit_run`.
static TLS_CHAIN: Mutex<Option<(u64, Vec<u64>)>> = Mutex::new(None);

/// Record the per-thread TLS base (`TP`, the value `mrs tpidr_el0` yields) and
/// each chain module's TP-relative TLS-block offset so the `__tls_get_addr`
/// host call can resolve `{module, offset}` → concrete guest address.
pub fn set_chain_tls(tp: u64, offsets: Vec<u64>) {
    *TLS_CHAIN.lock().unwrap() = Some((tp, offsets));
}

/// General-dynamic `__tls_get_addr` host resolver: `a0` = guest address of the
/// GOT `tls_index` struct `{ u64 module_id, u64 offset }`. Returns `TP +
/// offsets[module] + offset`, the address of the TLS variable (== what the
/// guest then loads/stores). Only reachable when a dependency was built with
/// `-mtls-dialect=trad -ftls-model=global-dynamic` (GCC 13+ defaults to TLSDESC,
/// which needs no `__tls_get_addr`).
extern "C" fn host_tls_get_addr(
    a0: u64,
    _a1: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
    _a7: u64,
) -> u64 {
    // The caller owns the current host thread == current guest thread; its
    // TLS TP is published by `jit_run` (set_current_guest_tp). This resolves
    // {module, offset} against the CALLING thread's OWN TLS blocks — correct
    // for spawned clone children that each have their own TP. Fall back to the
    // main-thread TP if the publish didn't happen (older direct call).
    let tp = crate::jit::current_guest_tp();
    let tp = if tp != 0 { tp } else { main_tp() };
    tls_get_addr_from_tp(a0, tp)
}

/// Main-thread TP cached at `set_chain_tls` (falls back to it for the plain
/// call path, which only the main thread uses).
fn main_tp() -> u64 {
    TLS_CHAIN.lock().unwrap().as_ref().map(|g| g.0).unwrap_or(0)
}

fn tls_get_addr_from_tp(a0: u64, tp: u64) -> u64 {
    // `tls_index` is 16 bytes of GOT: word[0] = module id, word[1] = offset
    // within that module's TLS block (both link-time; `bind_chain_tls` wrote
    // the module id and the DTPREL slot). Guest == host under libloader.
    let module = unsafe { std::ptr::read_unaligned(a0 as *const u64) };
    let offset = unsafe { std::ptr::read_unaligned((a0 + 8) as *const u64) };
    let guard = TLS_CHAIN.lock().unwrap();
    let offsets = guard.as_ref().map(|g| &g.1).cloned().unwrap_or_default();
    let block = offsets.get(module as usize).copied().unwrap_or(0);
    tp + block + offset
}

/// Register the `__tls_get_addr` host call by name exactly once so
/// `bind_image_plt` binds a dependency's `__tls_get_addr@plt` JUMP_SLOT to it
/// (otherwise it falls to the NULL/0 catch-all and the guest calls garbage).
fn ensure_tls_get_addr() {
    static REG: OnceLock<u64> = OnceLock::new();
    REG.get_or_init(|| {
        let addr = crate::jit::register_host_call_auto(host_tls_get_addr);
        crate::resolver::register_named(b"__tls_get_addr", host_tls_get_addr);
        addr
    });
}

/// Bind AArch64 TLS GOT relocations for a whole `DT_NEEDED` module chain.
///
/// Modern GCC emits one of two models for `__thread` in a `-shared -fPIC`
/// library (and for its own local accesses), both routed through the GOT:
///
///  - **initial-exec** — `mrs xN, tpidr_el0; adrp xN; ldr xN, [xN, #off];
///    add xN, xN, <tpidr>` with an `R_AARCH64_TLS_TPREL64` (1030) GOT slot
///    holding the symbol's TP-relative offset;
///
///  - **TLSDESC** (GCC 13+ default, even for the "global-dynamic" model) — a
///    16-byte descriptor `{resolver_fn, value}` at the GOT slot; the access is
///    `ldr x1, [desc]; add x0, <desc>; blr x1; mrs tpidr_el0; add x0, tp, x0`,
///    i.e. it calls the resolver which returns `value` = the TP-relative offset.
///
/// Both need the module's *static-TLS block offset* in the per-thread image —
/// the value `libloader::deps::setup_chain_tls` seeds and returns as
/// `offsets[i]` (TP-relative, in load order). Before binding these the GOT held
/// uninitialized bytes, so any `__thread` in a dependency read garbage/NULL.
///
/// `els[i]` must be the chain's `i`-th module and `offsets[i]` its TLS block
/// offset. Returns the number of TLS relocation slots bound.
#[allow(unused)]
pub fn bind_chain_tls(els: &[&LoadedElf], offsets: &[u64]) -> usize {
    const DT_NULL: i64 = 0;
    const PT_DYNAMIC: u8 = 2; // NOT 6 (PT_PHDR)
    const DT_RELA: i64 = 7;
    const DT_RELASZ: i64 = 8;
    const DT_RELAENT: i64 = 9;
    const DT_SYMTAB: i64 = 6;
    const DT_JMPREL: i64 = 23;
    const DT_PLTRELSZ: i64 = 2;
    const R_AARCH64_TLS_TPREL64: u64 = 1030;
    const R_AARCH64_TLSDESC: u64 = 1031;
    const R_AARCH64_TLS_DTPMOD64: u64 = 1028;
    const R_AARCH64_TLS_DTPREL64: u64 = 1029;

    let resolver_addr = tlsdesc_resolver_addr();
    ensure_tls_get_addr();
    let mut bound = 0usize;

    for (mi, (el, &off)) in els.iter().zip(offsets).enumerate() {
        let host = |g: u64| -> usize { el.host_addr_of(g).expect("guest not mapped") as usize };
        #[inline]
        fn rd64(p: usize) -> u64 {
            unsafe { std::ptr::read_unaligned(p as *const u64) }
        }
        #[inline]
        fn rd32(p: usize) -> u32 {
            unsafe { std::ptr::read_unaligned(p as *const u32) }
        }
        #[inline]
        fn rd16(p: usize) -> u16 {
            unsafe { std::ptr::read_unaligned(p as *const u16) }
        }
        #[inline]
        fn wr64(p: usize, v: u64) {
            unsafe { std::ptr::write_unaligned(p as *mut u64, v) };
        }

        let min_guest = el
            .segments
            .iter()
            .map(|s| s.guest_vaddr)
            .min()
            .expect("no segments");
        let ehdr = host(min_guest);
        let e_phoff = rd64(ehdr + 0x20) as usize;
        let e_phentsize = rd16(ehdr + 0x36) as usize;
        let e_phnum = rd16(ehdr + 0x38) as usize;

        let mut dyn_link = 0u64;
        for i in 0..e_phnum {
            let ph = ehdr + e_phoff + i * e_phentsize;
            if rd32(ph) == PT_DYNAMIC as u32 {
                dyn_link = rd64(ph + 0x10); // p_vaddr
                break;
            }
        }
        if dyn_link == 0 {
            continue; // static ELF: no GOT relocations to bind
        }
        let dynp = host(el.guest_of(dyn_link));
        let (mut rela, mut relasz, mut relaent, mut symtab_ref) = (0u64, 0u64, 0u64, 0u64);
        let (mut jmprel, mut pltrelsz) = (0u64, 0u64);
        let mut i = 0usize;
        loop {
            let tag = rd64(dynp + i * 16) as i64;
            let val = rd64(dynp + i * 16 + 8);
            if tag == DT_NULL {
                break;
            }
            match tag {
                DT_RELA => rela = val,
                DT_RELASZ => relasz = val,
                DT_RELAENT => relaent = val,
                DT_SYMTAB => symtab_ref = val,
                DT_JMPREL => jmprel = val,
                DT_PLTRELSZ => pltrelsz = val,
                _ => {}
            }
            i += 1;
            if i > 4096 {
                break;
            }
        }
        let entsz = if relaent != 0 { relaent as usize } else { 24 };
        // TLS GOT relocations live in either table: initial-exec
        // `R_AARCH64_TLS_TPREL64` is emitted into `.rela.dyn` (DT_RELA), while
        // GCC's TLSDESC relocations are placed into `.rela.plt` (DT_JMPREL),
        // alongside JUMP_SLOTs. Iterate both.
        let mut tables: Vec<(u64 /*guest vaddr*/, u64 /*bytes*/)> = Vec::new();
        if rela != 0 && relasz != 0 {
            tables.push((rela, relasz));
        }
        if jmprel != 0 && pltrelsz != 0 {
            tables.push((jmprel, pltrelsz));
        }
        let symtab_h = if symtab_ref != 0 {
            host(el.guest_of(symtab_ref))
        } else {
            0
        };

        for (tbl, tblsz) in tables {
            let tbl_h = host(el.guest_of(tbl));
            let n = (tblsz as usize) / entsz;
            for k in 0..n {
                let r = tbl_h + k * entsz;
                let r_offset = rd64(r);
                let r_info = rd64(r + 8);
                let r_addend = rd64(r + 16);
                let stype = (r_info & 0xffff_ffff) as u64;
                if stype != R_AARCH64_TLS_TPREL64
                    && stype != R_AARCH64_TLSDESC
                    && stype != R_AARCH64_TLS_DTPMOD64
                    && stype != R_AARCH64_TLS_DTPREL64
                {
                    continue;
                }
                // TP-relative offset = module block offset + symbol's offset within
                // its block (`st_value`, baked by the linker relative to the block)
                // + the reloc addend. For a module's own `__thread` (the common
                // case) that is exactly where `setup_chain_tls` laid the block.
                let sym_idx = (r_info >> 32) as usize;
                let st_value = if symtab_h != 0 {
                    rd64(symtab_h + sym_idx * 24 + 8)
                } else {
                    0
                };
                let sym_off = st_value.wrapping_add(r_addend as u64);
                let tprel = off.wrapping_add(sym_off);
                let slot = host(el.guest_of(r_offset));
                match stype {
                    // initial-exec: GOT slot = the symbol's TP-relative offset.
                    R_AARCH64_TLS_TPREL64 => wr64(slot, tprel),
                    // TLSDESC: 16-byte descriptor {resolver, tprel} at the slot.
                    R_AARCH64_TLSDESC => {
                        wr64(slot, resolver_addr);
                        wr64(slot + 8, tprel);
                    }
                    // general-dynamic `tls_index` word[0] = defining module's
                    // chain index (used by __tls_get_addr to pick the block).
                    R_AARCH64_TLS_DTPMOD64 => wr64(slot, mi as u64),
                    // general-dynamic `tls_index` word[1] = offset within that
                    // module's block (__tls_get_addr adds it to the block base).
                    R_AARCH64_TLS_DTPREL64 => wr64(slot, sym_off),
                    _ => unreachable!(),
                }
                bound += 1;
            }
        }
    }
    bound
}

#[cfg(test)]
mod tests {
    use super::*;

    // Guarded against the real Roblox binary so the suite stays hermetic when
    // the asset isn't cached; when present this proves the boot-path binder
    // resolves the full PLT JUMP_SLOT set you'd otherwise hit at runtime.
    #[test]
    fn bind_image_plt_real_roblox_binds_all() {
        let candidates = [
            "/home/code-agent/.cache/open-sober/libs/libroblox.so",
            "/home/hermes-worker/.cache/open-sober/robbox/libroblox.so",
        ];
        let Some(path) = candidates.iter().find(|p| std::path::Path::new(p).exists()) else {
            eprintln!("skipping: no cached libroblox.so");
            return;
        };
        let el = unsafe { libloader::elf::load_elf_image(std::path::Path::new(path)) }
            .expect("load_elf_image");
        let (bound, unbound) = bind_image_plt(&el, None);
        eprintln!("bound {bound}, unbound {unbound}");
        assert!(bound >= 500, "expected most of 537 JUMP_SLOT imports bound, got {bound}");
        assert_eq!(unbound, 0, "every import should bind via resolve/stub");
    }

    /// The GLOB_DAT *function* slot path must resolve GLES function-table names
    /// (glGetShaderInfoLog / glGetProgramInfoLog) through the GLES resolver to a
    /// real host-thunk slot, not the benign NULL stub. These were previously
    /// unbound (plain dlsym can't see RTLD_LOCAL lazily-loaded libGLESv2), which
    /// would make a real shader-compile info-log query return garbage. Hermetic:
    /// exercises the exact chain bind_glob_dat's function branch now uses.
    #[test]
    fn glob_dat_function_chain_resolves_gles_names_to_real_slots() {
        for name in ["glGetShaderInfoLog", "glGetProgramInfoLog", "glGetString", "glCompileShader"] {
            // plt.rs passes NUL-FREE names to the resolver chain (the .dynstr
            // scan breaks at the NUL and never pushes it); the GLOB_DAT function
            // branch follows the same convention. Pass the NUL-free bytes.
            let nafree = name.as_bytes();
            let slot = [
                crate::resolver::resolve(nafree),
                crate::resolver::resolve_egl(nafree),
                crate::resolver::resolve_gles_int(nafree),
                crate::resolver::resolve_gles_mixed(nafree),
            ]
            .into_iter()
            .flatten()
            .next();
            assert!(
                slot.is_some_and(|s| s >= crate::jit::HOST_THUNK_BASE),
                "{name} GLOB_DAT function slot must resolve to a real host-thunk slot, got {slot:?}"
            );
        }
    }
}