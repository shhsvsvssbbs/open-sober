// SPDX-License-Identifier: MIT
//
// Guest filesystem path remapping — the persistence enabler for the real
// client's data plane.
//
// The JIT's `guest_svc` file syscalls pass guest path pointers *verbatim* to
// the host libc (guest vaddr == host addr, no translation). A real Roblox
// Android session reads/writes its datastore, shared-preferences, cache and
// login/session cookies under Android *mount roots*:
//
//     /data/data/com.roblox.client/...        <-- app private data
//     /sdcard/...  /storage/emulated/0/...     <-- external storage
//     /cache/...                               <-- app cache
//
// On the host those absolute paths resolve against the host root, which either
// does not exist (ENOENT) or is not writable by the runtime process (EPERM) —
// so the client cannot persist anything. This module maps those guest roots to
// a real host directory that backs them, giving the client a *persistent*
// on-disk store: a value the client writes under `/data/...` is a real file on
// the host that survives a restart (i.e. "remembers sign-in").
//
// The mapped root is configured once (env `SOBER_ANDROID_ROOT`, or a test
// setter). When unset the remap is inactive and guest paths pass through
// unchanged — the existing boot behavior is untouched until a session root is
// provided. Only ABSOLUTE paths under a known writable Android root are
// remapped; relative (dirfd-relative) and all other absolute paths (`/proc`,
// `/system`, `/tmp`, ...) pass through, so reads of real system state are not
// disturbed.

use std::ffi::CStr;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The persistent host directory that backs the guest Android file system.
/// Defaults to `SOBER_ANDROID_ROOT` (absolute path) when the environment var is
/// set; `None` means remapping is disabled and paths pass through unchanged.
pub fn configured_root() -> Option<PathBuf> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        std::env::var("SOBER_ANDROID_ROOT")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    })
    .clone()
}

/// Test hook: pin the mapped root directly (the environment variable cannot be
/// depended on inside the parallel test harness). Clears any prior value.
pub fn set_root_for_tests(root: PathBuf) {
    let _ = configured_root(); // ensure the OnceLock is initialized before we clear
    // The OnceLock can't be re-set; use an override cell that takes precedence.
    *override_root().lock().unwrap() = Some(root);
}

fn override_root() -> &'static std::sync::Mutex<Option<PathBuf>> {
    static OVR: OnceLock<std::sync::Mutex<Option<PathBuf>>> = OnceLock::new();
    OVR.get_or_init(|| std::sync::Mutex::new(None))
}

fn active_root() -> Option<PathBuf> {
    override_root().lock().unwrap().clone().or_else(configured_root)
}

/// A guest filesystem path that has been resolved into a host path under the
/// configured Android root. Kept as a `CString` so its pointer is directly
/// consumable by the host libc call that takes it.
pub struct RemappedPath {
    host: CString,
    /// The directory that must exist before the path is used with O_CREAT /
    /// mkdirat; `None` if the target's parent is already guaranteed by the
    /// caller (or the path is a bare root).
    pub parent: Option<CString>,
}

impl RemappedPath {
    pub fn as_ptr(&self) -> *const libc::c_char {
        self.host.as_ptr()
    }
    pub fn host_path(&self) -> &Path {
        Path::new(OsStr::from_bytes(self.host.as_bytes()))
    }
}

/// Rewrite an absolute guest path under a writable Android root into the
/// corresponding host path under `active_root()`. Returns `None` when (a) no
/// root is configured, (b) the path is relative, or (c) the path is not under a
/// mapped root — the caller then passes the guest pointer through unchanged.
///
/// The four mounted roots are the standard Android persistent mount points the
/// client's data plane (datastore, shared_prefs, cache, session/cookie store)
/// writes to. Guest paths under the (read-only) boot images `/system`, `/vendor`,
/// `/apex`, `/odm` and the virtual `/proc`/`/dev` trees are deliberately NOT
/// mapped.
pub fn remap_path(guest: *const libc::c_char) -> Option<RemappedPath> {
    let root = active_root()?;
    if guest.is_null() {
        return None;
    }
    // SAFETY: the guest passes a NUL-terminated C string (guest==host src).
    let bytes = unsafe { CStr::from_ptr(guest) }.to_bytes();
    if bytes.is_empty() || bytes[0] != b'/' {
        return None; // relative (dirfd-relative) or empty -> leave alone
    }
    // Map the leading root by the longest-matching prefix so
    // `/storage/emulated/0` is handled before the shorter `/storage`.
    let candidates: [(&[u8], &str); 5] = [
        (b"/storage/emulated", "storage/emulated"),
        (b"/storage", "storage"),
        (b"/data", "data"),
        (b"/sdcard", "sdcard"),
        (b"/cache", "cache"),
    ];
    for (prefix, dir) in candidates {
        if bytes.starts_with(prefix) {
            let rest = &bytes[prefix.len()..];
            let host_rel = if rest.is_empty() {
                dir.to_string()
            } else if rest[0] == b'/' {
                // path.join with a leading-'/' would discard the root
                format!("{dir}{}", String::from_utf8_lossy(rest))
            } else {
                format!("{dir}/{}", String::from_utf8_lossy(rest))
            };
            let host = root.join(host_rel);
            let host_c = CString::new(host.as_os_str().as_bytes()).ok()?;
            let parent = parent_of(&host);
            return Some(RemappedPath {
                parent,
                host: host_c,
            });
        }
    }
    None
}

fn parent_of(p: &Path) -> Option<CString> {
    let par = p.parent()?;
    if par.as_os_str().is_empty() {
        return None;
    }
    CString::new(par.as_os_str().as_bytes()).ok()
}

/// Ensure the immediate parent directory of a remapped path exists (recursively),
/// so `openat(...,O_CREAT)` / `mkdirat` on a deep guest path never fails with
/// ENOENT just because the intermediate /data/user/0/com.roblox.client/... chain
/// has not been created yet. `create` is true only for O_CREAT-style opens.
pub fn ensure_parents(path: Option<&RemappedPath>, create: bool) {
    if !create {
        return;
    }
    if let Some(p) = path {
        if let Some(par) = p.parent.as_ref() {
            let _ = std::fs::create_dir_all(
                Path::new(OsStr::from_bytes(par.as_bytes())),
            );
        }
    }
}