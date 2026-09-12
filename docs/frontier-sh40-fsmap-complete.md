# SH40 — complete the fsmap data-plane path coverage

Building on SH38 (openat/mkdirat/unlinkat/renameat/faccessat/newfstatat), this
cycle routes the REMAINING path-taking syscalls a real session's datastore
touches through `crate::fsmap::remap_path`. Before SH40 they were forwarded raw
against the host root, so a guest `/data/...` path either ENOENTed (host path
absent) or EPERMed (not writable) — and critically a client `statx` on its own
`session.dat` (the modern way bionic/Java answer "does my store exist") would
report it missing.

## What changed (crates/arm64jit/src/jit.rs, guest_svc)

| sycall | nr | change |
|--------|----|--------|
| statx      | 291 | remap a1 (path); dirfd a0=AT_FDCWD for absolute guest paths; `struct statx` asm-generic identical, raw forward |
| statfs     | 43  | remap a0 (path) |
| truncate   | 45  | remap a0 (path) |
| chdir      | 49  | remap a0 (path) |
| linkat     | 37  | remap a1 + a3 (both paths), ensure_parents on both |
| fchmodat   | 53  | remap a1 (path) |
| fchownat   | 54  | remap a1 (path) |
| utimensat  | 88  | remap a1 (path) |
| readlinkat | 78  | remap a1 (pathname) AND fix arg order (see below) |
| symlinkat  | 36  | remap the linkpath arg |

## The readlinkat bug

The old handler was:

```rust
78 => libc::syscall(SYS_readlinkat, AT_FDCWD, a[0], a[1], a[2])
```

`readlinkat(dirfd, pathname, buf, bufsiz)` on aarch64 has dirfd in a0, pathname
in a1 — but the old code passed a0 (the dirfd) as the *pathname*, with a
hardcoded AT_FDCWD. So a real guest readlinkat of a host path (e.g. resolving a
store symlink) EFAULTed. Fixed to `(a0, remap(a1), a2, a3)`.

## Hermetic proof

`crates/arm64jit/tests/fsmap_persist.rs` (real guest_svc ABI, no APK):

1. `fsmap_statx_and_statfs_reach_the_persistent_store` — after openat+write of
   `session.dat`, a guest statx reads back the store's REAL stx_size (offset 40
   of the 256-byte `struct statx`); statx on a missing store path is -ENOENT
   (not EPERM → proves it resolved through the store); statfs on guest `/data`
   succeeds.
2. `fsmap_truncate_chdir_linkat_readlinkat_resolve_through_store` — truncate
   shrinks the mapped host file to 4 bytes; chdir lands in the store; linkat
   hard-links a store file; readlinkat resolves a store symlink and returns
   -ENOENT for a missing one (doubles as proof of the arg-order fix).

Workspace 484 → 486/0.

## Next

The standing structural wall (SH39b): the engine's per-CPU task-deque consumer
parks on the framework producer enqueue; the three BSS dispatch globals
(`0x1068262e8/300/320`) the type-4 maintenance handler blrs through are
framework-populated (statically 0). Untouched by SH40. Continue as the real
client surfaces new datastore/fs syscall gaps.