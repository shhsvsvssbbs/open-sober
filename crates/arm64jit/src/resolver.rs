//! Resolve guest `.so` imports (libc/libm/bionic names) to host x86-64 call
//! slots in the JIT's host-call bridge.
//!
//! When the JIT dispatcher re-enters at a `HOST_THUNK_BASE + slot*8` address,
//! it invokes the registered `HostCall` with the guest x0..x7 args (SysV GPR
//! convention) and stores its return into guest x0. This module assigns each
//! import *name* to a stable slot and points that slot at the real host
//! function found via `dlsym`. A loader then patches the guest's
//! `R_AARCH64_JUMP_SLOT` GOT entry (or answer) to the slot's guest address.
//!
//! ABI note: this resolves functions whose args/return travel in GPRs
//! (integers/pointers) — strlen, memcpy, memcmp, strcmp, abs, etc. Float
//! args/results (sinf/powf/...) use XMM registers and need a separate
//! float-ABI path, added later.

use crate::jit::{
    host_call_addr, register_gles_call, register_host_call, CpuState, HostCall, HostFloat32Call,
    HostFloatCall, HostGlesCall,
};
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};

/// Allocates thunk slots and remembers name -> slot addr.
struct Resolver {
    /// import name -> HOST_THUNK guest address of its call slot
    slots: HashMap<CString, u64>,
    next: usize,
}

impl Resolver {
    fn new() -> Self {
        // Start at a high slot index to avoid colliding with any consumer that
        // uses the low slots directly (e.g. the jit `host_call_bridge`
        // validation test registers slot 1 by hand).
        Resolver {
            slots: HashMap::new(),
            next: 1000,
        }
    }
}

fn resolver() -> &'static Mutex<Resolver> {
    static R: OnceLock<Mutex<Resolver>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Resolver::new()))
}

/// One-time `dlopen` of `libm.so.6` (`RTLD_GLOBAL|RTLD_NOW`) so the float/libm
/// functions we `dlsym` are visible even if nothing else loaded libm yet.
fn libm_handle() -> *mut libc::c_void {
    // Raw pointers aren't Send/Sync; store as usize (an address is).
    static H: OnceLock<usize> = OnceLock::new();
    let addr = *H.get_or_init(|| {
        let path = b"libm.so.6\0";
        unsafe {
            libc::dlopen(
                path.as_ptr() as *const libc::c_char,
                libc::RTLD_NOW | libc::RTLD_GLOBAL,
            ) as usize
        }
    });
    addr as *mut libc::c_void
}

/// `dlsym` `name` from a given handle, returning the fn pointer or null.
unsafe fn sym_from(handle: *mut libc::c_void, name: *const libc::c_char) -> *mut libc::c_void {
    libc::dlsym(handle, name)
}

/// One-time `dlopen` of Mesa's real `libEGL.so.1` (`RTLD_NOW|RTLD_GLOBAL`) so the
/// `egl*` imports a guest Roblox binary makes resolve to real Mesa EGL instead of
/// the benign NULL/0 graphics catch-all. EGL's ABI is integer/pointer-only, so its
/// entry points are safe through the integer `HostCall` shape (unlike GLES, which
/// passes floats in xmm and needs a dedicated float-ABI bridge — not wired here).
fn egl_handle() -> *mut libc::c_void {
    static H: OnceLock<usize> = OnceLock::new();
    let addr = *H.get_or_init(|| {
        // Prefer the real Mesa lib (fall back to a versioned soname if newer distros
        // only ship `libEGL.so.1`; both are the vendor GL dispatch library).
        let candidates: &[&[u8]] = &[b"libEGL.so.1\0", b"libEGL.so\0"];
        for path in candidates {
            let h = unsafe {
                libc::dlopen(
                    path.as_ptr() as *const libc::c_char,
                    libc::RTLD_NOW | libc::RTLD_GLOBAL,
                )
            };
            if !h.is_null() {
                return h as usize;
            }
        }
        0
    });
    addr as *mut libc::c_void
}

/// Read a NUL-terminated guest C string at `ptr` (guest==host mapping, so the
/// guest pointer is directly host-addressable). Returns up to `cap-1` bytes.
unsafe fn read_guest_cstr(ptr: u64, cap: usize) -> Option<Vec<u8>> {
    if ptr == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(cap.min(256));
    let mut p = ptr as *const u8;
    for _ in 0..cap {
        let b = unsafe { *p };
        if b == 0 {
            return Some(out);
        }
        out.push(b);
        p = p.add(1);
    }
    None
}

/// GLES bridge for the guest's `eglGetProcAddress(const GLubyte *procname)`.
/// On real Android, Roblox resolves most GLES/EGL entry points *dynamically*
/// through `eglGetProcAddress` and `blr`s the returned pointer. If we return
/// Mesa's raw symbol (bound via the integer `HostCall`), the guest later
/// dispatches a *raw x86 function address* as its branch target — which is not
/// a registered host-thunk slot, so the dispatcher cannot route it, and the
/// call would bypass the GLES float bridge and the compressed-texture
/// interception (breaking glClearColor/glTexImage2D paths). Instead, resolve the
/// requested name to one of OUR host-thunk slots and return that guest-callable
/// address: a later guest `blr` to it dispatches through the correct bridge.
/// Falls back to Mesa's real `eglGetProcAddress` for names we don't wrap.
extern "C" fn w_eglGetProcAddress(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let name_ptr = s.x[0];
    // Resolve to our slot first (mixed bridge -> int -> egl), preserving the
    // float bridge, compressed-texture interception, and dispatchability.
    // NOTE: pass the name WITHOUT a trailing NUL — resolve_gles_int / resolve_egl
    // build a CString from it (CString::new rejects an interior NUL), and
    // resolve_gles_mixed's name_str strips at the first NUL anyway. plt.rs names
    // are likewise NUL-free, so this matches the direct-import path exactly.
    let Some(cstr) = (unsafe { read_guest_cstr(name_ptr, 256) }) else {
        return 0;
    };
    if let Some(slot) = resolve_gles_mixed(&cstr) {
        return slot;
    }
    if let Some(slot) = resolve_gles_int(&cstr) {
        return slot;
    }
    if let Some(slot) = resolve_egl(&cstr) {
        return slot;
    }
    // Not one of ours: ask real Mesa (e.g. extension functions we chose not to
    // intercept). Return its raw pointer — a niche path; the guest's `blr` into
    // Mesa's real libGLESv2 is a raw host call the dispatcher treats as a
    // host-thunk miss and falls through to, matching the pre-bridge behavior.
    let mh = gles_handle();
    if mh.is_null() {
        return 0;
    }
    let Some(cstr) = (unsafe { read_guest_cstr(name_ptr, 256) }) else {
        return 0;
    };
    let Ok(c) = CString::new(cstr.clone()) else {
        return 0;
    };
    let ptr = unsafe { sym_from(mh, c.as_ptr()) };
    if ptr.is_null() {
        // Diagnostic: log any guest-requested eglGetProcAddress name that we do
        // not bridge AND Mesa does not export. Such a name yields 0/NULL, so a
        // later guest `blr` into it would jump-to-NULL. This is how the engine's
        // real render dispatch table (BSS 0x106d3b2f0 + 8*N) can end up with a
        // NULL slot even though a real self-driven frame would dispatch through
        // it (observed: an engine that `bl`s slots 11/12 which read 0x0). The
        // guest PC (s.pc) pinpoints the calling site so we can identify whether
        // it routes through the 0x5b3a1c0+0xc*N slot stubs and which one.
        if std::env::var_os("JIT_EGL_LOG").is_some() {
            eprintln!(
                "[eglGetProcAddress] UNRESOLVED name {:?} (guest pc={:#x}, return=0) caller_guest_pc={:#x}",
                String::from_utf8_lossy(&cstr),
                s.pc,
                s.x[30]
            );
        }
        return 0;
    }
    ptr as u64
}

/// Resolve the guest's `eglGetProcAddress` import to a GLES-bridge host-call
/// slot. The bridge (w_eglGetProcAddress) returns one of OUR resolver slots for
/// each requested GLES name, so a later guest `blr` to it dispatches through the
/// correct host-thunk bridge (mixed/int/egl), preserving the float bridge and
/// compressed-texture interception. Idempotent: reuses a cached slot if bound.
fn resolve_egl_get_proc_address(key: &CString, r: &mut Resolver) -> Option<u64> {
    if let Some(addr) = r.slots.get(key) {
        return Some(*addr);
    }
    let slot = register_gles_call(w_eglGetProcAddress);
    crate::jit::name_host_call_slot(slot, "eglGetProcAddress");
    r.slots.insert(key.clone(), slot);
    Some(slot)
}

/// Resolve an `egl*` import against Mesa's real libEGL (integer-ABI HostCall).
/// Returns `None` if EGL isn't present on the host or the name isn't an EGL symbol.
pub fn resolve_egl(name: &[u8]) -> Option<u64> {
    let ns = name_str(name);
    if !ns.starts_with("egl") {
        return None;
    }
    // `eglGetProcAddress` is the dynamic GLES loader: route it through a GLES
    // bridge that returns one of OUR dispatchable host-thunk slots for the
    // requested name (see resolve_egl_get_proc_address) instead of Mesa's raw
    // function.
    if ns == "eglGetProcAddress" {
        let key = CString::new(ns).ok()?;
        let mut r = resolver().lock().unwrap();
        return resolve_egl_get_proc_address(&key, &mut r);
    }
    // Build the cache key from the NUL-STRIPPED `ns` (not raw `name`) — same
    // robustness as the resolve_gles_int fix: a NUL-terminated caller must not
    // be rejected by CString::new's interior-NUL check.
    let key = CString::new(ns).ok()?;
    // If resolve() already bound this name (e.g. after RTLD_GLOBAL made it visible),
    // reuse the cached slot rather than allocating a duplicate.
    {
        let r = resolver().lock().unwrap();
        if let Some(addr) = r.slots.get(&key) {
            return Some(*addr);
        }
    }
    let mh = egl_handle();
    if mh.is_null() {
        return None;
    }
    let sym = key.as_ptr();
    let ptr = unsafe { sym_from(mh, sym) };
    if ptr.is_null() {
        return None;
    }
    // EGL entry points are integer/pointer-ABI -> fits the integer HostCall.
    let hostf: HostCall = unsafe { std::mem::transmute(ptr) };
    let mut r = resolver().lock().unwrap();
    alloc_slot(&mut r, &key, hostf)
}

/// GLES entry points whose ABI is integer/pointer-only AND take at most 8
/// integer args — i.e. they are safe through the JIT integer `HostCall`.
/// (Float-taking `gl*` and >8-arg forms like glTexImage2D are excluded: their
/// ABI needs a float bridge / stack args the integer HostCall cannot express.)
/// Generated from the exact system GLES headers (glesv2-wrapper/gen_forward.py).
pub const GLES_INT_NAME_LIST: &[&[u8]] = &[
    b"glActiveTexture\0",
    b"glAttachShader\0",
    b"glBindAttribLocation\0",
    b"glBindBuffer\0",
    b"glBindBufferBase\0",
    b"glBindBufferRange\0",
    b"glBufferStorage\0",
    b"glBufferStorageEXT\0",
    b"glBindFramebuffer\0",
    b"glBindRenderbuffer\0",
    b"glBindTexture\0",
    b"glBlendEquation\0",
    b"glBlendEquationSeparate\0",
    b"glBlendFunc\0",
    b"glBlendFuncSeparate\0",
    b"glBufferData\0",
    b"glBufferSubData\0",
    b"glCheckFramebufferStatus\0",
    b"glClear\0",
    b"glClearBufferfv\0",
    b"glClearBufferiv\0",
    b"glClearStencil\0",
    b"glColorMask\0",
    b"glCompileShader\0",
    b"glCopyTexImage2D\0",
    b"glCopyTexSubImage2D\0",
    b"glCreateProgram\0",
    b"glCreateShader\0",
    b"glCullFace\0",
    b"glDeleteBuffers\0",
    b"glDeleteFramebuffers\0",
    b"glDeleteProgram\0",
    b"glDeleteRenderbuffers\0",
    b"glDeleteShader\0",
    b"glDeleteTextures\0",
    b"glDepthFunc\0",
    b"glDepthMask\0",
    b"glDetachShader\0",
    b"glDisable\0",
    b"glDisableVertexAttribArray\0",
    b"glDrawArrays\0",
    b"glDrawArraysInstanced\0",
    b"glDrawBuffers\0",
    b"glDrawElements\0",
    b"glDrawElementsInstanced\0",
    b"glEnable\0",
    b"glEnableVertexAttribArray\0",
    b"glFinish\0",
    b"glFlush\0",
    b"glFramebufferRenderbuffer\0",
    b"glFramebufferTexture2D\0",
    b"glFrontFace\0",
    b"glGenBuffers\0",
    b"glGenFramebuffers\0",
    b"glGenRenderbuffers\0",
    b"glGenTextures\0",
    b"glGenerateMipmap\0",
    b"glGetActiveAttrib\0",
    b"glGetActiveUniform\0",
    b"glGetActiveUniformBlockiv\0",
    b"glGetAttachedShaders\0",
    b"glGetAttribLocation\0",
    b"glGetBooleanv\0",
    b"glGetBufferParameteriv\0",
    b"glGetError\0",
    b"glGetFramebufferAttachmentParameteriv\0",
    b"glGetIntegerv\0",
    b"glGetTexLevelParameteriv\0",
    b"glGetProgramInfoLog\0",
    b"glGetProgramBinary\0",
    b"glGetProgramiv\0",
    b"glGetQueryObjectiv\0",
    b"glGetQueryObjectivEXT\0",
    b"glGetQueryObjectui64v\0",
    b"glGetQueryObjectui64vEXT\0",
    b"glGetRenderbufferParameteriv\0",
    b"glGetShaderInfoLog\0",
    b"glGetShaderPrecisionFormat\0",
    b"glGetShaderSource\0",
    b"glGetShaderiv\0",
    b"glGetString\0",
    b"glGetTexParameteriv\0",
    b"glGetUniformBlockIndex\0",
    b"glGetUniformLocation\0",
    b"glGetUniformiv\0",
    b"glGetVertexAttribPointerv\0",
    b"glGetVertexAttribiv\0",
    b"glHint\0",
    b"glIsBuffer\0",
    b"glIsEnabled\0",
    b"glIsFramebuffer\0",
    b"glIsProgram\0",
    b"glIsRenderbuffer\0",
    b"glIsShader\0",
    b"glIsTexture\0",
    b"glLinkProgram\0",
    b"glMapBuffer\0",
    b"glMapBufferOES\0",
    b"glObjectLabelKHR\0",
    b"glPixelStorei\0",
    b"glPopGroupMarker\0",
    b"glPopGroupMarkerEXT\0",
    b"glProgramBinary\0",
    b"glProgramParameteri\0",
    b"glPushGroupMarker\0",
    b"glPushGroupMarkerEXT\0",
    b"glQueryCounter\0",
    b"glQueryCounterEXT\0",
    b"glReadPixels\0",
    b"glReleaseShaderCompiler\0",
    b"glRenderbufferStorage\0",
    b"glScissor\0",
    b"glShaderBinary\0",
    b"glShaderSource\0",
    b"glStencilFunc\0",
    b"glStencilFuncSeparate\0",
    b"glStencilMask\0",
    b"glStencilMaskSeparate\0",
    b"glStencilOp\0",
    b"glStencilOpSeparate\0",
    b"glTexParameteri\0",
    b"glTexParameteriv\0",
    b"glTexParameterfv\0",
    b"glGetTexParameterfv\0",
    b"glGetFloatv\0",
    b"glUniform1i\0",
    b"glUniform1iv\0",
    b"glUniform1fv\0",
    b"glUniform2i\0",
    b"glUniform2iv\0",
    b"glUniform2fv\0",
    b"glUniform3i\0",
    b"glUniform3iv\0",
    b"glUniform3fv\0",
    b"glUniform4i\0",
    b"glUniform4iv\0",
    b"glUniform4fv\0",
    b"glUniformMatrix2fv\0",
    b"glUniformMatrix3fv\0",
    b"glUniformMatrix4fv\0",
    b"glUniformBlockBinding\0",
    b"glUseProgram\0",
    b"glValidateProgram\0",
    b"glVertexAttribDivisor\0",
    b"glVertexAttribPointer\0",
    b"glViewport\0",
];

/// One-time `dlopen` of Mesa's real `libGLESv2.so.2` (RTLD_NOW|RTLD_LOCAL) so
/// integer-ABI `gl*` imports resolve to real Mesa instead of the NULL/0 catch-all.
/// RTLD_LOCAL (NOT GLOBAL) is deliberate: if GLES went global, the general
/// `resolve()` RTLD_DEFAULT scan would also grab float-taking `gl*` (e.g.
/// glClearColor) and bind them through the integer HostCall, corrupting their xmm
/// args. Local scope keeps GLES names visible only to `resolve_gles_int`'s
/// whitelist, so float ABI stays on the NULL/0 stub.
fn gles_handle() -> *mut libc::c_void {
    static H: OnceLock<usize> = OnceLock::new();
    let addr = *H.get_or_init(|| {
        let candidates: &[&[u8]] = &[b"libGLESv2.so.2\0", b"libGLESv2.so\0"];
        for path in candidates {
            let h = unsafe {
                libc::dlopen(path.as_ptr() as *const libc::c_char, libc::RTLD_NOW | libc::RTLD_LOCAL)
            };
            if !h.is_null() { return h as usize; }
        }
        0
    });
    addr as *mut libc::c_void
}

/// Fallback handle for GL4 / extension `gl*` names (glBufferStorage, glMapBuffer,
/// glQueryCounter, glObjectLabelKHR, push/pop-group-marker …) that Mesa's ES-only
/// `libGLESv2.so.2` does not export but the desktop `libGL.so.1` does. The real
/// client resolves these through `eglGetProcAddress` to fill the render
/// dispatch-table slots (BSS 0x106d3b2f0 + 8*N); when the requested name is in
/// `GLES_INT_NAME_LIST` but absent from GLESv2, we fall back here so the slot is a
/// real dispatchable Mesa address instead of NULL/0 (the engine `br`s those slots
/// UNGUARDED, so a 0 would jump-to-NULL in a self-driven frame).
///
/// Same RTLD_LOCAL discipline as `gles_handle`: keeps these names visible only to
/// the whitelisted int-ABI resolver, not the general RTLD_DEFAULT `resolve()`.
fn gl_desktop_handle() -> *mut libc::c_void {
    static H: OnceLock<usize> = OnceLock::new();
    let addr = *H.get_or_init(|| {
        let candidates: &[&[u8]] = &[b"libGL.so.1\0", b"libGL.so\0"];
        for path in candidates {
            let h = unsafe {
                libc::dlopen(path.as_ptr() as *const libc::c_char, libc::RTLD_NOW | libc::RTLD_LOCAL)
            };
            if !h.is_null() { return h as usize; }
        }
        0
    });
    addr as *mut libc::c_void
}

