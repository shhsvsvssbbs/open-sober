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

#[test]
fn fsmap_statx_and_statfs_reach_the_persistent_store() {
    // statx(291) is how bionic/Java answer "does the datastore file exist" —
    // the client checks session.dat/metadata presence BEFORE reading it. It
    // MUST reach the remapped store or the app thinks its store is gone.
    // statfs(43) free-space checks on a guest mount must go to the store too.
    let _g = lock_fsmap();
    let root = std::env::temp_dir().join(format!(
        "opensober-fsmap-statx-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    fsmap::set_root_for_tests(root.clone());

    const FILE: &str = "/data/user/0/com.roblox.client/shared_prefs/session.dat";
    const PAYLOAD: &[u8] = b"robassociated";

    // Create the file via a normal openat (its parent chain is auto-created).
    let path = cstr(FILE);
    let fd = svc(
        [
            libc::AT_FDCWD as u64,
            path.as_ptr() as u64,
            (libc::O_CREAT | libc::O_RDWR) as u64,
            0o600,
            0,
            0,
        ],
        56,
    );
    assert!(fd >= 0, "openat O_CREAT failed: {fd}");
    let fd = fd as i32;
    let buf = cstr(std::str::from_utf8(PAYLOAD).unwrap());
    assert_eq!(svc([fd as u64, buf.as_ptr() as u64, PAYLOAD.len() as u64, 0, 0, 0], 64), PAYLOAD.len() as i64);
    assert_eq!(svc([fd as u64, 0, 0, 0, 0, 0], 57), 0);

    // statx on the existing guest path: must SUCCEED (0) via the store, and
    // report a real regular-file size (stx_size at offset 40 of `struct statx`,
    // a 256-byte asm-generic layout identical on both arches).
    let path2 = cstr(FILE);
    let mut stx = [0u8; 256];
    let r = svc(
        [
            libc::AT_FDCWD as u64,
            path2.as_ptr() as u64,
            0,              // flags
            0x80000,        // STATX_SIZE (1<<19) — request the size bit
            stx.as_mut_ptr() as u64,
            0,
        ],
        291, // statx
    );
    assert_eq!(r, 0, "statx on existing guest store file failed: {r}");
    let size = u64::from_le_bytes((&stx[40..48]).try_into().unwrap());
    assert_eq!(size, PAYLOAD.len() as u64, "statx stx_size mismatch");

    // statx on a MISSING guest path must come back -ENOENT (=-2), proving the
    // path really resolved through the store (a raw host-root /data would EPERM,
    // not ENOENT).
    let missing = cstr("/data/user/0/com.roblox.client/shared_prefs/nope.db");
    let mut stx2 = [0u8; 256];
    let rm = svc(
        [
            libc::AT_FDCWD as u64,
            missing.as_ptr() as u64,
            0,
            0,
            stx2.as_mut_ptr() as u64,
            0,
        ],
        291,
    );
    assert_eq!(rm, -libc::ENOENT as i64, "missing statx should be -ENOENT, got {rm}");

    // statfs(43) on the guest root path must succeed through the store.
    let rootpath = cstr("/data");
    let mut fsbuf = [0u8; 120];
    let rf = svc(
        [rootpath.as_ptr() as u64, fsbuf.as_mut_ptr() as u64, 0, 0, 0, 0],
        43,
    );
    assert_eq!(rf, 0, "statfs on guest /data failed through store: {rf}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn fsmap_truncate_chdir_linkat_readlinkat_resolve_through_store() {
    // truncate(45), chdir(49), linkat(37), readlinkat(78) all take guest paths
    // a real session's data plane can touch; each must reach the store.
    let _g = lock_fsmap();
    let root = std::env::temp_dir().join(format!(
        "opensober-fsmap-meta-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    fsmap::set_root_for_tests(root.clone());

    // Create a file and write 16 bytes, then truncate to 4 via guest ABI.
    const FILE: &str = "/data/user/0/com.roblox.client/files/sess.bin";
    let path = cstr(FILE);
    let fd = svc(
        [
            libc::AT_FDCWD as u64,
            path.as_ptr() as u64,
            (libc::O_CREAT | libc::O_RDWR) as u64,
            0o600,
            0,
            0,
        ],
        56,
    );
    assert!(fd >= 0, "openat failed: {fd}");
    let fd = fd as i32;
    let big = cstr("0123456789abcdef");
    assert_eq!(svc([fd as u64, big.as_ptr() as u64, 16, 0, 0, 0], 64), 16);
    assert_eq!(svc([fd as u64, 0, 0, 0, 0, 0], 57), 0);

    let tp = cstr(FILE);
    let r = svc([tp.as_ptr() as u64, 4, 0, 0, 0, 0], 45); // truncate
    assert_eq!(r, 0, "truncate failed through store: {r}");
    let host_file = root.join(FILE.trim_start_matches('/'));
    assert_eq!(std::fs::metadata(&host_file).unwrap().len(), 4, "host file not truncated");

    // chdir into a remapped guest dir.
    let d = cstr("/data/user/0/com.roblox.client");
    assert_eq!(svc([d.as_ptr() as u64, 0, 0, 0, 0, 0], 49), 0, "chdir failed");

    // linkat(37): link the truncated file to a new guest name in the store.
    let old = cstr(FILE);
    let new = cstr("/data/user/0/com.roblox.client/files/sess_link.bin");
    let rl = svc(
        [
            libc::AT_FDCWD as u64, old.as_ptr() as u64,
            libc::AT_FDCWD as u64, new.as_ptr() as u64,
            0, 0,
        ],
        37,
    );
    assert_eq!(rl, 0, "linkat failed through store: {rl}");
    assert!(root.join("data/user/0/com.roblox.client/files/sess_link.bin").exists());

    // readlinkat(78) — ALSO validates the arg-order fix: the handler must use
    // a[1] as the pathname (dirfd in a[0]), remapped into the store. Create a
    // symlink in the store via GUEST symlinkat (exercising the remap), then
    // resolve it through a guest readlinkat.
    let link_dst = "sess_link.bin"; // relative target (resolved by the kernel)
    let link_src = cstr("/data/user/0/com.roblox.client/files/sym");
    let tgt = cstr(link_dst);
    let rs = svc(
        [
            tgt.as_ptr() as u64,
            libc::AT_FDCWD as u64,
            link_src.as_ptr() as u64,
            0, 0, 0,
        ],
        36, // symlinkat(target, newdirfd, linkpath)
    );
    assert_eq!(rs, 0, "symlinkat failed through store: {rs}");
    let lp = cstr("/data/user/0/com.roblox.client/files/sym");
    let mut rbuf = [0u8; 512];
    let rn = svc(
        [
            libc::AT_FDCWD as u64, lp.as_ptr() as u64,
            rbuf.as_mut_ptr() as u64, rbuf.len() as u64, 0, 0,
        ],
        78, // readlinkat
    );
    assert!(rn > 0, "readlinkat failed through store: {rn}");
    let target = std::str::from_utf8(&rbuf[..rn as usize]).unwrap();
    assert_eq!(target, "sess_link.bin", "readlink resolved {target:?}");

    // A guest readlinkat on a MISSING path must be -ENOENT (resolved through store).
    let gp = cstr("/data/user/0/com.roblox.client/files/ghost");
    let mut gbuf = [0u8; 64];
    let rn2 = svc(
        [
            libc::AT_FDCWD as u64, gp.as_ptr() as u64,
            gbuf.as_mut_ptr() as u64, gbuf.len() as u64, 0, 0,
        ],
        78,
    );
    assert_eq!(rn2, -libc::ENOENT as i64, "missing readlinkat should be -ENOENT, got {rn2}");

    let _ = std::fs::remove_dir_all(&root);
}