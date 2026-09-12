# SH43 — Guest DNS plane proven end-to-end (getaddrinfo → connect)

## What this closes

The client network plane (SH42b: socket/connect/sendto/recvfrom) was proven only
against a **hardcoded** loopback IP. But a real logged-in session's very first network
action is hostname resolution — `getaddrinfo("api.roblox.com")` — *before* any connect.
That resolution step was unproven, and `getaddrinfo` is a **libc JUMP_SLOT import the
resolver binds to HOST glibc via `dlsym`**, not a raw `svc` syscall — so it rides the
resolver, not `guest_svc`. This cycle proves the whole DNS→socket→byte plane.

## The regression

`guest_dns_getaddrinfo_resolves_hostname_then_connect_roundtrip`
(crates/arm64jit/src/resolver.rs):

1. `resolve(b"getaddrinfo")` / `resolve(b"freeaddrinfo")` return the host-thunk slots.
2. Guest code `blr x16` dispatches to the getaddrinfo slot with
   `getaddrinfo(node="localhost", service=<live-port>, hints=NULL, &res)`.
3. Host glibc resolves and writes an addrinfo chain; the guest-side walk reads the
   aarch64-LP64 layout (ai_family@4 / ai_addrlen@16 / ai_addr@24 / ai_next@40).
4. localhost is asserted to yield an **AF_INET sockaddr exactly 127.0.0.1**
   (network-order u32, `IIIpv4Addr::LOCALHOST`).
5. `ai_addr`/`ai_addrlen` feed straight into `guest_svc` socket(198)/connect(203).
6. A login payload roundtrips to a real host TCP listener and gets PONG back (the
   peer asserts it received the exact bytes).
7. The chain is freed through the guest's own `freeaddrinfo` import (no leak on the
   resolution path).

The client also imports the legacy `gethostbyname` path; SH43b adds a sibling regression
proving it returns a static thread-local `hostent` (h_addrtype@16/h_length@20/h_addr_list@24)
walkable to an AF_INET 127.0.0.1 — a differently-shaped result than getaddrinfo, so both
resolution shapes a session might use are covered. (Workspace 491/0.)

## Why it matters

Section objective 1.a/2 (drive the whole real client as a usable headless session):
a session cannot reach a real Roblox API host without first resolving its hostname.
This closes the final network-plane gap — resolution → connect → byte roundtrip now all
drop-through the real ABI. Combined with SH42b (byte plane) and SH42 (data plane), the
client-side I/O surface a logged-in session needs is complete and regression-pinned.

## Verified

- Workspace **490/0** (was 489/0).
- `cargo build --workspace` clean, `cargo test --workspace` green.
- Productized deliverable re-verified on current HEAD (runs/sh43-play-jit.txt):
  `open-sober play --apk roblox-android.apk --jit` → real libroblox.so → JNI_OnLoad →
  StartApp → render-init → engine triangle + textured quad + 6 quad-loop frames, every
  swap Ok(0x1), exit 124 stable, 534/534 JUMP_SLOT bound, zero ENOSYS.

## Frontier (unchanged)

The engine's own main-loop producer still never enqueues a render-task type
(`w4=4` maintenance cap → thin-TLS-upkeep framework globals; SH14/SH41/SH42). Frames
remain harness-driven. GPU host = documented environment for the final self-driven-login
and frame-performance proof.