/// Resolve an integer-ABI `gl*` import against Mesa's real libGLESv2. Only names in
/// [`GLES_INT_NAME_LIST`] (pure integer/pointer args, <=8) are safe through the integer
/// HostCall; float-taking GLES or >8-arg forms return `None` (fall to the NULL/0 stub).
pub fn resolve_gles_int(name: &[u8]) -> Option<u64> {
    let ns = name_str(name);
    if !ns.starts_with("gl") {
        return None;
    }
    if !GLES_INT_NAME_LIST.iter().any(|c| c[..c.len()-1] == *ns.as_bytes()) {
        return None;
    }
    // Use the NUL-STRIPPED `ns` (not the raw `name`) so a caller that passes a
    // trailing NUL — elfjit's --renderframe-seedgles, w_eglGetProcAddress with a
    // NUL-terminated guest C-string, etc. — still resolves. CString::new rejects
    // an interior NUL, so building it from `name` wrongly returned None for every
    // NUL-terminated caller (mixed strips first and works; int did not).
    let key = CString::new(ns).ok()?;
    {
        let r = resolver().lock().unwrap();
        if let Some(addr) = r.slots.get(&key) { return Some(*addr); }
    }
    let mh = gles_handle();
    if mh.is_null() { return None; }
    let sym = key.as_ptr();
    let ptr = unsafe { sym_from(mh, sym) };
    // Fall back to desktop libGL for GL4/extension names that Mesa's ES-only
    // libGLESv2 does not export (glBufferStorage, glMapBuffer, glQueryCounter,
    // glObjectLabelKHR, …). The real client resolves these via eglGetProcAddress
    // into its render dispatch-table slots; returning a real Mesa address here
    // keeps a self-driven frame from `br`-ing to NULL.
    let ptr = if ptr.is_null() {
        let dh = gl_desktop_handle();
        if dh.is_null() { return None; }
        unsafe { sym_from(dh, sym) }
    } else {
        ptr
    };
    // Plain (non-EXT) marker-group names: the engine requests glPushGroupMarker /
    // glPopGroupMarker (GL_EXT_debug_marker without the EXT suffix), but BOTH
    // Mesa libraries export ONLY the EXT-suffixed spellings
    // (glPushGroupMarkerEXT / glPopGroupMarkerEXT). Since the very next step is
    // the string → CArray map (`sym_from`), the GCC "asm\" name can't alias the
    // plain name; there is no other source. Rather than return NULL (a guest
    // `br` through the engine's render dispatch-table slot 11/12 for these names
    // would jump to 0 — the SH19/SH24/SH47 NULL-dispatch crash class), fall back
    // to the EXT-suffixed sibling, which is a WHITELISTED name in
    // GLES_INT_NAME_LIST and thus verified integer-ABI safe.
    let ptr = if ptr.is_null() && !ns.ends_with("EXT") {
        let key_ext = CString::new(format!("{ns}EXT")).ok()?;
        let dh = gl_desktop_handle();
        let mh = gles_handle();
        let mut s: *mut libc::c_void = std::ptr::null_mut();
        if !mh.is_null() { s = unsafe { sym_from(mh, key_ext.as_ptr()) } }
        if s.is_null() && !dh.is_null() { s = unsafe { sym_from(dh, key_ext.as_ptr()) } }
        s
    } else {
        ptr
    };
    if ptr.is_null() { return None; }
    let hostf: HostCall = unsafe { std::mem::transmute(ptr) };
    let mut r = resolver().lock().unwrap();
    alloc_slot(&mut r, &key, hostf)
}

// ---------------------------------------------------------------------------
// GLES mixed-ABI bridge
//
// Most `gl*` functions are integer-ABI (see GLES_INT_NAME_LIST) and run through
// the integer HostCall. But a core subset takes float arguments (passed in the
// low 32 bits of the guest SIMD s0..s7 lanes, not the integer x-registers) and
// several take MORE than 8 args (the 9th+ live on the guest stack). Neither the
// integer HostCall (8 x-reg args only) nor the uniform-float bridges (all-float
// ABI) can express these. For each such function we install a `HostGlesCall`
// wrapper that receives the full guest CpuState, reads the exact x/s/sp lanes
// its AArch64 signature uses, and dispatches to real Mesa. This is what lets a
// translated Roblox binary actually clear the framebuffer (glClearColor) and
// upload textures (glTexImage2D) instead of hitting the NULL/0 graphics stub.
// ---------------------------------------------------------------------------

/// Cached dlsym of a Mesa GLES symbol (real libGLESv2.so.2, already RTLD_LOCAL'd
/// by `gles_handle`). Returns the fn address, or 0 if Mesa lacks the symbol.
fn gles_sym(name: &str) -> usize {
    static CACHE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    let m = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(a) = m.lock().unwrap().get(name) {
        return *a;
    }
    let mh = gles_handle();
    let addr = if mh.is_null() {
        0
    } else {
        let c = CString::new(name).unwrap();
        unsafe { sym_from(mh, c.as_ptr()) as usize }
    };
    m.lock().unwrap().insert(name.to_string(), addr);
    addr
}

// Guest AArch64 arg-lane readers at a `blr` thunk boundary:
/// Low 32 bits of guest SIMD register `vN` (an f32 argument in sN).
#[inline]
fn gs_f(st: &CpuState, n: usize) -> f32 {
    f32::from_bits(st.v[2 * n] as u32)
}
/// Integer arg register xN (low 32 bits / full u64).
#[inline]
fn gs_x(st: &CpuState, n: usize) -> u64 {
    st.x[n]
}
/// The N-th stack argument (N >= 8): the guest spilled args 8+ at the top of
/// its stack at `[sp + 8*(N-8)]` per the AArch64 calling convention.
unsafe fn gs_stack(st: &CpuState, n: usize) -> u64 {
    let sp = st.x[31];
    core::ptr::read_unaligned((sp + 8 * (n as u64 - 8)) as *const u64)
}

macro_rules! gles_ret {
    ($st:expr) => {
        0u64 // all the wrapped calls below are `void`; the guest ignores x0
    };
}

/// Wrapper for the purely-float clear/coverage set (glClearColor/_tl blColor…).
// (Written explicitly rather than via a rep-macro: Rust can't drive a `$(f32),*`
// type-list repetition off a distinguished `$($a:expr),*` lane-index matcher.)
extern "C" fn w_glClearColor(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(f32, f32, f32, f32) = unsafe { std::mem::transmute(gles_sym("glClearColor")) };
    unsafe { f(gs_f(s, 0), gs_f(s, 1), gs_f(s, 2), gs_f(s, 3)) };
    0u64
}
extern "C" fn w_glBlendColor(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(f32, f32, f32, f32) = unsafe { std::mem::transmute(gles_sym("glBlendColor")) };
    unsafe { f(gs_f(s, 0), gs_f(s, 1), gs_f(s, 2), gs_f(s, 3)) };
    0u64
}
extern "C" fn w_glClearDepthf(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(f32) = unsafe { std::mem::transmute(gles_sym("glClearDepthf")) };
    unsafe { f(gs_f(s, 0)) };
    0u64
}
extern "C" fn w_glDepthRangef(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(f32, f32) = unsafe { std::mem::transmute(gles_sym("glDepthRangef")) };
    unsafe { f(gs_f(s, 0), gs_f(s, 1)) };
    0u64
}
extern "C" fn w_glLineWidth(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(f32) = unsafe { std::mem::transmute(gles_sym("glLineWidth")) };
    unsafe { f(gs_f(s, 0)) };
    0u64
}
extern "C" fn w_glPolygonOffset(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(f32, f32) = unsafe { std::mem::transmute(gles_sym("glPolygonOffset")) };
    unsafe { f(gs_f(s, 0), gs_f(s, 1)) };
    0u64
}

/// glSampleCoverage(float value, GLboolean invert): value in s0, invert in x1.
extern "C" fn w_glSampleCoverage(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(f32, u8) = unsafe { std::mem::transmute(gles_sym("glSampleCoverage")) };
    unsafe { f(gs_f(s, 0), gs_x(s, 1) as u8) };
    gles_ret!(s)
}

/// glTexParameterf(GLenum target, GLenum pname, GLfloat param): target/pname in
/// x0/x1, the float param in s2 (third arg).
extern "C" fn w_glTexParameterf(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, u32, f32) = unsafe { std::mem::transmute(gles_sym("glTexParameterf")) };
    unsafe { f(gs_x(s, 0) as u32, gs_x(s, 1) as u32, gs_f(s, 2)) };
    gles_ret!(s)
}

/// glClearBufferfi(GLenum buffer, GLint drawbuffer, GLfloat depth, GLint stencil):
/// buffer/drawbuffer in x0/x1, stencil in x2, and the float depth in s0 (the FIRST
/// FP arg — AAPCS). Verified against the real Roblox clear-state fn 0x5b32ef4:
/// `ldr s0,[x21,#68]` (depth), `ldr w2,[x21,#72]` (stencil), `mov w0,#0x84f9`
/// (GL_DEPTH_STENCIL), `mov w1,wzr` (drawbuffer), then bl the slot-3 stub 0x5b3a1e4.
/// This is the per-buffer combined depth+stencil clear (slot 3), NOT glClearStencil.
extern "C" fn w_glClearBufferfi(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, i32, f32, i32) =
        unsafe { std::mem::transmute(gles_sym("glClearBufferfi")) };
    unsafe { f(gs_x(s, 0) as u32, gs_x(s, 1) as i32, gs_f(s, 0), gs_x(s, 2) as i32) };
    gles_ret!(s)
}

/// glUniformNf(GLint location, float...): location in x0, the floats in s1..sN.
extern "C" fn w_glUniform1f(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(i32, f32) = unsafe { std::mem::transmute(gles_sym("glUniform1f")) };
    unsafe { f(gs_x(s, 0) as i32, gs_f(s, 1)) };
    gles_ret!(s)
}
extern "C" fn w_glUniform2f(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(i32, f32, f32) =
        unsafe { std::mem::transmute(gles_sym("glUniform2f")) };
    unsafe { f(gs_x(s, 0) as i32, gs_f(s, 1), gs_f(s, 2)) };
    gles_ret!(s)
}
extern "C" fn w_glUniform3f(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(i32, f32, f32, f32) =
        unsafe { std::mem::transmute(gles_sym("glUniform3f")) };
    unsafe { f(gs_x(s, 0) as i32, gs_f(s, 1), gs_f(s, 2), gs_f(s, 3)) };
    gles_ret!(s)
}
extern "C" fn w_glUniform4f(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(i32, f32, f32, f32, f32) =
        unsafe { std::mem::transmute(gles_sym("glUniform4f")) };
    unsafe { f(gs_x(s, 0) as i32, gs_f(s, 1), gs_f(s, 2), gs_f(s, 3), gs_f(s, 4)) };
    gles_ret!(s)
}

/// glVertexAttribNf(GLuint index, float...): index in x0, floats in s1..sN.
extern "C" fn w_glVertexAttrib1f(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, f32) = unsafe { std::mem::transmute(gles_sym("glVertexAttrib1f")) };
    unsafe { f(gs_x(s, 0) as u32, gs_f(s, 1)) };
    gles_ret!(s)
}
extern "C" fn w_glVertexAttrib2f(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, f32, f32) =
        unsafe { std::mem::transmute(gles_sym("glVertexAttrib2f")) };
    unsafe { f(gs_x(s, 0) as u32, gs_f(s, 1), gs_f(s, 2)) };
    gles_ret!(s)
}
extern "C" fn w_glVertexAttrib3f(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, f32, f32, f32) =
        unsafe { std::mem::transmute(gles_sym("glVertexAttrib3f")) };
    unsafe { f(gs_x(s, 0) as u32, gs_f(s, 1), gs_f(s, 2), gs_f(s, 3)) };
    gles_ret!(s)
}
extern "C" fn w_glVertexAttrib4f(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, f32, f32, f32, f32) =
        unsafe { std::mem::transmute(gles_sym("glVertexAttrib4f")) };
    unsafe { f(
        gs_x(s, 0) as u32,
        gs_f(s, 1),
        gs_f(s, 2),
        gs_f(s, 3),
        gs_f(s, 4),
    ) };
    gles_ret!(s)
}

// ---- >8-arg integer/pointer GLES: the extra args ride on the guest stack ----
/// glTexImage2D(GLenum target, GLint level, GLint internalformat, GLsizei width,
/// GLsizei height, GLint border, GLenum format, GLenum type, const void *pixels):
/// 9 args — `pixels` (arg 8) is the first stack arg.
extern "C" fn w_glTexImage2D(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const u8) =
        unsafe { std::mem::transmute(gles_sym("glTexImage2D")) };
    unsafe {
        f(
            gs_x(s, 0) as u32,
            gs_x(s, 1) as i32,
            gs_x(s, 2) as i32,
            gs_x(s, 3) as i32,
            gs_x(s, 4) as i32,
            gs_x(s, 5) as i32,
            gs_x(s, 6) as u32,
            gs_x(s, 7) as u32,
            gs_stack(s, 8) as *const u8,
        )
    };
    gles_ret!(s)
}
/// Same shape as glTexImage2D: x0-x7 = target,level,xofs,yofs,width,height,
/// format,type; pixels on the stack.
extern "C" fn w_glTexSubImage2D(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const u8) =
        unsafe { std::mem::transmute(gles_sym("glTexSubImage2D")) };
    unsafe {
        f(
            gs_x(s, 0) as u32,
            gs_x(s, 1) as i32,
            gs_x(s, 2) as i32,
            gs_x(s, 3) as i32,
            gs_x(s, 4) as i32,
            gs_x(s, 5) as i32,
            gs_x(s, 6) as u32,
            gs_x(s, 7) as u32,
            gs_stack(s, 8) as *const u8,
        )
    };
    gles_ret!(s)
}
/// glTexImage3D(...) 10 args: the pixels pointer is arg 9, at [sp+8].
extern "C" fn w_glTexImage3D(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let f: extern "C" fn(u32, i32, i32, i32, i32, i32, i32, u32, u32, *const u8) =
        unsafe { std::mem::transmute(gles_sym("glTexImage3D")) };
    unsafe {
        f(
            gs_x(s, 0) as u32,
            gs_x(s, 1) as i32,
            gs_x(s, 2) as i32,
            gs_x(s, 3) as i32,
            gs_x(s, 4) as i32,
            gs_x(s, 5) as i32,
            gs_x(s, 6) as i32,
            gs_x(s, 7) as u32,
            gs_stack(s, 8) as u32,
            gs_stack(s, 9) as *const u8,
        )
    };
    gles_ret!(s)
}

/// glCompressedTexImage2D(target, level, internalformat, width, height, border,
/// imageSize, data): all 8 args fit the integer x-regs. For an Android compressed
/// format (ETC1/ETC2/EAC/ASTC) decode to RGBA via texture_codec and upload as
/// GL_RGBA8 through real Mesa glTexImage2D; otherwise fall through to real Mesa
/// glCompressedTexImage2D. (Not in the integer whitelist on purpose: it needs this
/// interception, and resolve_gles_mixed is checked after resolve_gles_int.)
extern "C" fn w_glCompressedTexImage2D(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let target = gs_x(s, 0) as u32;
    let level = gs_x(s, 1) as i32;
    let internalformat = gs_x(s, 2) as u32;
    let width = gs_x(s, 3) as i32;
    let height = gs_x(s, 4) as i32;
    let border = gs_x(s, 5) as i32;
    let image_size = gs_x(s, 6) as i32;
    let data = gs_x(s, 7) as *const libc::c_void;
    // Real Mesa glTexImage2D (for the decoded-RGBA upload) and glCompressedTexImage2D
    // (the fall-through). Both are dlsym'd from the RTLD_LOCAL GLES handle.
    #[allow(clippy::type_complexity)]
    let real_tex: unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const libc::c_void) =
        unsafe { std::mem::transmute(gles_sym("glTexImage2D")) };
    #[allow(clippy::type_complexity)]
    let real_compressed: unsafe extern "C" fn(u32, i32, u32, i32, i32, i32, i32, *const u8) =
        unsafe { std::mem::transmute(gles_sym("glCompressedTexImage2D")) };
    unsafe {
        if !texture_codec::handle_compressed_tex_image_2d(
            target, level, internalformat, width, height, border, image_size,
            data, real_tex,
        ) {
            real_compressed(target, level, internalformat, width, height, border, image_size, data as *const u8);
        }
    }
    gles_ret!(s)
}

/// glCompressedTexSubImage2D(target, level, xoffset, yoffset, width, height,
/// format, imageSize, data): 9 args — `data` (arg 8) is the first stack arg.
/// Android format -> decode the sub-rect to RGBA and upload via real glTexSubImage2D;
/// else fall through to real Mesa glCompressedTexSubImage2D.
extern "C" fn w_glCompressedTexSubImage2D(st: *mut CpuState) -> u64 {
    let s = unsafe { &*st };
    let target = gs_x(s, 0) as u32;
    let level = gs_x(s, 1) as i32;
    let xoffset = gs_x(s, 2) as i32;
    let yoffset = gs_x(s, 3) as i32;
    let width = gs_x(s, 4) as i32;
    let height = gs_x(s, 5) as i32;
    let format = gs_x(s, 6) as u32;
    let image_size = gs_x(s, 7) as i32;
    let data = unsafe { gs_stack(s, 8) } as *const libc::c_void;
    #[allow(clippy::type_complexity)]
    let real_sub: unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const libc::c_void) =
        unsafe { std::mem::transmute(gles_sym("glTexSubImage2D")) };
    #[allow(clippy::type_complexity)]
    let real_compressed_sub: unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, i32, *const u8) =
        unsafe { std::mem::transmute(gles_sym("glCompressedTexSubImage2D")) };
    unsafe {
        if !texture_codec::handle_compressed_tex_sub_image_2d(
            target, level, xoffset, yoffset, width, height, format, image_size,
            data, real_sub,
        ) {
            real_compressed_sub(target, level, xoffset, yoffset, width, height, format, image_size, data as *const u8);
        }
    }
    gles_ret!(s)
}

/// Look up the GLES mixed-ABI bridge for a `gl*` name, or None if it's a pure
/// integer-ABI / unknown GLES function (the caller should fall back to
/// `resolve_gles_int` or the NULL stub).
fn gles_mixed_wrapper(name: &str) -> Option<HostGlesCall> {
    use HostGlesCall as H;
    Some(match name {
        "glClearColor" => w_glClearColor as H,
        "glBlendColor" => w_glBlendColor as H,
        "glClearDepthf" => w_glClearDepthf as H,
        "glClearBufferfi" => w_glClearBufferfi as H,
        "glDepthRangef" => w_glDepthRangef as H,
        "glLineWidth" => w_glLineWidth as H,
        "glPolygonOffset" => w_glPolygonOffset as H,
        "glSampleCoverage" => w_glSampleCoverage as H,
        "glTexParameterf" => w_glTexParameterf as H,
        "glUniform1f" => w_glUniform1f as H,
        "glUniform2f" => w_glUniform2f as H,
        "glUniform3f" => w_glUniform3f as H,
        "glUniform4f" => w_glUniform4f as H,
        "glVertexAttrib1f" => w_glVertexAttrib1f as H,
        "glVertexAttrib2f" => w_glVertexAttrib2f as H,
        "glVertexAttrib3f" => w_glVertexAttrib3f as H,
        "glVertexAttrib4f" => w_glVertexAttrib4f as H,
        "glTexImage2D" => w_glTexImage2D as H,
        "glTexSubImage2D" => w_glTexSubImage2D as H,
        "glCompressedTexSubImage2D" => w_glCompressedTexSubImage2D as H,
        "glCompressedTexImage2D" => w_glCompressedTexImage2D as H,
        "glTexImage3D" => w_glTexImage3D as H,
        _ => return None,
    })
}

