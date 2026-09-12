// SPDX-License-Identifier: MIT
//
// JIT launch path — drives the REAL Roblox client through the proven
// arm64jit boot+render harness (the `elfjit` binary).
//
// `open-sober play --apk real.apk --jit` extracts the client's native library
// (libroblox.so) from the APK and hands it to `elfjit` with the canonical boot
// recipe that the SH15–SH38 frontier proved headlessly on this box:
//
//   JNI_OnLoad(0x2173ff4: entry) → StartApp(0x258b144: the engine main-loop /
//   EGL/GLES context) → engine's real render-init thunk (0x105b3a280) → its own
//   frame-fn + geometry wrapper + swap → sustainable textured-quad loop, all
//   through the JIT GLES bridge onto a real Mesa-llvmpipe X11 window, with the
//   guest-persistence root (SOBER_ANDROID_ROOT) armed so the client's /data
//   datastore writes land on persistent host disk.
//
// elfjit is self-contained: it brings up its own Xvfb, wires the XID as the
// guest ANativeWindow, creates+exports SOBER_ANDROID_ROOT, and stabilizes into
// the engine's idle main loop (the run log exits 124 under `timeout`, meaning
// a stable, live session rather than a crash). The addresses below are
// specific to the current Roblox v2.738.1397 build — the same fixed-offset
// approach elfjit itself already uses.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use tracing::{info, warn};

/// The proven guest entry (JNI_OnLoad) for the bundled Roblox build.
const JNI_ONLOAD: &str = "0x2173ff4";
/// The nativeAppBridgeV2StartApp entry that creates the engine main loop + EGL/GLES context.
const STARTAPP: &str = "0x258b144";
/// The engine's real render-init thunk (recover the live EGL context + window).
const RENDERINIT: &str = "0x105b3a280";
/// Lifecycle-pulse global the host drives so StartApp keeps ticking.
const KICKER: &str = "0x106863af8";
/// Number of fresh textured-quad frames the play session renders before settling into idle.
const DEFAULT_PROVE_FRAMES: u32 = 6;

