// SPDX-License-Identifier: MIT
//
// open-sober — Main entry point for the Open Sober Roblox Linux runtime.
//
// Flow:
// 1. Parse CLI args
// 2. Initialize config
// 3. If auth needed, spawn sober-services (or open browser directly)
// 4. Initialize QEMU user-mode + android2gnulinux environment
// 5. Launch Roblox Android APK via binary translation
// 6. Handle graphics/sandbox

mod apk;
mod config;
mod qemu;
mod jitlaunch;
mod android_env;
mod dirs_setup;

use clap::Parser;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "open-sober", about = "Open-source Roblox Linux runtime")]
struct Cli {
    /// Path to a Roblox APK file (downloads if not provided)
    #[arg(short, long)]
    apk: Option<String>,

    /// Roblox place ID to join directly
    #[arg(short, long)]
    place_id: Option<u64>,

    /// Skip auth and use existing .ROBLOSECURITY cookie
    #[arg(short, long)]
    token: Option<String>,

    /// Path to config JSON
    #[arg(short, long)]
    config: Option<String>,

    /// Enable verbose logging
    #[arg(short, long, default_value_t = false)]
    verbose: bool,

    /// Command to run: "auth" (login only), "play" (play game), "launch" (full flow)
    #[arg(default_value = "launch")]
    command: String,

    /// Use the in-process ARM64->x86-64 JIT instead of QEMU (experimental).
    /// Loads the Roblox .so with libloader and runs its entry via arm64jit.
    #[arg(long, default_value_t = false)]
    jit: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose {
        "open_sober=debug,sober_core=debug"
    } else {
        "open_sober=info,sober_core=info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .init();

    info!("Open Sober v{} starting", env!("CARGO_PKG_VERSION"));

    // Load config
    let cfg = config::SoConfig::load(cli.config.as_deref())?;
    info!("Config loaded: {:?}", cfg);

    match cli.command.as_str() {
        "auth" => run_auth(&cli, &cfg)?,
        "play" => run_play(&cli, &cfg)?,
        "launch" => run_launch(&cli, &cfg)?,
        _ => {
            anyhow::bail!("Unknown command: {}. Use 'auth', 'play', or 'launch'.", cli.command);
        }
    }

    Ok(())
}

/// Run authentication flow — prints instructions since the game handles auth.
fn run_auth(_cli: &Cli, _cfg: &config::SoConfig) -> anyhow::Result<()> {
    info!("Auth flow started");
    println!("\nOpen Sober will launch Roblox. Sign in inside the game window when prompted.");
    println!("Auth tokens are stored by the game automatically.");
    Ok(())
}

/// Play a Roblox experience. Token is optional — the game handles auth.
fn run_play(cli: &Cli, cfg: &config::SoConfig) -> anyhow::Result<()> {
    // Token is now optional — the game handles login itself
    let token = cli.token.clone()
        .or_else(|| std::env::var("ROBLOSECURITY").ok())
        .or_else(|| {
            std::fs::read_to_string(
                dirs::data_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                    .join("open-sober")
                    .join(".ROBLOSECURITY")
            ).ok()
        })
        .unwrap_or_default();

    if token.is_empty() {
        info!("No auth token provided. Game will prompt for login.");
        println!("No auth token — Roblox will prompt for login in-game.");
    } else {
        info!("Auth token loaded ({} chars)", token.len());
    }

    // Get APK path from CLI or config
    let apk_path = cli.apk.clone()
        .map(std::path::PathBuf::from)
        .or_else(|| cfg.apk_path.clone())
        .unwrap_or_else(|| std::path::PathBuf::from("roblox-android.apk"));

    if !apk_path.exists() {
        anyhow::bail!("APK not found at: {}. Download one or specify with --apk", apk_path.display());
    }
    info!("APK: {}", apk_path.display());

    // Set up Android environment
    let env = android_env::AndroidEnv::setup()?;
    info!("Android environment ready at: {}", env.root.display());

    if cli.jit {
        info!("Launching via in-process JIT (no QEMU)...");
        let libs = apk::extract_libs(&apk_path, &env.root)?;
        let bin = qemu::find_main_binary(&libs)?;
        info!("Main Roblox binary for JIT: {}", bin.display());
        // Source the engine's REAL UI content for its own AAssetManager (recon-v2
        // "precondition for a frame"): extract the APK's assets/ and export the
        // host root the JIT asset shims read. The spawned elfjit inherits it, so
        // a self-driven frame serving real sprites/fonts/shader-packs just works.
        if let Some(assets) = apk::extract_assets(&apk_path, &env.root)? {
            info!("Serving engine assets from {}", assets.display());
            unsafe { std::env::set_var("SOBER_ASSETS_ROOT", assets.as_os_str()) };
        } else {
            warn!("APK carried no assets/; AAssetManager shims will serve NULL/0");
        }
        return jitlaunch::launch_jit(&bin);
    }

    // Launch via QEMU user-mode
    info!("Launching Roblox via QEMU user-mode...");
    qemu::launch_roblox(&apk_path, &env, &token, cfg, cli.place_id)?;

    Ok(())
}

/// Full launch: just plays directly.
fn run_launch(cli: &Cli, cfg: &config::SoConfig) -> anyhow::Result<()> {
    info!("Launching Roblox...");
    println!("Launching Roblox...");
    run_play(cli, cfg)
}