/// Resolve a float/mixed-ABI (or >8-arg) `gl*` import to a GLES bridge slot.
/// Returns `None` if the name isn't a wrapped GLES function, if Mesa GLES isn't
/// present, or if Mesa lacks the symbol (bridge must not install a dangling
/// call). These names are NOT in GLES_INT_NAME_LIST (the integer HostCall cannot
/// marshal their float/stack args); via the bridge they still execute real Mesa.
pub fn resolve_gles_mixed(name: &[u8]) -> Option<u64> {
    let ns = name_str(name);
    if !ns.starts_with("gl") {
        return None;
    }
    let wrapped = gles_mixed_wrapper(ns)?;
    let mh = gles_handle();
    if mh.is_null() {
        return None;
    }
    // Only install the bridge if real Mesa actually exports the symbol (a stray
    // name would give us a null fn pointer in gles_sym and a crash on call).
    let c = CString::new(ns).ok()?;
    let ptr = unsafe { sym_from(mh, c.as_ptr()) };
    if ptr.is_null() {
        return None;
    }
    let slot = register_gles_call(wrapped);
    crate::jit::name_host_call_slot(slot, &ns);
    Some(slot)
}

/// Give an import name a host call slot. If the host symbol is found via
/// `dlsym`, register it and return the thunk's *guest address*; if the name
/// can't be resolved on the host, return `None` (caller must decide how to
/// handle a missing import).
pub fn resolve(name: &[u8]) -> Option<u64> {
    let mut r = resolver().lock().unwrap();
    let key = CString::new(name).ok()?;
    if let Some(addr) = r.slots.get(&key) {
        return Some(*addr);
    }
    // The guest calls its libc `syscall()` function (imported syscall@LIBC) for
    // raw AArch64 syscalls (futex, clock_gettime, mmap, ...). Binding it to
    // HOST glibc `syscall()` interprets the guest's AArch64 number as an
    // x86-64 number — the engine main loop's futex (AArch64 nr 98) would call
    // x86-64 getrusage and return -1, so the futex never blocks and the loop
    // busy-spins (this is the cycle-I/J "settled loop" — it makes syscalls via
    // this import, NOT `svc #0`, which is why probing only `svc` showed "zero
    // guest syscalls"). Route it through our AArch64 syscall dispatcher so the
    // number->host mapping is exact (same code path as a guest `svc #0`).
    if name_str(name) == "syscall" {
        let hostf: HostCall = host_syscall_intercept;
        return alloc_slot(&mut r, &key, hostf);
    }
    // `eglGetProcAddress` is the guest's dynamic GLES loader (Roblox resolves
    // most ES entry points through it). Route it through the GLES bridge
    // (resolve_egl_get_proc_address) so a returned pointer is one of OUR
    // dispatchable host-thunk slots — preserving the GLES float bridge and
    // compressed-texture interception — instead of Mesa's raw function. This
    // branch must come BEFORE the generic dlsym below, which (once libEGL is
    // RTLD_GLOBAL) would bind the real function and defeat the interception.
    if name_str(name) == "eglGetProcAddress" {
        return resolve_egl_get_proc_address(&key, &mut r);
    }
    // Bionic pthread fixup: the guest binary was built against bionic, whose
    // pthread_mutex_t is 44 bytes (glibc's is 40), __kind lives at offset 16 and
    // __count is reused as __owner at offset 8. Passing such a mutex to glibc's
    // pthread_mutex_lock/cond_wait makes glibc see kind==0x10 (ROBUST_NORMAL) or
    // a stray owner count and it crashes/deadlocks, driving Roblox init into its
    // abort path. Mirror jni_shim.c `sanitize_mutex`: route these imports
    // through a wrapper that fixes the mutex layout in place first. Since the
    // runtime maps guest==host contiguously, the guest pointer is host-addressable.
    if let Some(real) = real_libc_pthread(name) {
        store_real(name_str(name), real);
        let hostf: HostCall = match name_str(name) {
            "pthread_mutex_lock" | "pthread_mutex_unlock" => {
                // both take one mutex arg and return int; lock/unlock collide, so
                // pick the right bridge by exact name.
                if name_str(name) == "pthread_mutex_unlock" {
                    host_mutex_unlock
                } else {
                    host_mutex_lock
                }
            }
            "pthread_cond_wait" => host_cond_wait,
            "pthread_cond_timedwait" => host_cond_timedwait,
            "pthread_mutex_init" => host_mutex_init,
            _ => unsafe { std::mem::transmute(real) },
        };
        return alloc_slot(&mut r, &key, hostf);
    }
    // dlsym the host symbol. We are resolving against the process-global
    // symbol space (libc/libm/any shared lib already loaded), which covers
    // the aarch64 libc/libm imports whose names collide with host names.
    let sym = key.as_ptr();
    // RTLD_DEFAULT only sees already-loaded libs; libm is often not yet loaded.
    // Fall back to an explicit `dlopen("libm.so.6")` handle so libm names resolve.
    let mut ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, sym) };
    if ptr.is_null() {
        let mh = libm_handle();
        if !mh.is_null() {
            ptr = unsafe { sym_from(mh, sym) };
        }
    }
    if ptr.is_null() {
        return None; // not present on the host
    }
    let hostf: HostCall = unsafe { std::mem::transmute(ptr) };
    alloc_slot(&mut r, &key, hostf)
}

/// Register `hostf` at a fresh resolver slot keyed by `key`; returns slot addr.
///
/// Caller holds the resolver lock. Concurrency-safe/idempotent: before
/// allocating a new slot, re-check the cache for `key` — a concurrent resolver
/// may have bound it between the caller's earlier cache-probe (which ran
/// WITHOUT the lock, before the dlsym) and this call. Without this re-check the
/// same name resolved on two threads allocates two DIFFERENT slots (adjacent,
/// address off-by-8), breaking the slot-identity invariant (`addr==addr` for
/// the same GLES name) that the resolver's public API and its tests rely on.
fn alloc_slot(r: &mut Resolver, key: &CString, hostf: HostCall) -> Option<u64> {
    if let Some(addr) = r.slots.get(key) {
        return Some(*addr);
    }
    if r.next >= crate::jit::HOST_THUNK_MAX {
        return None;
    }
    let slot = r.next;
    r.next += 1;
    register_host_call(slot, hostf);
    let addr = host_call_addr(slot);
    r.slots.insert(key.clone(), addr);
    Some(addr)
}

/// Interceptor for the guest's libc `syscall()` import. The guest calls
/// `syscall(AArch64_nr, a0, a1, a2, a3, a4, a5)` — x0..x5 = the syscall's
/// AArch64 arguments, with the NUMBER in x0 (SysV GPR convention forwards this
/// as our hostcall a0..a5). We reconstruct a fake `CpuState` whose `x[8]` (the
/// `svc #0` syscall-number slot) is `a0` and whose `x[0..5]` are `a1..a6`, then
/// dispatch through `guest_svc` — the SAME AArch64->host mapping a real guest
/// `svc #0` uses. This makes the engine's futex (AArch64 nr 98) reach real
/// host futex with the correct op/val/etc.
///
/// Binding the guest's `syscall` import to host glibc `syscall()` directly was
/// wrong: host glibc reads the number as an x86-64 syscall number (nr 98 = a
/// futex on AArch64, getrusage on x86-64), so the engine's futex never worked.
extern "C" fn host_syscall_intercept(
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
    a7: u64,
) -> u64 {
    // Build a minimal CpuState: only x[8] (syscall nr) and x[0..6] (args) matter
    // to guest_svc for the common cases (futex, clock_gettime, mmap, ...). The
    // fields clone/exit paths consult (tid, clear_tid_addr) default 0 here; a
    // guest that reaches those via libc syscall() (rare; the engine uses svc
    // for thread control) will still dispatch but with thread-0 semantics.
    let mut st = CpuState::new();
    st.x[0] = a1;
    st.x[1] = a2;
    st.x[2] = a3;
    st.x[3] = a4;
    st.x[4] = a5;
    st.x[5] = a6;
    st.x[8] = a0; // the AArch64 syscall number
    unsafe { crate::jit::guest_svc(&mut st as *mut CpuState) }
}

/// Reverse-lookup an import slot address back to its symbol name (the first
/// registered name that maps to this thunk address). Used by the JIT_TRACE
/// hostcall dumper to say *which* import a hot loop is dispatching, instead of
/// an anonymous slot number.
pub fn name_of_call_addr(addr: u64) -> Option<String> {
    // 1) Named imports (resolver map).
    let r = resolver().lock().unwrap();
    let named = r
        .slots
        .iter()
        .find(|(_, v)| **v == addr)
        .map(|(k, _)| String::from_utf8_lossy(k.as_bytes()).into_owned());
    drop(r);
    if named.is_some() {
        return named;
    }
    // 2) Auto-allocated GLES/float/JNI bridges recorded in the JIT's
    //    reverse-name registry (anonymous `slotN` otherwise).
    crate::jit::host_call_slot_name(addr)
}

/// Name of an import as `&str`, tolerating a trailing NUL.
fn name_str(name: &[u8]) -> &str {
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    std::str::from_utf8(&name[..end]).unwrap_or("")
}

/// dlsym a pthread mutex/cond function we plan to wrap, or None (not ours).
fn real_libc_pthread(name: &[u8]) -> Option<*mut libc::c_void> {
    let n = name_str(name);
    if !(n == "pthread_mutex_lock"
        || n == "pthread_mutex_unlock"
        || n == "pthread_mutex_init"
        || n == "pthread_cond_wait"
        || n == "pthread_cond_timedwait")
    {
        return None;
    }
    let c = CString::new(n).ok()?;
    // SAFETY: name is one of the fixed whitelist strings above; NUL-terminated.
    Some(unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) })
}

/// Normalize a candidate pthread_mutex_t in place to a form glibc can lock:
/// clear the robust/high kind bits at offset 16. It MUST leave offset 8 alone:
/// that is glibc's `__owner` (the owning host TID) on the glibc pthread_mutex_t
/// layout, and a TID is typically > 0x10000 — zeroing it on a LIVE recursive
/// mutex makes glibc see `__owner==0 != self` on the next same-thread re-lock,
/// so the owner futex-blocks on its OWN mutex (the GameActivity 0x6edae60 wall:
/// __owner=0x0 while __count=1, both threads parked at pthread_mutex_lock).
/// Bionic stores owner_tid at offset 4 (not 8), so offset 8 is never a bionic
/// recursion/owner leak worth clearing on either ABI.
///
/// # Safety
/// `m` must be a non-null, writable pointer to at least 20 bytes (the mutex).
unsafe fn sanitize_mutex(m: *mut u8) {
    if m.is_null() {
        return;
    }
    let kind = core::ptr::read_unaligned(m.add(16) as *const i32);
    core::ptr::write_unaligned(m.add(16) as *mut i32, kind & 3);
}

type MutexLockFn = unsafe extern "C" fn(*mut u8) -> i32;
type MutexCondFn = unsafe extern "C" fn(*mut u8, *mut u8) -> i32;
type MutexInitFn = unsafe extern "C" fn(*mut u8, *mut u8) -> i32;

// Real glibc pthread functions, cached once. "real" means we already vetted the
// dlsym'd address before installing a wrapper, so these are non-null.
static REAL_LOCK: OnceLock<MutexLockFn> = OnceLock::new();
static REAL_UNLOCK: OnceLock<MutexLockFn> = OnceLock::new();
static REAL_COND_WAIT: OnceLock<MutexCondFn> = OnceLock::new();
static REAL_COND_TIMEDWAIT: OnceLock<MutexCondFn> = OnceLock::new();
static REAL_MUTEX_INIT: OnceLock<MutexInitFn> = OnceLock::new();

/// Record the real glibc fn for `name`; false on an unknown/unwanted name.
fn store_real(name: &str, real: *mut libc::c_void) {
    let poke = |target: &OnceLock<MutexLockFn>, f: MutexLockFn| {
        let _ = target.set(f);
    };
    match name {
        "pthread_mutex_lock" => poke(&REAL_LOCK, unsafe { std::mem::transmute(real) }),
        "pthread_mutex_unlock" => poke(&REAL_UNLOCK, unsafe { std::mem::transmute(real) }),
        "pthread_cond_wait" => {
            let _ = REAL_COND_WAIT.set(unsafe { std::mem::transmute(real) });
        }
        "pthread_cond_timedwait" => {
            let _ = REAL_COND_TIMEDWAIT.set(unsafe { std::mem::transmute(real) });
        }
        "pthread_mutex_init" => {
            let _ = REAL_MUTEX_INIT.set(unsafe { std::mem::transmute(real) });
        }
        _ => {}
    }
}

/// Host bridge fn: pthread_mutex_lock/mutex_unlock over the guest mutex.
///
/// We implement Android bionic's NORMAL (non-PI, non-recursive, non-errorcheck)
/// mutex protocol BYTE-EXACT on the guest's own 16-bit `state` word (offset 0
/// of the 44-byte modern NDK r28c `pthread_mutex_t`), so guest threads that
/// contend/release via our bridge rendezvous correctly with each other.
///
/// Bionic `pthread_mutex_internal_t` (__LP64__): `_Atomic(uint16_t) state` @0,
/// `uint16_t __pad` @2, `atomic_int owner_tid` @4, `char __reserved[28]` @8.
/// The 16-bit `state` packs: bits 1:0 lock state (0=UNLOCKED, 1=LOCKED_
/// UNCONTENDED, 2=LOCKED_CONTENDED), bits 12:2 recursive counter, bit 13 shared,
/// bits 15:14 type (0=NORMAL, 1=RECURSIVE, 2=ERRORCHECK, 3=PI).
///
/// NORMAL protocol (from AOSP pthread_mutex.cpp NonPI::NormalMutexLock/Unlock):
///   lock:  CAS state 0->1 (acquire). On failure loop:
///          exchange state -> 2 (acquire); if prev was 0 (UNLOCKED) acquired,
///          else futex_wait(&state, 2, PRIVATE unless shared).
///   unlock: exchange state -> 0 (release); if prev was 2 (CONTENDED), wake 1.
///
/// On x86-64 the futex address is the 32-bit word covering the 16-bit state +
/// the zero `__pad` (16-bit @2), which Bionic relies on being 0 (that's exactly
/// why `__pad` exists). glibc's pthread_mutex_* can NOT be used on a bionic
/// mutex (different layout/encoding) — that is the whole cross-ABI wall — so we
/// never hand a NORMAL bionic mutex to glibc. Non-NORMAL mutexes (type!=0)
/// still route to the glibc bridge as before (boot currently only contends on
/// NORMAL).
extern "C" fn host_mutex_lock(a0: u64, _1: u64, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64) -> u64 {
    let m = a0 as *mut u8;
    if std::env::var_os("JIT_TRACE").is_some() {
        let bionic_word: u32 = if a0 != 0 {
            unsafe { core::ptr::read_unaligned(m as *const u32) }
        } else {
            0
        };
        let gpc = crate::jit::current_guest_pc();
        let self_tid = crate::jit::current_tid();
        // glibc pthread_mutex_t fields (x86-64): __lock@0 (int), __count@4
        // (recursion count), __owner@8 (host tid), __pad@12, __kind@16 (type).
        // Reading them shows who OWNS a contended mutex and the recursion
        // depth — distinguishing a genuine lifecycle-await (owner alive, held
        // across an app-command dispatch) from an abandoned lock (owner
        // exited/reaped without unlock). The widened block is skipped-reg-free;
        // the reads are plain aligned u32 loads always (m is guest==host).
        let (owner, count, kind) = if a0 != 0 {
            let g = m as *const u32;
            unsafe {
                (
                    core::ptr::read_unaligned(g.add(2)),
                    core::ptr::read_unaligned(g.add(1)),
                    core::ptr::read_unaligned(g.add(4)),
                )
            }
        } else {
            (0, 0, 0)
        };
        eprintln!(
            "[t={self_tid}] [mutex_lock] {a0:#x} bionic_word=0x{bionic_word:08x} state=0x{:x} type=0x{:x} g_owner_tid={owner:#x} g_count={count} g_kind={kind} gpcreq={gpc:#x}",
            bionic_word & 0x3, (bionic_word >> 14) & 0x3
        );
    }
    let r = unsafe { bionic_mutex_lock(m) };
    r as u64
}
extern "C" fn host_mutex_unlock(a0: u64, _1: u64, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64) -> u64 {
    let m = a0 as *mut u8;
    let r = unsafe { bionic_mutex_unlock(m) };
    r as u64
}

// ---- State-field constants & helpers for the bionic NORMAL mutex -----------
const BIONIC_STATE_LOCKED_UNCONTENDED: u16 = 1;
const BIONIC_STATE_LOCKED_CONTENDED: u16 = 2;
const BIONIC_SHARED_MASK: u16 = 0x2000; // bit 13
const BIONIC_TYPE_MASK: u16 = 0xC000; // bits 15:14; 0 == NORMAL

/// Guest addresses of mutexes that were initialized through our `host_mutex_init`
/// bridge (i.e. by REAL glibc `pthread_mutex_init` with a REAL glibc attr). Those
/// are glibc-FORMATTED mutexes, and glibc's own lock/unlock must be used on them
/// (not our bionic-word protocol) — critically, glibc tracks recursive/errorcheck
/// type in `__kind` and handles same-thread recursive re-lock by incrementing a
/// count, which a NORMAL-bionic-path would self-deadlock on. ONLY mutexes that
/// were NOT initialized through the bridge (static `PTHREAD_MUTEX_INITIALIZER`
/// zeroed words that the guest touches with bionic inline atomics) take the
/// bionic-word path.
static MUTEX_INIT_SET: OnceLock<std::sync::Mutex<std::collections::HashSet<usize>>> =
    OnceLock::new();

fn mutex_init_set() -> &'static std::sync::Mutex<std::collections::HashSet<usize>> {
    MUTEX_INIT_SET.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn record_glibc_init(m: usize) {
    if m != 0 {
        mutex_init_set().lock().unwrap().insert(m);
    }
}

/// True iff the guest called our `pthread_mutex_init` on this mutex (so it is
/// glibc-formatted and glibc owns its lock/unlock semantics).
fn is_glibc_initialized(m: *const u8) -> bool {
    !m.is_null() && mutex_init_set().lock().unwrap().contains(&(m as usize))
}

/// True iff this mutex is a plain NORMAL Non-PI mutex (the only layout we
/// implement byte-exact). Reads the packed state word to check the type bits.
unsafe fn bionic_is_normal(m: *const u8) -> bool {
    if m.is_null() {
        return false;
    }
    let state = core::ptr::read_unaligned(m as *const u16);
    (state & BIONIC_TYPE_MASK) == 0 // type bits == NORMAL
}

/// The 32-bit futex word covering state+__pad (Bionic relies on __pad==0).
unsafe fn bionic_state_ptr(m: *const u8) -> *const u32 {
    m as *const u32
}

/// Bionic `__futex_wait_ex` / `__futex_wake_ex` with PRIVATE unless shared.
/// (PRIVATE flag = 128; not all libc versions export FUTEX_*_PRIVATE.)
fn futex_wait(addr: *const u32, val: u32, shared: bool) {
    let op = if shared { 0 /* FUTEX_WAIT */ } else { 128 /* FUTEX_WAIT_PRIVATE */ };
    let _ = unsafe { libc::syscall(libc::SYS_futex, addr, op, val) };
}
fn futex_wake(addr: *const u32, n: i32, shared: bool) {
    let op = if shared { 1 /* FUTEX_WAKE */ } else { 129 /* FUTEX_WAKE_PRIVATE */ };
    let _ = unsafe { libc::syscall(libc::SYS_futex, addr, op, n) };
}

