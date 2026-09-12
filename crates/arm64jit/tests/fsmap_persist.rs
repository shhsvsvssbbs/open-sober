// SPDX-License-Identifier: MIT
//
// Hermetic proof of the guest->host persistence path (objective: the real
// client's datastore / login session persists like the real app).
//
// The JIT's `guest_svc` file syscalls are driven directly (no APK needed): a
// guest-style `openat("/data/...")` is remapped to a host file under the
// configured Android root (fsmap), written, closed, and then re-read in a
// *fresh* CpuState (a new "boot"). Asserting the bytes survive across states —
// and land in a real on-disk host file — proves the persistent store an app
// session relies on to "remember sign-in".

use arm64jit::fsmap;
use arm64jit::jit::{guest_svc, CpuState};
use std::ffi::CString;

/// Issue one guest syscall with the given register arguments; returns the
/// AArch64 kernel-style return (negative = -errno).
fn svc(args: [u64; 6], nr: u64) -> i64 {
    let mut st = CpuState::new();
    st.x[0..6].copy_from_slice(&args);
    st.x[8] = nr;
    guest_svc(&mut st as *mut CpuState) as i64
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap()
}

/// The fsmap tests share one process-global override root (set_root_for_tests),
/// so the parallel harness must serialize them.
fn lock_fsmap() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap()
}

#[test]
fn guest_data_dir_writes_persist_to_host_store_across_restart() {
    let _g = lock_fsmap();
    let root = std::env::temp_dir().join(format!(
        "opensober-fsmap-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    fsmap::set_root_for_tests(root.clone());

    const DATA_FILE: &str = "/data/user/0/com.roblox.client/files/session.dat";
    const PAYLOAD: &[u8] = b"ROBLOSECURITY=_abc123_remembered_session";

    // -- "boot" #1: create + write the datastore file through the guest ABI.
    let path = cstr(DATA_FILE);
    let fd = svc(
        [
            libc::AT_FDCWD as u64,
            path.as_ptr() as u64,
            (libc::O_CREAT | libc::O_RDWR | libc::O_TRUNC) as u64,
            0o600,
            0,
            0,
        ],
        56, // openat
    );
    assert!(fd >= 0, "openat O_CREAT failed: {fd}");
    let fd = fd as i32;

    let buf = cstr(std::str::from_utf8(PAYLOAD).unwrap());
    let n = svc([fd as u64, buf.as_ptr() as u64, PAYLOAD.len() as u64, 0, 0, 0], 64); // write
    assert_eq!(n, PAYLOAD.len() as i64, "write short/failed: {n}");
    assert_eq!(svc([fd as u64, 0, 0, 0, 0, 0], 57), 0); // close

    // The remap must have landed a REAL host file (the persistence backend).
    let host_file = root
        .join("data/user/0/com.roblox.client/files/session.dat");
    let on_disk = std::fs::read(&host_file)
        .unwrap_or_else(|e| panic!("mapped host file {} missing: {e}", host_file.display()));
    assert_eq!(on_disk, PAYLOAD, "host-disk bytes != written payload");

    // -- "boot" #2: a FRESH state reopens the same guest path read-only and
    //    sees the earlier payload -> the store survives a restart.
    let path2 = cstr(DATA_FILE);
    let fd2 = svc(
        [
            libc::AT_FDCWD as u64,
            path2.as_ptr() as u64,
            libc::O_RDONLY as u64,
            0,
            0,
            0,
        ],
        56,
    );
    assert!(fd2 >= 0, "reopen read-only failed: {fd2}");
    let fd2 = fd2 as i32;
    let mut rbuf = vec![0u8; PAYLOAD.len()];
    let rn = svc(
        [
            fd2 as u64,
            rbuf.as_mut_ptr() as u64,
            rbuf.len() as u64,
            0,
            0,
            0,
        ],
        63, // read
    );
    assert_eq!(rn, PAYLOAD.len() as i64, "re-read short/failed: {rn}");
    assert_eq!(svc([fd2 as u64, 0, 0, 0, 0, 0], 57), 0);
    assert_eq!(&rbuf, PAYLOAD, "bytes did not survive restart");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn fsmap_only_remaps_absolute_paths_under_writable_android_roots() {
    let _g = lock_fsmap();
    let root = std::env::temp_dir().join(format!(
        "opensober-fsmap-map-{}",
        std::process::id()
    ));
    fsmap::set_root_for_tests(root.clone());

    // Writable Android roots are remapped under the host root: the host path is
    // `root + the guest path sans its leading '/'` (the mount dir equals the
    // first top-level component), preserving the full subtree verbatim.
    for g in [
        "/data/data/com.roblox.client/shared_prefs/prefs.xml",
        "/sdcard/Android/data/com.roblox.client/files/save",
        "/storage/emulated/0/Pictures/x.png",
        "/cache/x.db",
    ] {
        let c = cstr(g);
        let rm = fsmap::remap_path(c.as_ptr())
            .unwrap_or_else(|| panic!("expected {g} to remap"));
        let hp = rm.host_path();
        let expected = root.join(g.trim_start_matches('/'));
        assert_eq!(hp, expected.as_path(), "mapped host path for {g}");
        assert!(hp.starts_with(&root), "host {hp:?} not under root");
    }

    // Non-writable / virtual roots and relative paths pass through unchanged.
    for g in ["/system/etc/hosts", "/proc/self/status", "relative/file.txt"] {
        let c = cstr(g);
        assert!(
            fsmap::remap_path(c.as_ptr()).is_none(),
            "should NOT remap {g}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn fsmap_openat_mkdirat_create_parent_chain_under_root() {
    let _g = lock_fsmap();
    let root = std::env::temp_dir().join(format!(
        "opensober-fsmap-mkdir-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    fsmap::set_root_for_tests(root.clone());

    // mkdirat a deep guest dir; intermediate parents must be auto-created.
    let d = cstr("/data/user/0/com.roblox.client/db");
    let r = svc(
        [libc::AT_FDCWD as u64, d.as_ptr() as u64, 0o700, 0, 0, 0],
        34, // mkdirat
    );
    assert_eq!(r, 0, "mkdirat failed: {r}");
    assert!(
        root.join("data/user/0/com.roblox.client/db").is_dir(),
        "mapped dir not created under root"
    );

    // Now create a file deep inside that chain in one openat (parents ensured).
    let f = cstr("/data/user/0/com.roblox.client/db/nested/even/deeper/state");
    let fd = svc(
        [
            libc::AT_FDCWD as u64,
            f.as_ptr() as u64,
            (libc::O_CREAT | libc::O_RDWR) as u64,
            0o600,
            0,
            0,
        ],
        56,
    );
    assert!(fd >= 0, "deep openat O_CREAT failed: {fd}");
    assert_eq!(svc([fd as u64, 0, 0, 0, 0, 0], 57), 0);
    assert!(
        root.join("data/user/0/com.roblox.client/db/nested/even/deeper/state")
            .is_file(),
        "deep file not created under root"
    );
    let _ = std::fs::remove_dir_all(&root);
}