/// A ready-to-spawn elfjit invocation, kept as data so it is unit-testable
/// without spawning the ~90 s real-client boot.
pub struct ElfJitInvocation {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Build the canonical proven `play` recipe against `lib` (the extracted
/// libroblox.so). Renders `prove_frames` fresh textured frames (proving the
/// engine's own geometry path) then settles into the idle main loop.
pub fn invocation_proven(lib: &Path, prove_frames: u32) -> ElfJitInvocation {
    let args: Vec<String> = vec![
        lib.display().to_string(),
        JNI_ONLOAD.to_string(), // guest entry = JNI_OnLoad
        "--jni".into(),
        "--startapp".into(),
        STARTAPP.to_string(),
        "--renderinit".into(),
        RENDERINIT.to_string(),
        "--renderthunk".into(),
        "--renderframe".into(),
        "--renderframe-drive".into(),
        "--renderframe-seedgles".into(),
        "--renderframe-drawprobe".into(),
        "--renderframe-triangle".into(),
        "--renderframe-quad".into(),
        "--renderframe-quad-loop".into(),
        prove_frames.to_string(),
        "--kicker".into(),
        KICKER.to_string(),
    ];
    ElfJitInvocation {
        args,
        env: vec![
            // Drive the full boot lifecycle (Xvfb bring-up, app-command post,
            // ANativeWindow wiring) and keep the render-init warm so the engine
            // reaches the EGL context before the first frame drive.
            ("JIT_DRIVE_LIFECYCLE".into(), "1".into()),
            ("RENDERINIT_WARMUP_MS".into(), "5000".into()),
        ],
    }
}

/// Resolve the `elfjit` binary path, building it on demand if necessary.
///
/// elfjit is the proven boot/render harness declared as an example in
/// `arm64jit` (the same executable the `runs/capture_*.sh` harnesses drive via
/// `target/debug/examples/elfjit`). Building it on demand keeps the product in
/// sync with the harness source.
pub fn resolve_elfjit_bin() -> Result<PathBuf> {
    // Workspace target dir: CARGO_TARGET_DIR env, else {manifest}/../../target.
    let base = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(td) => PathBuf::from(td),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target"),
    };
    let candidates = [
        base.join("debug").join("examples").join("elfjit"),
        base.join("release").join("examples").join("elfjit"),
    ];
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    // Not built — try to build the harness on demand.
    info!("elfjit harness not found; building `cargo build -p arm64jit --example elfjit`…");
    let status = Command::new("cargo")
        .args(["build", "-p", "arm64jit", "--example", "elfjit"])
        .status()
        .context("failed to run `cargo build -p arm64jit --example elfjit`")?;
    if !status.success() {
        anyhow::bail!(
            "`cargo build -p arm64jit --example elfjit` failed ({}); build it manually, then retry",
            status
        );
    }
    // Default profile is debug unless --release was passed during this build.
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    anyhow::bail!(
        "elfjit harness still not found after build; looked in {:?}",
        candidates
    )
}

/// Launch the real client via the proven JIT boot+render recipe.
///
/// Spawns `elfjit` against the extracted `lib` (libroblox.so), inheriting
/// stdout/stderr so the full boot+render run log streams to the terminal.
/// Blocks until the client session ends (under `timeout` this is a stable
/// idle main loop, exit 124 — not a crash).
pub fn launch_jit(lib: &Path) -> Result<()> {
    let elfjit = resolve_elfjit_bin()?;
    info!("Launching Roblox via the arm64jit JIT engine ({})", elfjit.display());
    warn!(
        "JIT `play` drives the client's OWN render path (engine GLES bridge); \
         the engine's self-driven main-loop producer is the remaining frontier — \
         frames here are proven harness-driven on the live engine context."
    );

    let inv = invocation_proven(lib, DEFAULT_PROVE_FRAMES);
    let mut cmd = Command::new(&elfjit);
    cmd.args(&inv.args);
    for (k, v) in &inv.env {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    cmd.stdin(Stdio::inherit());

    info!("JIT command: {} {}", elfjit.display(), inv.args.join(" "));
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn JIT engine {}", elfjit.display()))?;
    info!("Roblox (JIT) started (PID: {})", child.id());
    let status = child
        .wait()
        .context("failed to wait for the JIT engine process")?;
    info!("Roblox (JIT) exited with status: {:?}", status.code());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proven_invocation_matches_the_elfjit_boot_and_render_recipe() {
        let inv = invocation_proven(Path::new("/tmp/libroblox.so"), 6);

        let a = &inv.args;
        assert_eq!(a[0], "/tmp/libroblox.so");
        assert_eq!(a[1], "0x2173ff4", "entry = JNI_OnLoad");
        assert!(a.iter().any(|x| x == "--jni"));
        let si = a.iter().position(|x| x == "--startapp").unwrap();
        assert_eq!(a[si + 1], "0x258b144", "StartApp = engine main-loop entry");
        let ri = a.iter().position(|x| x == "--renderinit").unwrap();
        assert_eq!(a[ri + 1], "0x105b3a280", "render-init thunk");
        // Prove the render: the full geometry/draw/swap chain is driven.
        for needed in [
            "--renderinit",
            "--renderthunk",
            "--renderframe",
            "--renderframe-drive",
            "--renderframe-seedgles",
            "--renderframe-drawprobe",
            "--renderframe-triangle",
            "--renderframe-quad",
            "--renderframe-quad-loop",
        ] {
            assert!(a.iter().any(|x| x == needed), "missing {needed}");
        }
        let ql = a.iter().position(|x| x == "--renderframe-quad-loop").unwrap();
        assert_eq!(a[ql + 1], "6", "bounded fresh textured frames before idle");
        let ki = a.iter().position(|x| x == "--kicker").unwrap();
        assert_eq!(a[ki + 1], "0x106863af8", "lifecycle pulse");

        // Env drives the boot lifecycle + warms the render init.
        assert!(inv.env.iter().any(|(k, v)| k == "JIT_DRIVE_LIFECYCLE" && v == "1"));
        assert!(inv.env.iter().any(|(k, v)| k == "RENDERINIT_WARMUP_MS" && v == "5000"));
    }

    #[test]
    fn render_prove_frame_count_is_respected() {
        let inv = invocation_proven(Path::new("/tmp/libroblox.so"), 3);
        let ql = inv.args.iter().position(|x| x == "--renderframe-quad-loop").unwrap();
        assert_eq!(inv.args[ql + 1], "3");
    }
}