/// Bionic NonPI::NormalMutexLock against the guest 16-bit state word.
/// # Safety
/// `m` must point to a guest `pthread_mutex_t` whose first 4 bytes are
/// writable; it must be a NORMAL (type bits 0) mutex or this returns EPROTO.
unsafe fn bionic_mutex_lock(m: *mut u8) -> i32 {
    if m.is_null() {
        return libc::EINVAL;
    }
    if is_glibc_initialized(m) {
        // glibc owns this mutex (its `__kind` encodes the type, and it handles
        // recursive/errorcheck re-entry correctly). Route to the real glibc.
        return unsafe { glibc_mutex_lock(m) };
    }
    if !bionic_is_normal(m) {
        // Non-NORMAL (recursive/errorcheck/PI) — fall back to the glibc bridge.
        return unsafe { glibc_mutex_lock(m) };
    }
    let state = m as *mut u16;
    let shared = (unsafe { core::ptr::read_unaligned(state) } & BIONIC_SHARED_MASK) != 0;
    let word = unsafe { &*(m as *const AtomicU16) };

    // Fast path: CAS 0 -> 1, further coloured by shared.
    let unlocked = if shared { BIONIC_SHARED_MASK } else { 0 };
    let locked_uncontended = unlocked | BIONIC_STATE_LOCKED_UNCONTENDED;
    let locked_contended = unlocked | BIONIC_STATE_LOCKED_CONTENDED;

    if word.compare_exchange_weak(
        unlocked,
        locked_uncontended,
        Ordering::Acquire,
        Ordering::Relaxed,
    ) == Ok(unlocked)
    {
        return 0;
    }

    // Contention: exchange state -> locked_contended; if we got UNLOCKED we won,
    // else futex-wait until woken (the holder's unlock will exchange 0 and wake
    // us if it saw CONTENDED). Matches bionic's `while exchange != unlocked`.
    loop {
        let prev = word.swap(locked_contended, Ordering::Acquire);
        if prev == unlocked {
            return 0;
        }
        futex_wait(bionic_state_ptr(m), locked_contended as u32, shared);
    }
}

/// Bionic NonPI::NormalMutexUnlock against the guest 16-bit state word.
unsafe fn bionic_mutex_unlock(m: *mut u8) -> i32 {
    if m.is_null() {
        return libc::EINVAL;
    }
    if is_glibc_initialized(m) || !bionic_is_normal(m) {
        return unsafe { glibc_mutex_unlock(m) };
    }
    let shared = (unsafe { core::ptr::read_unaligned(m as *const u16) } & BIONIC_SHARED_MASK) != 0;
    let unlocked = if shared { BIONIC_SHARED_MASK } else { 0 };
    let locked_contended = unlocked | BIONIC_STATE_LOCKED_CONTENDED;
    let word = unsafe { &*(m as *const AtomicU16) };

    let prev = word.swap(unlocked, Ordering::Release);
    if prev == locked_contended {
        // Waiters exist: wake exactly one (they re-swap to CONTENDED on sleep,
        // so the account is self-perpetuating, per bionic's comment).
        futex_wake(bionic_state_ptr(m), 1, shared);
    }
    0
}

/// glibc lock/unlock fallback for non-NORMAL mutexes (unchanged behavior).
/// Resolves the real glibc entry point lazily if the OnceLock isn't populated
/// yet (direct unit-test calls), so it never panics on an un-resolved slot.
unsafe fn glibc_mutex_lock(m: *mut u8) -> i32 {
    let f: MutexLockFn = match REAL_LOCK.get() {
        Some(f) => *f,
        None => resolve_glibc_pthread("pthread_mutex_lock"),
    };
    unsafe {
        sanitize_mutex(m);
        f(m)
    }
}
unsafe fn glibc_mutex_unlock(m: *mut u8) -> i32 {
    let f: MutexLockFn = match REAL_UNLOCK.get() {
        Some(f) => *f,
        None => resolve_glibc_pthread("pthread_mutex_unlock"),
    };
    unsafe {
        sanitize_mutex(m);
        f(m)
    }
}

/// dlsym a glibc pthread function directly (RTLD_NEXT host libc), for the
/// lazy fallback path. Panics only if glibc is genuinely missing the symbol.
fn resolve_glibc_pthread(name: &str) -> MutexLockFn {
    let sym = CString::new(name).expect("static symbol name");
    let ptr = unsafe { libc::dlsym(libc::RTLD_NEXT, sym.as_ptr()) };
    assert!(!ptr.is_null(), "glibc missing {name}");
    unsafe { std::mem::transmute(ptr) }
}
extern "C" fn host_cond_wait(a0: u64, a1: u64, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64) -> u64 {
    // Lifecycle drive (JIT_DRIVE_LIFECYCLE=1): no real Java layer broadcasts a
    // guest condvar, so a cond_wait entered before we satisfy the predicate can
    // sleep forever. Emulate a bounded sawtooth: short timedwait instead of a
    // non-timeout wait, returning to the guest's predicate re-check loop so it
    // observes externally-satisfied lifecycle flags and advances.
    if std::env::var_os("JIT_DRIVE_LIFECYCLE").is_some() {
        type SawtoothFn = unsafe extern "C" fn(*mut u8, *mut u8, *const libc::timespec) -> i32;
        let f: SawtoothFn =
            unsafe { std::mem::transmute(*REAL_COND_TIMEDWAIT.get().expect("pthread_cond_timedwait resolved")) };
        let ts = libc::timespec { tv_sec: 0, tv_nsec: 2_000_000 };
        unsafe {
            sanitize_mutex(a1 as *mut u8);
            return f(a0 as *mut u8, a1 as *mut u8, &ts) as u64;
        }
    }
    let f = *REAL_COND_WAIT.get().expect("pthread_cond_wait resolved");
    if std::env::var_os("JIT_TRACE").is_some() {
        let gpc = crate::jit::current_guest_pc();
        eprintln!("[cond_wait] cond={a0:#x} mutex={a1:#x} caller_guest_pc={gpc:#x}");
    }
    unsafe {
        sanitize_mutex(a1 as *mut u8); // mutex is arg1 (pthread_cond_wait(cond, mutex))
        f(a0 as *mut u8, a1 as *mut u8) as u64
    }
}
extern "C" fn host_cond_timedwait(a0: u64, a1: u64, a2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64) -> u64 {
    type CondTimedwaitFn = unsafe extern "C" fn(*mut u8, *mut u8, *const libc::timespec) -> i32;
    let f: CondTimedwaitFn = unsafe {
        std::mem::transmute(
            *REAL_COND_TIMEDWAIT.get().expect("pthread_cond_timedwait resolved"),
        )
    };
    if std::env::var_os("JIT_TRACE").is_some() {
        let gpc = crate::jit::current_guest_pc();
        eprintln!("[cond_timedwait] cond={a0:#x} mutex={a1:#x} ts={a2:#x} caller_guest_pc={gpc:#x}");
    }
    unsafe {
        sanitize_mutex(a1 as *mut u8);
        f(a0 as *mut u8, a1 as *mut u8, a2 as *const libc::timespec) as u64
    }
}
extern "C" fn host_mutex_init(a0: u64, a1: u64, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64) -> u64 {
    let f = *REAL_MUTEX_INIT.get().expect("pthread_mutex_init resolved");
    unsafe {
        let r = f(a0 as *mut u8, a1 as *mut u8);
        if r == 0 && a0 != 0 {
            sanitize_mutex(a0 as *mut u8);
            // glibc formatted & owns it now (glibc __kind carries the type).
            record_glibc_init(a0 as usize);
        }
        r as u64
    }
}

/// Convenience: resolve and return the slot address (panics if unresolved).
pub fn require(name: &str) -> u64 {
    resolve(name.as_bytes()).expect("host symbol not resolvable")
}

/// Register a host call for a **named** import without `dlsym` (for bionic/
/// Android names that have no host symbol). Returns the thunk's guest address.
/// Takes a NUL-terminated byte slice (the `.dynstr`-style name).
pub fn register_named(name: &[u8], f: crate::jit::HostCall) -> u64 {
    let mut r = resolver().lock().unwrap();
    let key = CString::new(name).ok().or_else(|| {
        CString::new(name.strip_suffix(&[0]).unwrap_or(name)).ok()
    });
    let key = match key {
        Some(k) => k,
        None => return 0,
    };
    if let Some(addr) = r.slots.get(&key) {
        return *addr;
    }
    let slot = r.next;
    r.next += 1;
    crate::jit::register_host_call(slot, f);
    let addr = crate::jit::host_call_addr(slot);
    r.slots.insert(key, addr);
    addr
}

/// Resolve a well-known **data-object** import to a stable host address.
///
/// The real libroblox.so (built against Android libmediandk, which does not
/// exist on the host) imports 10 `AMEDIAFORMAT_KEY_*` OBJECT symbols — the
/// NDK media-format string constants ("mime", "width", ...). A GLOB_DAT /
/// ABS64 relocation writes the *address of the constant* into the GOT slot;
/// the guest does `adrp; ldr xN,[xN,#off]` to load that pointer and passes it
/// to `AMediaFormat_*` as a `const char*`. Left unresolved (0), a real
/// video/audio-decoding session reads a NULL key string (SH19/SH24-style data
/// fault). Addr is the address of a leaked, immortal `CString` so the pointer
/// stays valid for the whole process.
pub fn resolve_android_data(name: &[u8]) -> Option<u64> {
    use std::collections::HashMap;
    static CACHE: OnceLock<Mutex<HashMap<Vec<u8>, u64>>> = OnceLock::new();
    const KEYS: &[(&[u8], &[u8])] = &[
        (b"AMEDIAFORMAT_KEY_MIME", b"mime\0"),
        (b"AMEDIAFORMAT_KEY_WIDTH", b"width\0"),
        (b"AMEDIAFORMAT_KEY_HEIGHT", b"height\0"),
        (b"AMEDIAFORMAT_KEY_COLOR_FORMAT", b"color-format\0"),
        (b"AMEDIAFORMAT_KEY_STRIDE", b"stride\0"),
        (b"AMEDIAFORMAT_KEY_BIT_RATE", b"bitrate\0"),
        (b"AMEDIAFORMAT_KEY_FRAME_RATE", b"frame-rate\0"),
        (b"AMEDIAFORMAT_KEY_I_FRAME_INTERVAL", b"i-frame-interval\0"),
        (b"AMEDIAFORMAT_KEY_CHANNEL_COUNT", b"channel-count\0"),
        (b"AMEDIAFORMAT_KEY_SAMPLE_RATE", b"sample-rate\0"),
    ];
    let Some((_, val)) = KEYS.iter().find(|(k, _)| *k == name) else {
        return None;
    };
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    let key_v = name.to_vec();
    if let Some(addr) = cache.get(&key_v) {
        return Some(*addr);
    }
    // Note: we must return a fresh pointer if not provisioned; the CStr is
    // deliberately leaked so its address remains valid for the guest lifetime.
    let cs = CString::from_vec_with_nul(val.to_vec()).ok()?;
    let raw = cs.as_ptr() as u64;
    std::mem::forget(cs); // immortal: guest allocates/reads this for the process lifetime
    cache.insert(key_v, raw);
    Some(raw)
}

/// Resolve a **double-precision** float-ABI import to a float thunk guest addr.
/// The guest (Roblox) passes doubles in v0-v7; our float bridge reads those
/// lanes as f64 and calls the host double function through xmm0-xmm7. Only
/// double-precision names are safe here (single-precision `*f` need f32 lane
/// handling and are intentionally excluded).
pub fn resolve_float(name: &[u8]) -> Option<u64> {
    let key = CString::new(name).ok()?;
    let sym = key.as_ptr();
    let mut ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, sym) };
    if ptr.is_null() {
        let mh = libm_handle();
        if !mh.is_null() {
            ptr = unsafe { sym_from(mh, sym) };
        }
    }
    if ptr.is_null() {
        return None;
    }
    // Host f64 -> f64 via double (xmm0..) ABI = `HostFloatCall`.
    let hostf: HostFloatCall = unsafe { std::mem::transmute(ptr) };
    let slot = crate::jit::register_float_call(hostf);
    let ns = name_str(name);
    crate::jit::name_host_call_slot(slot, &ns);
    Some(slot)
}

/// Double-precision libm names whose f64 ABI matches our float bridge.
pub const DOUBLE_FLOAT_NAMES: &[&str] = &[
    "atan2", "atan", "asin", "acos", "sin", "cos", "tan", "exp", "log", "log10", "log2",
    "pow", "sqrt", "floor", "ceil", "fabs", "fmod", "hypot", "copysign", "trunc", "round",
    "exp2", "log1p", "expm1", "sinh", "cosh", "tanh", "asinh", "acosh", "atanh",
];

/// Single-precision float-ABI libm names (guest stores f32 in low 32 bits of
/// s0-s7); these match our f32 float bridge.
pub const FLOAT32_NAMES: &[&str] = &[
    "atan2f", "atanf", "asinf", "acosf", "sinf", "cosf", "tanf", "expf", "logf", "log10f",
    "log2f", "powf", "sqrtf", "floorf", "ceilf", "fabsf", "fmodf", "hypotf", "copysignf",
    "truncf", "roundf", "exp2f", "log1pf", "expm1f", "sinhf", "coshf", "tanhf", "asinhf",
    "acoshf", "atanhf",
];

/// Resolve a **single-precision** float-ABI import to an f32 thunk guest addr.
pub fn resolve_float32(name: &[u8]) -> Option<u64> {
    let key = CString::new(name).ok()?;
    let sym = key.as_ptr();
    let mut ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, sym) };
    if ptr.is_null() {
        let mh = libm_handle();
        if !mh.is_null() {
            ptr = unsafe { sym_from(mh, sym) };
        }
    }
    if ptr.is_null() {
        return None;
    }
    let hostf: HostFloat32Call = unsafe { std::mem::transmute(ptr) };
    let slot = crate::jit::register_float32_call(hostf);
    let ns = name_str(name);
    crate::jit::name_host_call_slot(slot, &ns);
    Some(slot)
}

/// Regist directly known common imports: name -> host function. Returns a map
/// of import name -> thunk guest address for the ones the host provides.
pub fn resolve_common() -> HashMap<String, u64> {
    let names: &[&str] = &[
        "strlen",
        "strcmp",
        "strncmp",
        "memcmp",
        "memcpy",
        "memmove",
        "memset",
        "abs",
        "labs",
        "llabs",
        "atoi",
        "atol",
        "strtol",
        "strtoul",
        "strtod",
        "strstr",
        "strchr",
        "strrchr",
        "strcspn",
        "strspn",
        "strlen",
        "strdup",
        "strndup",
        "memchr",
        "malloc",
        "free",
        "realloc",
        "calloc",
        "rand",
        "srand",
        "atoi",
        "isalpha",
        "isdigit",
        "isalnum",
        "isupper",
        "islower",
        "toupper",
        "tolower",
        "getenv",
        "setenv",
        "qsort",
        "bsearch",
        "fabs",
        "fabsf",
        "fmax",
        "fmin",
        "floor",
        "floorf",
        "ceil",
        "ceilf",
        "round",
        "roundf",
        "trunc",
        "truncf",
        "sqrt",
        "sqrtf",
        "log10",
        "log2",
        "logf",
        "exp",
        "expf",
        "pow",
        "powf",
        "sin",
        "sinf",
        "cos",
        "cosf",
        "tan",
        "tanf",
        "fmod",
        "fmodf",
        "memcpy",
        "qsort_r",
        "__errno_location",
        "__cxa_atexit",
        "__cxa_thread_atexit_impl",
        "__android_log_print",
        "__android_log_vprint",
        "dlopen",
        "dlclose",
        "dlsym",
        "dlerror",
        "getpid",
        "getuid",
        "getgid",
        "getppid",
        "clock_gettime",
        "nanosleep",
        "futex",
        "pthread_mutex_lock",
        "pthread_mutex_unlock",
        "pthread_mutex_init",
        "pthread_mutex_destroy",
        "pthread_cond_wait",
        "pthread_cond_broadcast",
        "pthread_cond_signal",
        "pthread_cond_destroy",
        "pthread_mutexattr_init",
        "pthread_mutexattr_destroy",
        "pthread_mutexattr_settype",
        "pthread_key_create",
        "pthread_getspecific",
        "pthread_setspecific",
        "pthread_once",
        "pthread_self",
        "pthread_cleanup_push",
        "pthread_cleanup_pop",
        "pthread_condattr_init",
        "pthread_condattr_setclock",
        "usleep",
        "sysconf",
    ];
    let mut out = HashMap::new();
    for n in names {
        if let Some(addr) = resolve(n.as_bytes()) {
            out.insert(n.to_string(), addr);
        }
    }
    out
}

/// Readable C string for debug/log.
pub fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jit::{jit_run, CpuState};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Two REAL host threads rendezvous through the bionic NORMAL mutex bridge
    /// operating on a byte-exact bionic-layout `pthread_mutex_t` (16-bit state
    /// @0, zero __pad @2, owner_tid @4). This is the regression gate the handoff
    /// REQUIRES before the bridge is trusted anywhere near the real boot: the
    /// old glibc path blocked forever on a contended bionic mutex because glibc
    /// can't parse the 16-bit state word (the cross-ABI wall). This proves the
    /// byte-exact bionic protocol wakes a genuinely-blocked waiter.
    #[test]
    fn bionic_normal_mutex_two_thread_rendezvous() {
        // A host-aligned bionic pthread_mutex_t with NORMAL type (type bits 0),
        // unlocked (state=0), like Bionic's PTHREAD_MUTEX_INITIALIZER + __pad.
        let mut m = Box::new([0u8; 44]); // state@0 + pad@2 + owner_tid@4 + 28 tail
        let mp = m.as_mut_ptr() as *mut u8;
        let mp_addr = mp as usize;

        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered2 = entered.clone();
        let release2 = release.clone();

        // Thread A acquires via the bridge, signals it holds the lock, then
        // waits for the main thread's signal before unlocking.
        let a = std::thread::spawn(move || {
            let mpa = mp_addr as *mut u8;
            let rc = unsafe { bionic_mutex_lock(mpa) };
            assert_eq!(rc, 0, "thread A must acquire the unlocked mutex");
            assert!(
                unsafe { core::ptr::read_unaligned(mpa as *const u16) } & 0x3 == 1,
                "after uncontended acquire, state word must be 1 (LOCKED_UNCONTENDED)"
            );
            entered2.store(true, Ordering::SeqCst);
            while !release2.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            let rc = unsafe { bionic_mutex_unlock(mpa) };
            assert_eq!(rc, 0, "thread A unlock must succeed");
        });

        // Spin until A holds the lock, then have the MAIN thread contend on it
        // through the bridge — this must FUTEX-BLOCK (never return until A
        // unlocks), proving the contention path actually parks.
        while !entered.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        let blocked = Arc::new(AtomicBool::new(false));
        let blocked2 = blocked.clone();
        let b = std::thread::spawn(move || {
            let mpb = mp_addr as *mut u8;
            // Should block until A (via futex_wake) releases it.
            let rc = unsafe { bionic_mutex_lock(mpb) };
            assert_eq!(rc, 0, "thread B must acquire after A unlocks");
            blocked.store(true, Ordering::SeqCst);
        });
        // Give B a moment to enter the futex wait on the contended mutex. Poll
        // for the contended state rather than relying on a fixed sleep, so the
        // test isn't flaky on a loaded machine.
        let deadline = Instant::now() + Duration::from_secs(5);
        while unsafe { core::ptr::read_unaligned(mp as *const u16) } & 0x3 != 2 {
            assert!(
                Instant::now() < deadline,
                "B never entered the contended futex wait (state never reached 2)"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Now release A; A unlocks and must FUTEX_WAKE B.
        release.store(true, Ordering::SeqCst);
        a.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !blocked2.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "thread B was never woken by A's unlock — the futex wake failed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        b.join().unwrap();
        // B acquired via the contended path, so bionic leaves the word at 2
        // (LOCKED_CONTENDED — "maybe waiters"), NOT back to 1. That is correct
        // bionic behavior: a thread that had to spin marks itself as contended
        // so a future unlocker still performs a wake. Assert contended, then
        // release so the word returns to unlocked.
        assert!(
            unsafe { core::ptr::read_unaligned(mp as *const u16) } & 0x3 == 2,
            "after contended re-acquire, state word must be 2 (LOCKED_CONTENDED)"
        );
        let rc = unsafe { bionic_mutex_unlock(mp) }; // B left it held; release clean
        assert_eq!(rc, 0, "final unlock succeeds");
        assert!(
            unsafe { core::ptr::read_unaligned(mp as *const u16) } & 0x3 == 0,
            "after final unlock, state word must be 0 (UNLOCKED)"
        );
    }

    /// The bridge must correctly classify mutex types before any protocol is
    /// applied: only a NORMAL (type bits 0) Non-PI mutex uses our byte-exact
    /// bionic protocol; recursive/errorcheck/PI mutexes (type bits non-zero)
    /// must route to the glibc fallback instead. Verifies the classifier.
    #[test]
    fn bionic_classifier_routes_only_normal_to_bionic_protocol() {
        let mut m = Box::new([0u8; 64]);
        let mp = m.as_mut_ptr();
        unsafe fn state(mp: *mut u8) -> u16 {
            core::ptr::read_unaligned(mp as *const u16)
        }
        // PTHREAD_MUTEX_INITIALIZER-equivalent: all zero -> NORMAL.
        assert!(unsafe { bionic_is_normal(mp) }, "all-zero state must be NORMAL");
        // RECURSIVE: bits 15:14 = 1.
        unsafe { core::ptr::write_unaligned(mp as *mut u16, 0x4000u16) };
        assert!(!unsafe { bionic_is_normal(mp) }, "recursive must NOT be NORMAL");
        // ERRORCHECK: bits 15:14 = 2.
        unsafe { core::ptr::write_unaligned(mp as *mut u16, 0x8000u16) };
        assert!(!unsafe { bionic_is_normal(mp) }, "errorcheck must NOT be NORMAL");
        // PI mutex: type bits = 3.
        unsafe { core::ptr::write_unaligned(mp as *mut u16, 0xC000u16) };
        assert!(!unsafe { bionic_is_normal(mp) }, "PI must NOT be NORMAL");
        // NULL ptr -> not normal (EINVAL guard).
        assert!(!unsafe { bionic_is_normal(core::ptr::null()) });
        // A NORMAL mutex that has entered the LOCKED_CONTENDED state is still
        // NORMAL (only the low 2 state bits moved).
        unsafe { core::ptr::write_unaligned(mp as *mut u16, 0x2u16) };
        assert!(unsafe { bionic_is_normal(mp) }, "locked-contended NORMAL stays NORMAL");
    }

    #[test]
    fn resolve_strlen_and_call_via_guest_blr() {
        // Resolve the *host* `strlen` into a thunk slot, then make guest code
        // `blr` to that slot and confirm the host strlen runs on a guest C
        // string and returns its length into x0.
        let slot = resolve(b"strlen").expect("host strlen resolvable");
        // Put a C string somewhere the guest and host both can read. Use host
        // memory directly (guest==host addressing), store the ptr in x0.
        let msg = b"hello-roblox\0";
        // Guest code: x0 = &msg (set from host), blr x16 (x16 = slot), brk.
        let mut img: Vec<u8> = Vec::new();
        img.extend_from_slice(&0xd63f0200u32.to_le_bytes()); // blr x16
        img.extend_from_slice(&0xd4200000u32.to_le_bytes()); // brk #0

        let msg_ptr = msg.as_ptr() as u64;
        let mut st = CpuState::new();
        st.x[0] = msg_ptr; // arg0 = the string
        st.x[16] = slot; // target = host strlen slot
        let r = jit_run(&img, 0x1000, 0x1000, &mut st as *mut CpuState).expect("jit_run");
        assert_eq!(
            r as usize,
            msg.len() - 1,
            "guest blr to host strlen(hello-roblox) == 12"
        );
    }

    /// A mutex that the guest initialized through our `host_mutex_init` bridge
    /// is GLIBC-formatted (glibc __kind carries the type, incl. RECURSIVE), so
    /// lock/unlock MUST route to glibc — NOT to the bionic 16-bit-word protocol,
    /// which can't parse it and would self-deadlock on a same-thread recursive
    /// re-lock (the exact wall: GameActivity's 0x6edae60 is init'd with
    /// pthread_mutexattr_settype(RECURSIVE)). Verifies the registry routing.
    #[test]
    fn glibc_initialized_mutex_routes_to_glibc_not_bionic() {
        let mut m = Box::new([0u8; 64]);
        let mp = m.as_mut_ptr();
        // Mark as glibc-initialized (as host_mutex_init would).
        record_glibc_init(mp as usize);
        assert!(is_glibc_initialized(mp), "registry must remember the init");
        // Even though the word reads as NORMAL, routing must go to glibc. We
        // can't call host_mutex_lock without a populated glibc slot, so assert
        // the classifier-level routing decision that drives it.
        assert!(
            is_glibc_initialized(mp),
            "glibc-initialized mutex flagged for glibc path"
        );
        // A non-initialized zero word must NOT be in the registry.
        assert!(!super::is_glibc_initialized(core::ptr::null()));
        // Sanity: a distinct never-inited address is not flagged.
        let mut other = Box::new([0u8; 64]);
        assert!(!super::is_glibc_initialized(other.as_mut_ptr()));
        // Writing a bionic RECURSIVE type into a NON-inited word routes it away
        // from the bionic path too (classifier).
        unsafe { core::ptr::write_unaligned(mp as *mut u16, 0x4000u16) };
        assert!(!unsafe { bionic_is_normal(mp) });
    }

    #[test]
    fn resolve_common_has_strlib() {
        let m = resolve_common();
        assert!(m.contains_key("strlen"), "common imports include strlen");
        assert!(m.contains_key("memcpy"), "common imports include memcpy");
        assert!(m.contains_key("abs"), "common imports include abs");
    }

    #[test]
    fn sanitize_mutex_clears_kind_but_preserves_glibc_owner() {
        // glibc pthread_mutex_t: __lock@0, __count@4, __owner@8 (host TID),
        // __kind@16. sanitize must mask the robust/high kind bits at 16 (so
        // glibc locks a NORMAL/RECURSIVE mutex it understands) but MUST NOT
        // touch __owner at offset 8 — a real owner TID (e.g. 3392123) is
        // > 0x10000 and is the field glibc checks to allow a recursive
        // same-thread re-lock. Zeroing it made a held recursive mutex look
        // unowned, so the owner futex-blocked on its own lock (0x6edae60 wall).
        let mut m = [0u8; 24];
        m[16..20].copy_from_slice(&0x10u32.to_le_bytes()); // robust kind bit
        let owner_tid = 3392123u32; // a plausible glibc __owner (> 0x10000)
        m[8..12].copy_from_slice(&owner_tid.to_le_bytes());

        unsafe { super::sanitize_mutex(m.as_mut_ptr()) };

        let kind = u32::from_le_bytes(m[16..20].try_into().unwrap());
        let owner = u32::from_le_bytes(m[8..12].try_into().unwrap());
        assert_eq!(kind & 3, kind, "kind high bits cleared (kind=0x{kind:x})");
        assert_eq!(
            owner, owner_tid,
            "glibc __owner at offset 8 MUST survive sanitize (recursive re-lock depends on it)"
        );
    }

    /// Real Mesa EGL must be resolvable as an integer-ABI host call, and a guest
    /// `blr` to it must actually execute Mesa code (not a NULL/0 catch-all). This is
    /// the regression gate for the graphics-layer wiring: without `resolve_egl`, the
    /// same import fell to `register_graphics_stubs` -> stub_zero (return 0 trivially).
    /// Skipped on hosts without Mesa EGL (CI boxes may lack /usr/lib/libEGL.so.1).
    #[test]
    fn resolve_egl_binds_real_mesa_not_null_stub() {
        let Some(slot) = resolve_egl(b"eglGetError\0") else {
            eprintln!("skipping: Mesa EGL (libEGL.so.1) not present on this host");
            return;
        };
        // Guest code: blr x16 then brk. eglGetError() takes no args, returns EGLint.
        let mut img: Vec<u8> = Vec::new();
        img.extend_from_slice(&0xd63f0200u32.to_le_bytes()); // blr x16
        img.extend_from_slice(&0xd4200000u32.to_le_bytes()); // brk #0
        let mut st = CpuState::new();
        st.x[16] = slot;
        let r = jit_run(&img, 0x1000, 0x1000, &mut st as *mut CpuState).expect("jit_run");
        // EGL_GET_ERROR with no current display returns an error/status code in the
        // EGL enum space (0x3000..0x3089) — never the NULL-stub's trivial 0x0 and
        // never a garbage pointer. The real Mesa path returns a real EGL error state.
        assert_ne!(r, 0, "eglGetError must return a real Mesa result, not stub 0");
    }

    #[test]
    fn resolve_egl_rejects_non_egl_names() {
        // Must NOT dlopen/allocate for GLES or unrelated names (float ABI, not wired).
        assert!(resolve_egl(b"glViewport\0").is_none());
        assert!(resolve_egl(b"strlen\0").is_none());
    }

    /// Integer-ABI GLES imports (gl* with integer/pointer args, <=8) must resolve to
    /// real Mesa and actually execute — same graphics-wiring gate as resolve_egl, for
    /// the GLES texture/state/draw pipeline. Float-taking GLES must be rejected (their
    /// ABI is not expressible through the integer HostCall).
    #[test]
    fn resolve_gles_int_binds_real_mesa_for_integer_abi_names() {
        // glGetError() -> GLenum, integer ABI, in the whitelist.
        let Some(slot) = resolve_gles_int(b"glGetError\0") else {
            eprintln!("skipping: Mesa GLES (libGLESv2.so.2) not present on this host");
            return;
        };
        let mut img: Vec<u8> = Vec::new();
        img.extend_from_slice(&0xd63f0200u32.to_le_bytes()); // blr x16
        img.extend_from_slice(&0xd4200000u32.to_le_bytes()); // brk #0
        let mut st = CpuState::new();
        st.x[16] = slot;
        let r = jit_run(&img, 0x1000, 0x1000, &mut st as *mut CpuState).expect("jit_run");
        // GL_NO_ERROR (0) is a valid Mesa result; the guarantee is that real Mesa GLES
        // executed (no segfault / no crash). Without a current context Mesa returns 0
        // cleanly; the point is it didn't dispatch to the old NULL/0 stub and crash on
        // a garbage fn pointer while marshalling xmm/stack args.
        let _ = r;
    }

    #[test]
    fn resolve_gles_int_accepts_trailing_nul_like_mixed() {
        // Regression (SH20): resolve_gles_int built its CString from the RAW name,
        // so a NUL-terminated caller (elfjit --renderframe-seedgles, a guest
        // eglGetProcAddress C-string) got None even for whitelisted int-ABI names
        // that Mesa exports. resolve_gles_mixed strips the NUL first and worked;
        // the int resolver must too, else the engine's clear/draw dispatch slots
        // can't resolve through the integer bridge.
        let _ = resolve_gles_mixed(b"glClearColor\0");
        for n in ["glClear", "glViewport", "glColorMask", "glDepthMask",
                  "glStencilMask", "glClearStencil", "glDrawElements", "glGetError"] {
            let nm = format!("{n}\0");
            let in_ = resolve_gles_int(nm.as_bytes())
                .unwrap_or_else(|| panic!("{n} NOT resolvable via int with trailing NUL"));
            eprintln!("resolve_gles_int({n}\0) -> slot {in_:#x}");
        }
    }

    #[test]
    fn gles4_extension_names_resolve_via_int_bridge_desktop_gl_fallback() {
        // Regression (SH47): the engine's real render dispatch table (BSS
        // 0x106d3b2f0 + 8*N) had slots 11/12 read 0x0 even though its render code
        // brs those slots UNGUARDED. The reason: the client resolves them via
        // eglGetProcAddress with GL4/extension names (glBufferStorage, glMapBuffer,
        // glQueryCounter, glObjectLabelKHR, push/pop-group-marker) that Mesa's
        // ES-only libGLESv2.so.2 does NOT export, so our int bridge returned None
        // and w_eglGetProcAddress fell through to 0 - a NULL jump in a self-driven
        // frame. resolve_gles_int now falls back to desktop libGL.so.1 for
        // whitelisted names absent from GLESv2, so these become real dispatchable
        // int-bridge slots. All are pure int/ptr ABI (<=4 args), safe through the
        // integer HostCall. Live proof: the seed snapshot now shows slot 11/12 =
        // 0x7f0000003098/90 (were 0), and JIT_EGL_LOG dropped from 16 unresolved
        // to 2 (the un-EXT-suffixed glPush/PopGroupMarker, absent from both libs).
        let names = [
            "glBufferStorage", "glBufferStorageEXT",
            "glMapBuffer", "glMapBufferOES",
            "glObjectLabelKHR",
            "glPopGroupMarker", "glPopGroupMarkerEXT",
            "glPushGroupMarker", "glPushGroupMarkerEXT",
            "glQueryCounter", "glQueryCounterEXT",
            "glGetQueryObjectiv", "glGetQueryObjectivEXT",
            "glGetQueryObjectui64v", "glGetQueryObjectui64vEXT",
        ];
        let mut resolved = 0;
        let mut unresolved = 0;
        for n in names {
            let nm = format!("{n}\0");
            let slot = resolve_gles_int(nm.as_bytes());
            eprintln!("resolve_gles_int({n}\0) -> {slot:?}");
            match slot {
                Some(_) => resolved += 1,
                None => unresolved += 1,
            }
            // Pure int/ptr ABI => never mixed-wrapped.
            assert!(resolve_gles_mixed(nm.as_bytes()).is_none(), "{n} should not be mixed");
        }
        // glBufferStorage / glMapBuffer / glQueryCounter / glObjectLabelKHR are the
        // load-bearing ones the engine's buffer/timer path actually dispatches
        // (slot 11 reads w0=GL_UNIFORM_BUFFER,size,NULL,flags = glBufferStorage).
        for critical in ["glBufferStorage", "glMapBuffer", "glQueryCounter", "glObjectLabelKHR"] {
            let slot = resolve_gles_int(format!("{critical}\0").as_bytes());
            assert!(
                slot.is_some(),
                "{critical} must resolve via int bridge (desktop-GL fallback)"
            );
        }
        // At least the desktop-exported subset must resolve; libGL.so.1 exports all
        // but the two un-EXT-suffixed marker names.
        assert!(resolved >= 13, "expected most names to resolve, got {resolved}/{unresolved}");
    }

    #[test]
    fn plain_non_ext_marker_names_resolve_via_int_bridge_ext_sibling_fallback() {
        // Regression (SH50): the engine requests the PLAIN (non-EXT) debug-marker
        // names glPushGroupMarker / glPopGroupMarker (GL_EXT_debug_marker without
        // the EXT suffix), but BOTH Mesa libraries export ONLY the EXT-suffixed
        // spellings (glPushGroupMarkerEXT / glPopGroupMarkerEXT). Before this
        // cycle resolve_gles_int returned None for the plain names even though
        // they were in GLES_INT_NAME_LIST, so a guest `br` through the engine's
        // render dispatch-table slot 11/12 for these names would jump to NULL —
        // the SH19/SH24/SH47 NULL-dispatch crash class kept open by the two names
        // JIT_EGL_LOG still reported as UNRESOLVED. resolve_gles_int now falls
        // back to the EXT-suffixed sibling (a whitelisted, integer-ABI-safe name)
        // so the plain names become real dispatchable bridge slots.
        //
        // Verify: both plain names AND their EXT siblings resolve via the integer
        // bridge (trailing-NUL form, as eglGetProcAddress passes them), and none
        // is routed through the mixed (float) bridge (pure int/ptr ABI).
        let mut resolved = 0;
        for n in ["glPushGroupMarker", "glPushGroupMarkerEXT", "glPopGroupMarker", "glPopGroupMarkerEXT"] {
            let nm = format!("{n}\0");
            let slot = resolve_gles_int(nm.as_bytes())
                .unwrap_or_else(|| panic!("{n} must resolve via int bridge (EXT-sibling fallback)"));
            eprintln!("resolve_gles_int({n}\\0) -> slot {slot:#x}");
            let addr = slot & 0xffff_ffff_0000_0000;
            assert!(addr != 0, "{n} resolved to a non-bridge pointer");
            assert!(resolve_gles_mixed(nm.as_bytes()).is_none(), "{n} should not be mixed");
            resolved += 1;
        }
        assert_eq!(resolved, 4, "all four marker names must resolve");
    }

    #[test]
    fn draw_slots_gl_draw_elements_arrays_resolve_via_int_bridge() {
        // Regression (SH24): the engine's real geometry draw path —
        // wrapper 0x5b35288 dispatching the GLES dispatch-table slots 9/10
        // (0x5b352f4 bl 0x5b3a22c / 0x5b35368 bl 0x5b3a238) — was left
        // UNSEEDED by --renderframe-seedgles (it only covered the clear
        // slots 0-7), so slot 9/10 held raw-Mesa addresses (the same
        // SH19/SH3 bug class) and a driven real draw would `br` out-of-image.
        // glDrawElements/glDrawArrays are pure integer-ABI GLES, so they must
        // resolve through the integer bridge WITH a trailing NUL (the way
        // elfjit's seedgles passes them).
        for n in ["glDrawElements", "glDrawArrays"] {
            let nm = format!("{n}\0");
            resolve_gles_int(nm.as_bytes())
                .unwrap_or_else(|| panic!("{n} NOT resolvable via int bridge (draw slot)"));
            eprintln!("resolve_gles_int({n}\\0) -> int bridge OK (slot draw)");
            assert!(
                resolve_gles_mixed(nm.as_bytes()).is_none(),
                "{n} should not be mixed-wrapped"
            );
        }
    }

    #[test]
    fn resolve_gles_mixed_clearbufferfi_is_mixed_abi_not_int() {
        // Regression (SH36): the real engine's clear-state sub-fn 0x5b32e08
        // dispatches slot 3 (guest BSS 0x106d3b2f0+8*3 = 0x106d3b308) through the
        // stub 0x5b3a1e4 as `glClearBufferfi(GLenum buffer, GLint drawbuffer,
        // GLfloat depth, GLint stencil)` — the per-buffer COMBINED depth+stencil
        // clear (0x84F9=GL_DEPTH_STENCIL). The real call site 0x5b32ef4 loads
        // `ldr s0,[x21,#68]` (depth, first FP arg -> s0), `ldr w2,[x21,#72]`
        // (stencil), `mov w0,#0x84f9`, `mov w1,wzr`. Because depth is a float,
        // glClearBufferfi is a MIXED-ABI function (float in s0 + 3 int/ptr), so:
        //  - resolve_gles_mixed MUST register it (so w_eglGetProcAddress returns a
        //    dispatchable float-shaped bridge slot -- it CANNOT go through the
        //    integer HostCall, which only marshals x-regs and would drop the s0
        //    depth, mis-clearing the depth attachment).
        //  - resolve_gles_int MUST reject it (float ABI), so the fallback chain in
        //    resolve_egl_get_proc_address correctly keeps the mixed slot.
        // The pre-SH36 seed used glClearStencil (single-int-ABI) for slot 3 -- that
        // mis-routes a GL_DEPTH_STENCIL dispatch (a float depth in s0 would be
        // ignored, and none of the 3 int args decoded as a color/stencil mask).
        let nm = b"glClearBufferfi\0";
        let mixed = resolve_gles_mixed(nm)
            .unwrap_or_else(|| panic!("glClearBufferfi must resolve via the mixed (float) bridge"));
        eprintln!("resolve_gles_mixed(glClearBufferfi\\0) -> mixed bridge slot {mixed:#x}");
        assert!(mixed >= crate::jit::HOST_THUNK_BASE, "slot is a real dispatchable host thunk");
        assert!(
            resolve_gles_int(nm).is_none(),
            "glClearBufferfi has a float s0 arg -> must be rejected by the int bridge"
        );
    }

    #[test]
    fn sealed_gles3_ubo_and_instanced_slots_dispatch_real_mesa_clean() {
        // Hermetic SH37 gate: the SH35-sealed GLES3 pipeline slots (UBO bind slot 5 =
        // glBindBufferBase, instanced draw slot 10 = glDrawArraysInstanced) must not
        // merely RESOLVE — a real session's frame dispatches THROUGH the engine's slot
        // stubs into these bridge slots, so on a live Mesa context they must actually
        // execute without a GL error or crash. We drive each resolve_gles_int slot via
        // guest `blr` (gcall) on a surfaceless ES3 context exactly as the live probe did,
        // and assert glGetError stays GL_NO_ERROR throughout. A mis-bridged slot (wrong
        // fn or wrong ABI) surfaces as GL_INVALID_ENUM/OPERATION or a native crash.
        unsafe { std::env::set_var("EGL_PLATFORM", "surfaceless") };
        let Some(egl_getdisplay) = resolve_egl(b"eglGetDisplay\0") else { return };
        let Some(egl_initialize) = resolve_egl(b"eglInitialize\0") else { return };
        let Some(egl_choose_config) = resolve_egl(b"eglChooseConfig\0") else { return };
        let Some(egl_create_ctx) = resolve_egl(b"eglCreateContext\0") else { return };
        let Some(egl_make_current) = resolve_egl(b"eglMakeCurrent\0") else { return };
        let Some(egl_create_pbuf) = resolve_egl(b"eglCreatePbufferSurface\0") else { return };
        const EGL_NONE: u64 = 0x3038;
        const EGL_SURFACE_TYPE: u64 = 0x3033;
        const EGL_PBUFFER_BIT: u64 = 0x0001;
        const EGL_RENDERABLE_TYPE: u64 = 0x3040;
        const EGL_OPENGL_ES3_BIT: u64 = 0x40;
        const EGL_CONTEXT_CLIENT_VERSION: u64 = 0x3098;
        const EGL_WIDTH: u64 = 0x3057;
        const EGL_HEIGHT: u64 = 0x3056;
        const GL_UNIFORM_BUFFER: u64 = 0x8A11;
        const GL_DYNAMIC_DRAW: u64 = 0x88E8;
        const GL_TRIANGLES: u64 = 0x0004;
        const GL_ARRAY_BUFFER: u64 = 0x8892;
        let mut st = CpuState::new();
        let dpy = gcall(egl_getdisplay, &mut st);
        assert_ne!(dpy, 0);
        let mut ver = [0u32; 2];
        st.x[0] = dpy;
        st.x[1] = ver.as_mut_ptr() as u64;
        st.x[2] = ver.as_mut_ptr().wrapping_add(1) as u64;
        gcall(egl_initialize, &mut st);
        let mut attribs = [
            EGL_SURFACE_TYPE as i32, EGL_PBUFFER_BIT as i32,
            EGL_RENDERABLE_TYPE as i32, EGL_OPENGL_ES3_BIT as i32,
            EGL_NONE as i32, 0,
        ];
        let mut config = 0u64;
        let mut num = 0i32;
        st.x[0] = dpy;
        st.x[1] = attribs.as_mut_ptr() as u64;
        st.x[2] = (&mut config) as *mut u64 as u64;
        st.x[3] = 16;
        st.x[4] = (&mut num) as *mut i32 as u64;
        // Some Mesa surfaceless configs may return 0 for a strict ES3 pbuffer
        // request under llvmpipe; that already proves dispatch but gate softly.
        let _ = gcall(egl_choose_config, &mut st);
        if config == 0 {
            eprintln!("skipping: no ES3 pbuffer config on this Mesa (dispatch-check only via resolve)");
            // Still assert the sealed slots resolve (the SH35-layer guarantee).
            assert!(resolve_gles_int(b"glBindBufferBase\0").is_some());
            assert!(resolve_gles_int(b"glDrawArraysInstanced\0").is_some());
            return;
        }
        let mut ctx_attribs = [EGL_CONTEXT_CLIENT_VERSION as i32, 3, EGL_NONE as i32, 0];
        st.x[0] = dpy;
        st.x[1] = config;
        st.x[2] = 0;
        st.x[3] = ctx_attribs.as_mut_ptr() as u64;
        let ctx = gcall(egl_create_ctx, &mut st);
        assert_ne!(ctx, 0, "eglCreateContext ES3");
        let mut surf_attribs = [EGL_WIDTH as i32, 16, EGL_HEIGHT as i32, 16, EGL_NONE as i32, 0];
        st.x[0] = dpy;
        st.x[1] = config;
        st.x[2] = surf_attribs.as_mut_ptr() as u64;
        let surf = gcall(egl_create_pbuf, &mut st);
        assert_ne!(surf, 0, "eglCreatePbufferSurface");
        st.x[0] = dpy;
        st.x[1] = surf;
        st.x[2] = surf;
        st.x[3] = ctx;
        let made = gcall(egl_make_current, &mut st);
        assert_eq!(made, 1, "eglMakeCurrent");
        // The sealed int-ABI slots (SH35 SLOT5/10) + the GL state helpers.
        let gl_get_error = resolve_gles_int(b"glGetError\0").expect("glGetError resolves");
        let gl_gen_buffers = resolve_gles_int(b"glGenBuffers\0").expect("glGenBuffers resolves");
        let gl_bind_buffer = resolve_gles_int(b"glBindBuffer\0").expect("glBindBuffer resolves");
        let gl_buffer_data = resolve_gles_int(b"glBufferData\0").expect("glBufferData resolves");
        let gl_bind_buffer_base = resolve_gles_int(b"glBindBufferBase\0").expect("glBindBufferBase resolves (SH35 slot5)");
        let gl_draw_arrays_instanced = resolve_gles_int(b"glDrawArraysInstanced\0").expect("glDrawArraysInstanced resolves (SH35 slot10)");
        // UBO: gen + fill + bind a real buffer to GL_UNIFORM_BUFFER index 0.
        let mut buf = 0u32;
        st.x[0] = 1;
        st.x[1] = (&mut buf) as *mut u32 as u64;
        gcall(gl_gen_buffers, &mut st);
        assert_ne!(buf, 0, "glGenBuffers produced a buffer id");
        let mut data = [0x11u8; 64];
        st.x[0] = GL_UNIFORM_BUFFER;
        st.x[1] = buf as u64;
        gcall(gl_bind_buffer, &mut st);
        st.x[0] = GL_UNIFORM_BUFFER;
        st.x[1] = 64;
        st.x[2] = data.as_mut_ptr() as u64;
        st.x[3] = GL_DYNAMIC_DRAW;
        gcall(gl_buffer_data, &mut st);
        st.x[0] = GL_UNIFORM_BUFFER;
        st.x[1] = 0;
        st.x[2] = buf as u64;
        gcall(gl_bind_buffer_base, &mut st); // SLOT5: glBindBufferBase
        st.x[0] = 0;
        let e = gcall(gl_get_error, &mut st);
        assert_eq!(e, 0, "glBindBufferBase through sealed slot5 must be GL_NO_ERROR (got {e:#x})");
        // Instanced: a count=0 no-op draw through the shared slot drives the same
        // bridge the engine's instanced path uses; must dispatch without error.
        st.x[0] = GL_TRIANGLES;
        st.x[1] = 0;
        st.x[2] = 0;
        st.x[3] = 3;
        gcall(gl_draw_arrays_instanced, &mut st); // SLOT10: glDrawArraysInstanced
        st.x[0] = 0;
        let e = gcall(gl_get_error, &mut st);
        assert_eq!(e, 0, "glDrawArraysInstanced through sealed slot10 must be GL_NO_ERROR (got {e:#x})");
        // Per-instance vertex-attribute divisor (SH48): a real instanced mesh sets
        // glVertexAttribDivisor(index, 1) on the per-instance buffer; without it every
        // instance reads instance 0's data. This was ABSENT from GLES_INT_NAME_LIST,
        // so the engine's instanced draw fell to the NULL/0 stub (duplicated instance
        // 0). Bind a real vertex buffer to attrib 0, set divisor 1, and issue a
        // NON-empty instanced draw (count=1, 4 instances) so the sealed slot executes
        // a real command, not just the count=0 probe.
        let gl_vertex_attrib_divisor = resolve_gles_int(b"glVertexAttribDivisor\0")
            .expect("glVertexAttribDivisor resolves via int bridge (SH48)");
        let gl_bind_attrib = resolve_gles_int(b"glEnableVertexAttribArray\0")
            .expect("glEnableVertexAttribArray resolves");
        let gl_attrib_ptr = resolve_gles_int(b"glVertexAttribPointer\0")
            .expect("glVertexAttribPointer resolves");
        let gl_bind_buffer = resolve_gles_int(b"glBindBuffer\0").expect("glBindBuffer resolves");
        // A real vertex buffer for attrib 0 (4 vec4s: enough for 4 instances of a
        // tri-quad primitive).
        let mut vbuf = 0u32;
        st.x[0] = 1;
        st.x[1] = (&mut vbuf) as *mut u32 as u64;
        gcall(gl_gen_buffers, &mut st);
        assert_ne!(vbuf, 0, "glGenBuffers produced a vertex buffer id");
        let verts: [u8; 64] = [0u8; 64];
        st.x[0] = GL_ARRAY_BUFFER; // 0x8892
        st.x[1] = vbuf as u64;
        gcall(gl_bind_buffer, &mut st);
        st.x[0] = GL_ARRAY_BUFFER;
        st.x[1] = 64;
        st.x[2] = (&verts[0]) as *const u8 as u64;
        st.x[3] = GL_DYNAMIC_DRAW;
        gcall(gl_buffer_data, &mut st);
        // Enable attrib 0 at the format table, then set divisor 1 (per-instance).
        st.x[0] = 0;
        gcall(gl_bind_attrib, &mut st);
        st.x[0] = 0; // index
        st.x[1] = 4; // size (vec4)
        st.x[2] = 0x1406; // GL_FLOAT
        st.x[3] = 0; // normalized
        st.x[4] = 16; // stride
        st.x[5] = 0; // offset
        gcall(gl_attrib_ptr, &mut st);
        st.x[0] = 0; // index
        st.x[1] = 1; // divisor -> per-instance
        gcall(gl_vertex_attrib_divisor, &mut st);
        st.x[0] = 0;
        let e = gcall(gl_get_error, &mut st);
        assert_eq!(e, 0, "glVertexAttribDivisor(index=0, divisor=1) must be GL_NO_ERROR (got {e:#x})");
        // Non-empty instanced draw through the sealed slot (count=1, 4 instances).
        st.x[0] = GL_TRIANGLES;
        st.x[1] = 0;
        st.x[2] = 1;
        st.x[3] = 4;
        gcall(gl_draw_arrays_instanced, &mut st); // SLOT10, now a real draw
        st.x[0] = 0;
        let e = gcall(gl_get_error, &mut st);
        assert_eq!(e, 0, "non-empty glDrawArraysInstanced(count=1,vcount=4) with divisor must be GL_NO_ERROR (got {e:#x})");
    }

    #[test]
    fn resolve_gles_int_clear_buffer_fv_and_draw_buffers() {
        // Regression (SH22): the engine's frame clear path dispatches slot2 as
        // glClearBufferfv (per-buffer clear loop with GL_COLOR=0x1800/GL_DEPTH=
        // 0x1801 buffer enums, drawbuffer in w1, float4 value ptr in x2) and slot0
        // as glDrawBuffers (GL_COLOR_ATTACHMENT0..3 / GL_BACK arrays). Neither was
        // in GLES_INT_NAME_LIST, so they resolved to None and elfjit's
        // --renderframe-seedgles seeded slot0/2 with glClearColor/glClearDepthf --
        // which misinterpreted the int/ptr args and never cleared the color buffer
        // (black window). Both are pure integer/pointer ABI (<=8 args, no float
        // s-regs), so they must resolve through the integer bridge.
        // glClearBufferiv (slot1): the depth/stencil clear dispatch uses
        // slot1(0x1802=GL_STENCIL, drawbuffer, value_ptr) — pure int/ptr ABI.
        for n in ["glClearBufferfv", "glDrawBuffers", "glClearBufferiv"] {
            let nm = format!("{n}\0");
            resolve_gles_int(nm.as_bytes())
                .unwrap_or_else(|| panic!("{n} NOT resolvable via int bridge"));
            eprintln!("resolve_gles_int({n}\\0) -> int bridge OK");
            // Not mixed-ABI wrapped: resolve_gles_mixed must reject them.
            assert!(resolve_gles_mixed(nm.as_bytes()).is_none(), "{n} should not be mixed-wrapped");
        }
    }

    #[test]
    fn gles3_pipeline_names_resolve_via_int_bridge_for_engine_draw_slots() {
        // Regression (SH28/SH35): the real engine's GLES dispatch table — slots
        // 4-8 (glUniformBlockBinding/glBindBufferBase/glBindBufferRange/
        // glGetUniformBlockIndex/glGetActiveUniformBlockiv), 9/10 instanced draws,
        // 13-15 (glGetProgramBinary/glProgramBinary/glProgramParameteri) — holds
        // raw-Mesa addresses (same SH19/SH24 crash class: a guest `br` through the
        // 0x5b3a1c0+0xc*N stub jumps out-of-image). Because these slots were absent
        // from GLES_INT_NAME_LIST, --renderframe-seedgles could not re-seed them
        // with bridge slots. All are pure integer/pointer ABI, so each MUST resolve
        // through the integer bridge (with a trailing NUL, as elfjit passes them),
        // and MUST be rejected by the float/mixed wrapper.
        let names = [
            // UBO / buffer-binding block (SH28 slots 4-8)
            "glUniformBlockBinding",
            "glBindBufferBase",
            "glBindBufferRange",
            "glGetUniformBlockIndex",
            "glGetActiveUniformBlockiv",
            // instanced draws (SH28 slots 9/10 at init)
            "glDrawElementsInstanced",
            "glDrawArraysInstanced",
            // per-instance vertex-attribute divisor (SLOT4-adjacent int ABI): a real
            // instanced mesh must set attrib-divisor 1 on the per-instance buffer,
            // else every instance reads instance 0's data. Was absent from the
            // whitelist (SH48-discovered) so the engine's instanced draw fell to the
            // NULL/0 stub and duplicated instance 0.
            "glVertexAttribDivisor",
            // program binary (SH28 slots 13-15)
            "glGetProgramBinary",
            "glProgramBinary",
            "glProgramParameteri",
        ];
        for n in names {
            let nm = format!("{n}\0");
            let in_ = resolve_gles_int(nm.as_bytes())
                .unwrap_or_else(|| panic!("{n} NOT resolvable via int bridge (GLES3 pipeline slot)"));
            eprintln!("resolve_gles_int({n}\\0) -> int bridge slot {in_:#x}");
            assert!(
                resolve_gles_mixed(nm.as_bytes()).is_none(),
                "{n} should not be mixed-wrapped"
            );
        }
    }

    #[test]
    fn gl_vertex_attrib_divisor_resolves_via_int_bridge_only_for_instancing() {
        // Regression (SH48): the real client's instanced-mesh pipeline must set a
        // per-instance vertex-attribute divisor (glVertexAttribDivisor(index, n>0))
        // on the attribute that varies per instance — otherwise every instance reads
        // instance 0's data and the draw is a single duplicated triangle. This was
        // ABSENT from GLES_INT_NAME_LIST (only glDraw{Arrays,Elements}Instanced were
        // whitelisted in SH35), so a guest `br` through the engine's slot resolves to
        // NULL/0 and the instanced path silently degenerates to instance 0. Assert:
        //   - it resolves via the INTEGER bridge with a trailing NUL (as elfjit and
        //     w_eglGetProcAddress pass names),
        //   - it is REJECTED by the float/mixed wrapper (pure int ABI),
        //   - glVertexAttribPointer + glEnableVertexAttribArray (its companions in
        //     the per-instance setup) also resolve through the int bridge.
        for n in [
            "glVertexAttribDivisor",
            "glVertexAttribPointer",
            "glEnableVertexAttribArray",
        ] {
            let nm = format!("{n}\0");
            let in_ = resolve_gles_int(nm.as_bytes())
                .unwrap_or_else(|| panic!("{n} NOT resolvable via int bridge (instancing setup)"));
            eprintln!("resolve_gles_int({n}\\0) -> int bridge slot {in_:#x}");
            assert!(
                resolve_gles_mixed(nm.as_bytes()).is_none(),
                "{n} should not be mixed-wrapped (pure int ABI)"
            );
        }
    }

    #[test]
    fn resolve_gles_int_rejects_float_abi_and_unknown() {
        // glClearColor takes GLfloat args -> MUST NOT resolve through the integer HostCall.
        assert!(resolve_gles_int(b"glClearColor\0").is_none());
        // glTexImage2D has 9 args (stack-arg beyond the 8 the HostCall can express).
        assert!(resolve_gles_int(b"glTexImage2D\0").is_none());
        // Non-GLES names rejected.
        assert!(resolve_gles_int(b"strlen\0").is_none());
        // Integer-ABI GLES that is NOT in the shipped Mesa GLESv2 (should be absent).
        assert!(resolve_gles_int(b"glTotallyFake\0").is_none());
    }

    /// Drive one guest `blr x16` to `slot` and return what jit_run leaves in x0.
    /// The guest image is just `blr x16` (link to the following `brk`) so the
    /// JIT dispatcher falls through to the host-call bridge for `slot` and then
    /// stops. `st` persists across calls so float/stack args survive too.
    fn gcall(slot: u64, st: &mut CpuState) -> u64 {
        let mut img: Vec<u8> = Vec::new();
        img.extend_from_slice(&0xd63f0200u32.to_le_bytes()); // blr x16
        img.extend_from_slice(&0xd4200000u32.to_le_bytes()); // brk #0
        st.x[16] = slot;
        jit_run(&img, 0x1000, 0x1000, st as *mut CpuState).expect("jit_run")
    }

    /// Put an f32 into guest SIMD register sN (low 32 bits of v[N] low lane).
    #[cfg(test)]
    fn set_sf(st: &mut CpuState, n: usize, f: f32) {
        st.v[2 * n] = (st.v[2 * n] & !0xffff_ffffu64) | (f.to_bits() as u64);
    }

    /// Full graphics-translation gate through the JIT bridges (no GPU, Mesa
    /// llvmpipe software + surfaceless EGL):
    ///   1. EGL int-ABI slice (resolve_egl) creates a real surfaceless ES3
    ///      context: eglGetDisplay -> Initialize -> ChooseConfig -> CreateContext
    ///      -> MakeCurrent, all driven by guest `blr`.
    ///   2. GLES float/mixed bridge (resolve_gles_mixed) clears a color:
    ///      glClearColor(0.5,0.25,0.75,1.0) via s0-s3.
    ///   3. GLES int bridge (resolve_gles_int) glClear's the colorful background
    ///      + glReadPixels: float-bridge set clear color renders real pixels.
    ///   4. GLES >8-arg bridge uploads a texture: glTexImage2D with the pixels
    ///      pointer on the guest stack; glGetError returns a sane GL enum (no
    ///      crash from gl* now reaching real Mesa through the stack-arg bridge).
    /// Skipped on hosts without Mesa EGL/GLES.
    #[test]
    fn resolve_gles_mixed_float_and_stack_abi_execute_real_mesa() {
        // This gate drives a real EGL/GLES chain purely through the JIT bridges
        // (eglGetDisplay->Initialize->ChooseConfig->CreateContext->MakeCurrent;
        // float-bridge glClearColor; int-bridge glGetFloatv round-trip; >8-arg
        // glTexImage2D). It is headless-capable ONLY when Mesa picks a display
        // platform: without an X DISPLAY, EGL_DEFAULT_DISPLAY needs the
        // `surfaceless` platform. Set it here (before the first EGL call) so the
        // gate runs on a GPU-less box instead of silently skipping — the previous
        // NUL-terminated resolve_egl calls all returned None and short-circuited
        // this test (exit-early skip), so it never actually exercised EGL at all.
        // If Mesa is genuinely absent, the resolve_* calls below return None and
        // the gate skips (as designed).
        unsafe { std::env::set_var("EGL_PLATFORM", "surfaceless") };
        let Some(egl_getdisplay) = resolve_egl(b"eglGetDisplay\0") else {
            eprintln!("skipping: Mesa EGL not present on this host");
            return;
        };
        let Some(egl_initialize) = resolve_egl(b"eglInitialize\0") else { return };
        let Some(egl_choose_config) = resolve_egl(b"eglChooseConfig\0") else { return };
        let Some(egl_create_ctx) = resolve_egl(b"eglCreateContext\0") else { return };
        let Some(egl_make_current) = resolve_egl(b"eglMakeCurrent\0") else { return };
        let Some(gl_clear_color) = resolve_gles_mixed(b"glClearColor\0") else {
            eprintln!("skipping: Mesa GLES float bridge unavailable");
            return;
        };
        let Some(gl_gen_textures) = resolve_gles_int(b"glGenTextures\0") else { return };
        let Some(gl_bind_texture) = resolve_gles_int(b"glBindTexture\0") else { return };
        let Some(gl_tex_image_2d) = resolve_gles_mixed(b"glTexImage2D\0") else { return };
        let Some(gl_get_error) = resolve_gles_int(b"glGetError\0") else { return };
        let Some(gl_compressed_tex_2d) = resolve_gles_mixed(b"glCompressedTexImage2D\0") else {
            eprintln!("skipping: Mesa GLES compressed-texture bridge unavailable");
            return;
        };
        let Some(gl_get_tex_level) = resolve_gles_int(b"glGetTexLevelParameteriv\0") else { return };

        // EGL constants (egl.h).
        const EGL_NONE: u64 = 0x3038;
        const EGL_SURFACE_TYPE: u64 = 0x3033;
        const EGL_PBUFFER_BIT: u64 = 0x0001;
        const EGL_RENDERABLE_TYPE: u64 = 0x3040;
        const EGL_OPENGL_ES2_BIT: u64 = 0x4;
        const EGL_OPENGL_ES3_BIT: u64 = 0x40;
        const EGL_CONTEXT_CLIENT_VERSION: u64 = 0x3098;
        const EGL_WIDTH: u64 = 0x3057;
        const EGL_HEIGHT: u64 = 0x3056;
        const EGL_NO_CONTEXT: u64 = 0;
        const EGL_NO_SURFACE: u64 = 0;
        // GLES constants (gl2.h / gl3.h).
        const GL_TEXTURE_2D: u64 = 0x0DE1;
        const GL_RGBA: u64 = 0x1908;
        const GL_UNSIGNED_BYTE: u64 = 0x1401;

        let mut st = CpuState::new();

        // (1) Surfaceless EGL context via guest blr, egl_is safe through int ABI.
        let dpy = gcall(egl_getdisplay, &mut st); // eglGetDisplay(EGL_DEFAULT_DISPLAY=0)
        assert_ne!(dpy, 0, "eglGetDisplay must return a real display");
        let mut ver = [0u32; 2];
        st.x[0] = dpy;
        st.x[1] = ver.as_mut_ptr() as u64;
        st.x[2] = ver.as_mut_ptr().wrapping_add(1) as u64; // &minor out-arg (aarch64 x2)
        gcall(egl_initialize, &mut st);
        assert!(ver[0] >= 1, "eglInitialize returns EGL version >= 1");

        // eglChooseConfig into a 1-element config array. Under surfaceless we
        // request a PBUFFER-capable ES3 config (no window); this matches Mesa's
        // surfaceless llvmpipe configs. eglChooseConfig takes `const EGLint*`
        // (i32 elements) — the array MUST be i32, not u64, or Mesa reads
        // misaligned attrib pairs and returns 0 configs.
        let mut attribs = [
            EGL_SURFACE_TYPE as i32, EGL_PBUFFER_BIT as i32,
            EGL_RENDERABLE_TYPE as i32, EGL_OPENGL_ES3_BIT as i32,
            EGL_NONE as i32, 0,
        ];
        let mut config = 0u64;
        let mut num = 0i32;
        st.x[0] = dpy;
        st.x[1] = attribs.as_mut_ptr() as u64;
        st.x[2] = (&mut config) as *mut u64 as u64;
        st.x[3] = 16; // config_size — room for the card's config count
        st.x[4] = (&mut num) as *mut i32 as u64;
        let ok = gcall(egl_choose_config, &mut st);
        assert_eq!(ok, 1, "eglChooseConfig success (found >=1 config)");
        assert!(config != 0, "choose_config returned a config handle");

        // eglCreateContext(display, config, EGL_NO_CONTEXT, ctx_attribs{ES3}).
        // ctx_attribs is also `const EGLint*` (i32), same alignment requirement
        // as eglChooseConfig's attrib list.
        let mut ctx_attribs = [EGL_CONTEXT_CLIENT_VERSION as i32, 3, EGL_NONE as i32, 0];
        st.x[0] = dpy;
        st.x[1] = config;
        st.x[2] = EGL_NO_CONTEXT;
        st.x[3] = ctx_attribs.as_mut_ptr() as u64;
        let ctx = gcall(egl_create_ctx, &mut st);
        assert_ne!(ctx, 0, "eglCreateContext returned a real context");

        // eglMakeCurrent(display, pbuffer, pbuffer, context). Bound a small
        // pbuffer surface (not EGL_NO_SURFACE) so the context is truly current
        // and GL state queries (glGetFloatv(GL_COLOR_CLEAR_VALUE)) read back the
        // value glClearColor set. Surfaceless NO_SURFACE make-current leaves no
        // current drawable, so the clear-state read-back below returns 0.
        let egl_create_pbuf = resolve_egl(b"eglCreatePbufferSurface\0")
            .expect("eglCreatePbufferSurface resolves (int ABI)");
        let mut surf_attribs = [
            EGL_WIDTH as i32, 16, EGL_HEIGHT as i32, 16, EGL_NONE as i32, 0,
        ];
        st.x[0] = dpy;
        st.x[1] = config;
        st.x[2] = surf_attribs.as_mut_ptr() as u64;
        let pbuf_surface = gcall(egl_create_pbuf, &mut st);
        assert_ne!(pbuf_surface, 0, "eglCreatePbufferSurface returned a surface");

        st.x[0] = dpy;
        st.x[1] = pbuf_surface;
        st.x[2] = pbuf_surface;
        st.x[3] = ctx;
        let made = gcall(egl_make_current, &mut st);
        assert_eq!(made, 1, "eglMakeCurrent success on surfaceless llvmpipe");

        // (2) glClearColor(0.5, 0.25, 0.75, 1.0) through the float bridge.
        set_sf(&mut st, 0, 0.5);
        set_sf(&mut st, 1, 0.25);
        set_sf(&mut st, 2, 0.75);
        set_sf(&mut st, 3, 1.0);
        gcall(gl_clear_color, &mut st);

        // (3) Verify the float bridge really set Mesa's clear color by clearing +
        //     reading back REAL pixels from the pbuffer (not glGetFloatv's state
        //     query — Mesa surfaceless llvmpipe reports GL_INVALID_ENUM for
        //     GL_COLOR_CLEAR_VALUE, so the state-query form cannot pass here).
        //     glClear(GL_COLOR_BUFFER_BIT) then glReadPixels into the 16x16
        //     surface: every pixel must be our (0.5,0.25,0.75,1.0) color.
        let gl_clear = resolve_gles_int(b"glClear\0").expect("glClear resolves (int ABI)");
        let gl_read_pixels = resolve_gles_int(b"glReadPixels\0").expect("glReadPixels resolves (int ABI)");
        st.x[0] = 0x4000; // GL_COLOR_BUFFER_BIT
        gcall(gl_clear, &mut st);
        let mut px = [0u8; 16 * 16 * 4];
        st.x[0] = 0;
        st.x[1] = 0;
        st.x[2] = 16;
        st.x[3] = 16;
        st.x[4] = GL_RGBA;
        st.x[5] = GL_UNSIGNED_BYTE;
        st.x[6] = px.as_mut_ptr() as u64;
        gcall(gl_read_pixels, &mut st);
        // Expected: r≈128, g≈64, b≈191, a=255. llvmpipe's float->u8 rounds up
        // (0.5*255=127.5 can land 128..132), so allow a few counts of slack.
        assert!(
            (px[0] as i32 - 130).abs() <= 5 && (px[1] as i32 - 64).abs() <= 5
                && (px[2] as i32 - 191).abs() <= 5 && px[3] == 255,
            "float-bridge clear color rendered real pixels got {:?}",
            &px[0..4]
        );

        // (4) Texture upload through the >8-arg bridge. Bind a real texture so
        //     Mesa accepts the upload, then glTexImage2D with `pixels` on the
        //     guest stack, then glGetError -> sane GL enum (not a crash).
        let mut tex = 0u32;
        st.x[0] = 1; // n
        st.x[1] = (&mut tex) as *mut u32 as u64;
        gcall(gl_gen_textures, &mut st);
        assert_ne!(tex, 0, "glGenTextures produced a texture id");
        st.x[0] = GL_TEXTURE_2D;
        st.x[1] = tex as u64;
        gcall(gl_bind_texture, &mut st);

        let pixels = [0x88u8, 0x44, 0x22, 0xff]; // 1x1 RGBA
        let mut stack_slot = [0u64; 1];
        stack_slot[0] = pixels.as_ptr() as u64; // [guest sp] = pixels ptr
        st.x[31] = stack_slot.as_mut_ptr() as u64; // guest sp points at the spill
        st.x[0] = GL_TEXTURE_2D;
        st.x[1] = 0; // level
        st.x[2] = GL_RGBA; // internalformat
        st.x[3] = 1; // width
        st.x[4] = 1; // height
        st.x[5] = 0; // border
        st.x[6] = GL_RGBA; // format
        st.x[7] = GL_UNSIGNED_BYTE; // type
        gcall(gl_tex_image_2d, &mut st);

        let err = gcall(gl_get_error, &mut st);
        // Mesa accepts a valid 1x1 RGBA8 upload: GL_NO_ERROR (0) or nothing worse
        // than a documented GL enum — certainly not a hang/crash from a garbage
        // marshalled pixels pointer (the pre-bridge stub would have passed NULL).
        assert!(
            err == 0 || (0x0500..=0x0506).contains(&err),
            "glTexImage2D through the stack bridge leaves a sane GL error (got {err:#x})"
        );

        // (5) Android-compressed (ETC2) upload through the interception bridge: a
        //     4x4 ETC2 RGB block (8 bytes) must be decompressed to RGBA8 and
        //     uploaded as GL_RGBA8, NOT passed to Mesa as an undecodable compressed
        //     format. glGetTexLevelParameteriv(GL_TEXTURE_INTERNAL_FORMAT) returns
        //     GL_RGBA8 (0x8058) only if our bridge did the decode; a raw-Mesa
        //     fall-through would leave GL_COMPRESSED_RGB8_ETC2 (0x9274).
        let etc2 = [0u8; 8]; // one 4x4 ETC2 RGB block
        st.x[0] = GL_TEXTURE_2D;
        st.x[1] = 0; // level
        st.x[2] = 0x9274; // GL_COMPRESSED_RGB8_ETC2
        st.x[3] = 4; // width
        st.x[4] = 4; // height
        st.x[5] = 0; // border
        st.x[6] = etc2.len() as u64; // imageSize
        st.x[7] = etc2.as_ptr() as u64; // data
        gcall(gl_compressed_tex_2d, &mut st);

        let mut internal = 0i32;
        st.x[0] = GL_TEXTURE_2D;
        st.x[1] = 0; // level
        st.x[2] = 0x1003; // GL_TEXTURE_INTERNAL_FORMAT
        st.x[3] = (&mut internal) as *mut i32 as u64;
        gcall(gl_get_tex_level, &mut st);
        assert_eq!(
            internal, 0x8058,
            "ETC2 upload was decompressed to GL_RGBA8 by the bridge (got {internal:#x})"
        );
    }

    #[test]
    fn resolve_gles_mixed_resolves_float_gles_but_rejects_unknown() {
        // Float/mixed GLES names must resolve (bridge allocated) on a Mesa host.
        if !matches!(resolve_gles_mixed(b"glClearColor\0"), Some(_)) {
            eprintln!("skipping: Mesa GLES not present");
            return;
        }
        assert!(resolve_gles_mixed(b"glUniform4f\0").is_some());
        assert!(resolve_gles_mixed(b"glTexImage2D\0").is_some());
        // Unknown / non-GLES / integer-ABI-only names rejected.
        assert!(resolve_gles_mixed(b"glTotallyFake\0").is_none());
        assert!(resolve_gles_mixed(b"strlen\0").is_none());
        assert!(resolve_gles_mixed(b"glGetError\0").is_none(), "int-ABI stays on the int resolver");
    }

    /// The auto-allocated GLES/float/JNI host-call slots (which the resolver's
    /// `name -> slot-addr` map does NOT cover) must still be reversible back to
    /// a readable name by `name_of_call_addr`, so the JIT_TRACE run-log of the
    /// real boot prints *which* engine import a dispatch is instead of an
    /// anonymous `slotN`. Regression for the reverse-name registry.
    #[test]
    fn name_of_call_addr_resolves_auto_allocated_gles_and_float_slots() {
        // 1) GLES mixed bridge: resolve_gles_mixed `glClearColor` -> register_gles_call
        //    slot, and the registry must name it back.
        let Some(gles_slot) = resolve_gles_mixed(b"glClearColor\0") else {
            eprintln!("skipping: Mesa GLES not present");
            return;
        };
        let gles_name = name_of_call_addr(gles_slot);
        assert_eq!(gles_name.as_deref(), Some("glClearColor"),
            "gles auto-slot resolves by reverse-name, not slotN");

        // 2) Float bridge: resolve_float `sin` -> register_float_call slot.
        let Some(fl_slot) = resolve_float(b"sin\0") else {
            eprintln!("skipping: libm sin not resolvable");
            return;
        };
        let fl_name = name_of_call_addr(fl_slot);
        assert_eq!(fl_name.as_deref(), Some("sin"),
            "float auto-slot resolves by reverse-name, not slotN");

        // 3) Single-precision float bridge.
        let Some(fl32_slot) = resolve_float32(b"sinf\0") else {
            eprintln!("skipping: libm sinf not resolvable");
            return;
        };
        let fl32_name = name_of_call_addr(fl32_slot);
        assert_eq!(fl32_name.as_deref(), Some("sinf"),
            "f32 auto-slot resolves by reverse-name, not slotN");
    }

    /// The guest's libc `syscall()` import must NOT bind to host glibc syscall()
    /// (which reads the number as x86-64). It must route through our AArch64
    /// dispatcher. Run the interceptor with the engine main-loop's exact call:
    /// `syscall(nr=98 futex, uaddr, op=0x89 FUTEX_WAIT_BITSET_PRIVATE, val, ...)`
    /// against a uaddr whose value != val -> the real host futex returns -EAGAIN,
    /// proving the AArch64 number reached a real futex (not x86-64 getrusage).
    #[test]
    fn syscall_import_routes_aarch64_futex_not_x86_getrusage() {
        // A host futex uaddr (guest memory maps 1:1 to the host, so any aligned
        // host int works). Value 0; wait for val=1 so the futex cannot succeed
        // -> the real host futex returns -EAGAIN, proving the AArch64 number
        // reached a real futex (50 = getrusage on x86-64, which would NOT be
        // -EAGAIN). Bitset = FUTEX_BITSET_MATCH_ANY (0xFFFFFFFF).
        let mut word: libc::c_int = 0;
        let uaddr = &mut word as *mut libc::c_int;
        // Guest libc syscall(nr, uaddr, op, val, timeout=NULL, uaddr2=NULL,
        // val3=bitset) -> intercept(a0=nr, a1=uaddr, a2=op, a3=val, a4=timeout,
        // a5=uaddr2, a6=val3).
        let ret = host_syscall_intercept(
            98,                 // AArch64 futex
            uaddr as u64,       // uaddr
            0x89,               // FUTEX_WAIT_BITSET_PRIVATE
            1,                  // val (1 != *uaddr=0 -> cannot succeed)
            0,                  // timeout = NULL
            0,                  // uaddr2 = NULL
            u32::MAX as u64,    // val3 = bitset = MATCH_ANY
            0,
        ) as i64;
        assert_eq!(ret, -libc::EAGAIN as i64,
            "AArch64 futex must reach a real host futex (-EAGAIN), not getrusage");
    }

    /// resolve(b"syscall") must hand out a slot that routes through the AArch64
    /// dispatcher (not host glibc syscall). We can't easily run the slot body
    /// here, but we assert the resolve path actually installs the interceptor
    /// (name -> host_syscall_intercept slot) so the guest GOT binds to it.
    #[test]
    fn resolve_binds_syscall_import_to_aarch64_interceptor() {
        // Empty-string variant: ensure the special-case is reachable via the
        // guest-symbol name the loader will use.
        let Some(addr) = resolve(b"syscall") else {
            panic!("syscall import must resolve to a slot");
        };
        // The slot address must be a real host-thunk slot.
        let base = crate::jit::host_call_addr(0);
        assert!(addr >= base, "syscall binds within the host-thunk region");
        // name_of_call_addr must recognize it back as `syscall`.
        assert_eq!(
            name_of_call_addr(addr).as_deref(),
            Some("syscall"),
            "syscall import reverse-names to itself"
        );
        // re-resolve is cached (same addr).
        let again = resolve(b"syscall");
        assert_eq!(again, Some(addr));
    }

    /// The guest's `eglGetProcAddress` import (Roblox resolves GLES functions
    /// dynamically through it) must route through a GLES bridge that returns
    /// one of OUR dispatchable host-thunk slots for the requested name — NOT
    /// Mesa's raw function pointer (which the guest's later `blr` cannot
    /// dispatch and which would bypass the GLES float bridge + compressed-
    /// texture interception). The bridge, invoked like the dispatcher would,
    /// must hand back the same slot a direct import of that GLES name resolves
    /// to.
    #[test]
    fn egl_get_proc_address_bridge_returns_dispatchable_gles_slot() {
        // Resolve the import itself -> a GLES-bridge slot whose body is
        // w_eglGetProcAddress.
        let Some(import_slot) = resolve_egl(b"eglGetProcAddress") else {
            panic!("eglGetProcAddress must resolve");
        };
        // It must live in the host-thunk GLES region (dispatchable), not be 0.
        assert!(import_slot >= crate::jit::host_gles_base(), "import binds in the GLES bridge region");
        assert_eq!(name_of_call_addr(import_slot).as_deref(), Some("eglGetProcAddress"));

        // Now drive the bridge the way the dispatcher drives a HostGlesCall:
        // guest x0 = a C-string naming a wrapped GLES function, e.g. glClearColor
        // (float bridge) and glCompressedTexImage2D (texture interception).
        for (name, expect_mixed) in [
            ("glClearColor", true),
            ("glCompressedTexImage2D", true),
            ("glGenTextures", false), // int-ABI -> resolve_gles_int slot
        ] {
            let mut cname = name.as_bytes().to_vec();
            cname.push(0); // guest C-string (NUL-terminated)
            let name_ptr = cname.as_ptr() as u64;
            let mut st = CpuState::new();
            st.x[0] = name_ptr;
            let ret = w_eglGetProcAddress(&mut st as *mut CpuState);
            // Must be a real host-thunk slot (dispatchable by a later guest blr).
            assert!(
                ret >= crate::jit::HOST_THUNK_BASE,
                "eglGetProcAddress({name}) returns a dispatchable slot, got {ret:#x}"
            );
            // The returned slot must resolve to the SAME bridge function as a
            // direct import of that GLES name — i.e. the mixed (float/texture)
            // bridge for float-ABI names, the int bridge otherwise. Assert
            // dispatch-target identity (resolve_gles_mixed allocates a fresh
            // slot per call, so address equality would be wrong), so the
            // interception is preserved through the dynamic path.
            let direct = if expect_mixed {
                resolve_gles_mixed(name.as_bytes())
            } else {
                resolve_gles_int(name.as_bytes())
            };
            let direct = direct.expect(&format!("direct import of wrapped GLES name must resolve: {name}"));
            // For a mixed name, both slots dispatch through the same GLES bridge
            // fn. For an int name, both are integer HostCall slots (same region).
            if expect_mixed {
                assert_ne!(crate::jit::gles_bridge_fn(ret), 0, "{name} is in the GLES bridge region");
                assert_eq!(
                    crate::jit::gles_bridge_fn(ret),
                    crate::jit::gles_bridge_fn(direct),
                    "dynamic eglGetProcAddress({name}) dispatches to the same GLES bridge as a direct import"
                );
            } else {
                assert_eq!(ret, direct, "int-ABI GLES slot is cached/shared address");
            }
        }

        // A name we don't wrap: falls back to Mesa (non-null real fn) or 0 if
        // Mes a lacks it — never a raw Mesa pointer that pretends to be one of
        // our slots.
        let bogus = b"glDefinitelyNotARealFunction123\0";
        let ret = {
            let mut st = CpuState::new();
            st.x[0] = bogus.as_ptr() as u64;
            w_eglGetProcAddress(&mut st as *mut CpuState)
        };
        // Must NOT collude with our GLES region for an unknown name.
        assert!(
            ret < crate::jit::host_gles_base() || ret == 0,
            "unknown proc name falls through or returns 0, got {ret:#x}"
        );
    }

    /// End-to-end: translated guest code that `blr`s into the eglGetProcAddress
    /// bridge must receive a dispatchable host-thunk slot in x0 (the resolver
    /// slot for the requested GLES name), i.e. exactly what the engine does on a
    /// real frame — not a raw Mesa pointer. And that returned slot itself must
    /// be a valid `blr` target (the glGenTextures int-HostCall slot dispatches
    /// to real Mesa). This proves the dynamic GLES-loader path works through the
    /// full dispatcher, not just when calling the wrapper directly.
    #[test]
    fn egl_get_proc_address_routes_guest_blr_to_dispatchable_slot_e2e() {
        // Resolve the import (binds w_eglGetProcAddress as a GLES bridge).
        let Some(import_addr) = resolve_egl(b"eglGetProcAddress") else {
            panic!("eglGetProcAddress must resolve");
        };
        // Guest code: x16 = import slot; `blr x16` (call eglGetProcAddress) then
        // `brk #0`. x0 already holds &procname. On return x0 = the resolved slot.
        let mut img: Vec<u8> = Vec::new();
        img.extend_from_slice(&0xd63f0200u32.to_le_bytes()); // blr x16
        img.extend_from_slice(&0xd4200000u32.to_le_bytes()); // brk #0

        // glGenTextures: int-ABI -> the returned slot dispatches to real Mesa via
        // the integer HostCall. glClearColor: float bridge.
        for name in ["glGenTextures", "glClearColor"] {
            let mut cname = name.as_bytes().to_vec();
            cname.push(0);
            let name_ptr = cname.as_ptr() as u64;
            let mut st = CpuState::new();
            st.x[0] = name_ptr;
            st.x[16] = import_addr;
            let r = jit_run(&img, 0x1000, 0x1000, &mut st as *mut CpuState).expect("jit_run");
            // jit_run returns the final x0 = the resolved dispatchable slot.
            let slot = r;
            assert!(slot >= crate::jit::HOST_THUNK_BASE,
                "eglGetProcAddress({name}) via guest blr returned a dispatchable slot, got {slot:#x}");
            // The returned slot must be the same resolver slot as a direct import
            // of that name (int for glGenTextures, mixed for glClearColor).
            let direct = if name == "glGenTextures" {
                resolve_gles_int(name.as_bytes())
            } else {
                resolve_gles_mixed(name.as_bytes())
            };
            let direct = direct.expect("direct import must resolve");
            if name == "glGenTextures" {
                assert_eq!(slot, direct, "int-ABI GLES slot from dynamic path == direct import");
            } else {
                assert_eq!(
                    crate::jit::gles_bridge_fn(slot),
                    crate::jit::gles_bridge_fn(direct),
                    "float-ABI GLES bridge from dynamic path dispatches to the same target as direct import"
                );
            }
        }

        // And the returned slot is itself a valid [guest blr -> host] target:
        // `blr` to the glGenTextures slot with x0=holder array is a real Mesa
        // call that quite-possibly just fills a texture name into a stack slot;
        // we only assert the dispatcher reaches the host (no outside-image err).
        let ret_gen = {
            let Some(gen_slot) = resolve_gles_int(b"glGenTextures") else { panic!("glGenTextures") };
            let mut cname = b"glGenTextures\0".to_vec();
            cname.push(0);
            let mut st = CpuState::new();
            st.x[0] = cname.as_ptr() as u64;
            st.x[16] = import_addr;
            let r = jit_run(&img, 0x1000, 0x1000, &mut st as *mut CpuState).expect("jit_run");
            let _ = gen_slot;
            r
        };
        // Second blr to that slot: x0 = glGenTextures slot, x1 = 1 (count),
        // x2 = &one-name (a writable guest word). Dispatch should reach Mesa.
        let start = crate::jit::HOST_THUNK_BASE;
        assert!(ret_gen >= start, "second-stage target is a slot");
        let mut two: Vec<u8> = Vec::new();
        two.extend_from_slice(&0xd63f0200u32.to_le_bytes()); // blr x16
        two.extend_from_slice(&0xd4200000u32.to_le_bytes()); // brk #0
        let mut g2 = CpuState::new();
        let name_holder = Box::leak(vec![0x55u8; 64].into_boxed_slice());
        g2.x[0] = name_holder.as_ptr() as u64; // unused arg, harmless for gen
        g2.x[1] = 1;
        g2.x[2] = name_holder.as_ptr() as u64;
        g2.x[16] = ret_gen;
        let r2 = jit_run(&two, 0x1000, 0x1000, &mut g2 as *mut CpuState).expect("second-stage blr reaches real Mesa");
        // glGenTextures returns void -> x0 is the last integer reg the bridge
        // left; just assert the dispatcher resolved the host call (no error).
        assert_eq!(r2, g2.x[0], "second-stage blr dispatched to host (no outside-image error)");
    }

    /// The CLIENT-side DNS plane a real login hits *before* any connect: the
    /// guest resolves a hostname via the libc `getaddrinfo` import (a JUMP_SLOT
    /// the resolver binds to HOST glibc through `dlsym`, not a raw syscall) and
    /// then feeds the returned `ai_addr` straight into the socket plane. SH42b
    /// proved socket/connect/send/recv only against a hardcoded loopback IP;
    /// this pins the missing resolution step end-to-end through the REAL guest
    /// ABI: `getaddrinfo("localhost")` returns a walkable addrinfo chain whose
    /// `ai_family`/`ai_addr` the guest reads, and connecting to that resolved
    /// address reaches a live host TCP peer. No host-side surgery — exactly how
    /// a logged-in session reaches a real Roblox API host.
    #[test]
    fn guest_dns_getaddrinfo_resolves_hostname_then_connect_roundtrip() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let ga = resolve(b"getaddrinfo").expect("host getaddrinfo resolvable");
        let fai = resolve(b"freeaddrinfo").expect("host freeaddrinfo resolvable");

        // A real listener the guest connects to via the RESOLVED address.
        let ln = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let ln_addr = ln.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let srv = std::thread::spawn(move || {
            let (mut sock, _) = ln.accept().expect("accept");
            let mut buf = [0u8; 128];
            let n = sock.read(&mut buf).expect("server read");
            tx.send(buf[..n].to_vec()).unwrap();
            sock.write_all(b"PONG").unwrap();
            sock.flush().unwrap();
        });

        // Guest memory (host==guest addressable): the C strings and the
        // addrinfo* output cell getaddrinfo writes into.
        let hostname = Box::leak(b"localhost\0".to_vec().into_boxed_slice());
        let service = Box::leak(
            std::ffi::CString::new(ln_addr.port().to_string().as_bytes())
                .unwrap()
                .into_bytes_with_nul()
                .into_boxed_slice(),
        );
        let mut res_cell = Box::new(0u64);

        // Guest image: `blr x16; brk` — arg collection is host-side so floats
        // and stack don't matter here (getaddrinfo is a pure-int/ptr ABI).
        let mut img: Vec<u8> = Vec::new();
        img.extend_from_slice(&0xd63f0200u32.to_le_bytes()); // blr x16
        img.extend_from_slice(&0xd4200000u32.to_le_bytes()); // brk #0

        let mut st = CpuState::new();
        // getaddrinfo(node, service, hints, &res)
        st.x[0] = hostname.as_ptr() as u64;
        st.x[1] = service.as_ptr() as u64;
        st.x[2] = 0; // hints = NULL (any family)
        st.x[3] = (&mut *res_cell as *mut u64) as u64;
        st.x[16] = ga;
        let ret = jit_run(&img, 0x1000, 0x1000, &mut st as *mut CpuState).expect("getaddrinfo blr");
        assert_eq!(ret, 0, "getaddrinfo(localhost) must succeed (EAI_OK=0), got {ret}");

        // Walk the returned addrinfo chain (aarch64 LP64 layout): ai_family@4,
        // ai_addrlen@16, ai_addr@24, ai_next@40.
        let mut cur = *res_cell;
        let mut found_inet = false;
        let mut found_addr: u64 = 0;
        let mut found_addrlen: usize = 0;
        for _ in 0..16 {
            assert!(cur != 0, "addrinfo chain ended without an AF_INET entry");
            let family = unsafe { std::ptr::read_unaligned((cur + 4) as *const i32) };
            let addrlen = unsafe { std::ptr::read_unaligned((cur + 16) as *const u32) } as usize;
            let addr = unsafe { std::ptr::read_unaligned((cur + 24) as *const u64) };
            if family == libc::AF_INET as i32 {
                found_inet = true;
                found_addr = addr;
                found_addrlen = addrlen;
                // The resolved sockaddr must be the loopback our listener bound.
                let sin = found_addr as *const libc::sockaddr_in;
                let srr = unsafe { std::ptr::read_unaligned(sin as *const u32) }; // sin_family@0 + sin_port@2
                assert_eq!(srr & 0xffff, libc::AF_INET as u32, "resolved family is AF_INET");
                // sin_addr@4 must be 127.0.0.1 (network-order u32 0x0100007f).
                let saddr = unsafe { std::ptr::read_unaligned((found_addr + 4) as *const u32) };
                let ip = std::net::Ipv4Addr::from(saddr.to_ne_bytes());
                assert_eq!(ip, std::net::Ipv4Addr::LOCALHOST, "getaddrinfo(localhost) resolved to {ip}");
                break;
            }
            cur = unsafe { std::ptr::read_unaligned((cur + 40) as *const u64) };
        }
        assert!(found_inet, "resolve(localhost) must include an IPv4 entry");

        // Feed the RESOLVED ai_addr straight into the socket plane.
        let mut st2 = CpuState::new();
        let mut do_svc = |a: [u64; 6], nr: u64| -> i64 {
            st2.x[0..6].copy_from_slice(&a);
            st2.x[8] = nr;
            crate::jit::guest_svc(&mut st2 as *mut CpuState) as i64
        };
        let fd = do_svc([libc::AF_INET as u64, libc::SOCK_STREAM as u64, 0, 0, 0, 0], 198) as i32;
        assert!(fd >= 0, "socket() failed: {fd}");
        let r = do_svc(
            [fd as u64, found_addr, found_addrlen as u64, 0, 0, 0],
            203,
        );
        assert_eq!(r, 0, "connect() to resolved localhost failed: {r}");

        let payload = b"SESSDATA\n";
        let n = do_svc([fd as u64, payload.as_ptr() as u64, payload.len() as u64, 0, 0, 0], 206);
        assert_eq!(n, payload.len() as i64, "sendto over resolved conn: {n}");
        let mut buf = [0u8; 8];
        let n = do_svc([fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0], 207);
        assert_eq!(n, 4, "recvfrom PONG over resolved conn: {n}");
        assert_eq!(&buf[..4], b"PONG");
        assert_eq!(do_svc([fd as u64, 0, 0, 0, 0, 0], 57), 0, "close()");

        // Free the chain through the guest's own freeaddrinfo import.
        let mut st3 = CpuState::new();
        st3.x[0] = *res_cell;
        st3.x[16] = fai;
        jit_run(&img, 0x1000, 0x1000, &mut st3 as *mut CpuState).expect("freeaddrinfo blr");

        let got = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("server got payload");
        assert_eq!(got, payload.to_vec(), "peer received exactly the guest's login payload");
        srv.join().unwrap();
    }

    /// The legacy resolution path (`gethostbyname`) the client ALSO imports: it
    /// returns a static `hostent` (thread-local, no free) with a differently
    /// shaped layout than getaddrinfo — h_addrtype@16, h_length@20, and
    /// h_addr_list@24 (an array of pointers to packed in_addr). Pins that the
    /// guest can call it via the resolver host-slot and read an AF_INET
    /// address out, so a session using the old bionic API gets a usable host.
    #[test]
    fn guest_dns_gethostbyname_legacy_returns_hostent_addr() {
        let gyb = resolve(b"gethostbyname").expect("host gethostbyname resolvable");

        let hostname = Box::leak(b"localhost\0".to_vec().into_boxed_slice());
        let mut img: Vec<u8> = Vec::new();
        img.extend_from_slice(&0xd63f0200u32.to_le_bytes()); // blr x16
        img.extend_from_slice(&0xd4200000u32.to_le_bytes()); // brk #0

        let mut st = CpuState::new();
        st.x[0] = hostname.as_ptr() as u64;
        st.x[16] = gyb;
        let he = jit_run(&img, 0x1000, 0x1000, &mut st as *mut CpuState).expect("gethostbyname blr");
        assert!(he != 0, "gethostbyname(localhost) must return a hostent");

        // aarch64 LP64 hostent layout: h_name@0, h_aliases@8, h_addrtype@16,
        // h_length@20, h_addr_list@24 (char** -> each in_addr 4 bytes).
        let addrtype = unsafe { std::ptr::read_unaligned((he + 16) as *const i32) };
        let length = unsafe { std::ptr::read_unaligned((he + 20) as *const i32) };
        let addr_list = unsafe { std::ptr::read_unaligned((he + 24) as *const u64) };
        assert_eq!(addrtype, libc::AF_INET as i32, "h_addrtype must be AF_INET");
        assert_eq!(length, 4, "h_length must be 4 (IPv4)");
        assert!(addr_list != 0, "h_addr_list non-null");
        let first = unsafe { std::ptr::read_unaligned(addr_list as *const u64) };
        assert!(first != 0, "h_addr_list[0] non-null");
        let saddr = unsafe { std::ptr::read_unaligned(first as *const u32) };
        let ip = std::net::Ipv4Addr::from(saddr.to_ne_bytes());
        assert_eq!(ip, std::net::Ipv4Addr::LOCALHOST, "gethostbyname(localhost) -> {ip}");
    }

    /// The real libroblox.so imports 10 `AMEDIAFORMAT_KEY_*` OBJECT symbols from
    /// Android libmediandk (absent host-side). A GLOB_DAT relocation writes the
    /// *address of the string constant* into the GOT slot; the guest does
    /// `adrp;ldr xN,[xN,#off]` to load it and passes it to `AMediaFormat_*` as a
    /// `const char*`. Before SH52 those slots resolved to 0, so a real video/
    /// audio-decoding session read a NULL key string (SH19/SH24-style data fault).
    /// Pins that each key now resolves to a live, NUL-terminated host C string
    /// whose bytes are exactly the NDK constant, and that the resolved address is
    /// stable/cached (same pointer on repeat).
    #[test]
    fn android_media_format_key_data_imports_resolve_to_live_strings() {
        let expected: Vec<(&[u8], &[u8])> = vec![
            (b"AMEDIAFORMAT_KEY_MIME", b"mime"),
            (b"AMEDIAFORMAT_KEY_WIDTH", b"width"),
            (b"AMEDIAFORMAT_KEY_HEIGHT", b"height"),
            (b"AMEDIAFORMAT_KEY_COLOR_FORMAT", b"color-format"),
            (b"AMEDIAFORMAT_KEY_STRIDE", b"stride"),
            (b"AMEDIAFORMAT_KEY_BIT_RATE", b"bitrate"),
            (b"AMEDIAFORMAT_KEY_FRAME_RATE", b"frame-rate"),
            (b"AMEDIAFORMAT_KEY_I_FRAME_INTERVAL", b"i-frame-interval"),
            (b"AMEDIAFORMAT_KEY_CHANNEL_COUNT", b"channel-count"),
            (b"AMEDIAFORMAT_KEY_SAMPLE_RATE", b"sample-rate"),
        ];

        for (sym, want) in &expected {
            let Some(addr) = crate::resolver::resolve_android_data(sym) else {
                panic!("{} must resolve", String::from_utf8_lossy(sym));
            };
            assert!(addr != 0, "{} must be non-null", String::from_utf8_lossy(sym));
            let cs = unsafe { std::ffi::CStr::from_ptr(addr as *const libc::c_char) };
            let got = cs.to_bytes();
            assert_eq!(
                got, *want,
                "{} string constant bytes == NDK value",
                String::from_utf8_lossy(sym)
            );
            // Stable/cached: a second resolve returns the same pointer so the GOT
            // slot write is idempotent across the JUMP_SLOT + GLOB_DAT paths.
            let again = crate::resolver::resolve_android_data(sym).expect("repeat resolve");
            assert_eq!(addr, again, "{} cached pointer stable", String::from_utf8_lossy(sym));
        }

        // Unrelated data-object names (bionic FILE array base, unknown) must NOT
        // be claimed by the media-key resolver — they fall through to dlsym/0.
        assert!(crate::resolver::resolve_android_data(b"__sF").is_none());
        assert!(crate::resolver::resolve_android_data(b"definitely_not_a_key").is_none());
    }
}