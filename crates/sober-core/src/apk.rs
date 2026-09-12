// SPDX-License-Identifier: MIT
//
// APK downloader and extractor for Roblox Android APK.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};

/// Ensure a Roblox APK is available at the given path, or download it.
#[allow(dead_code)]
pub fn ensure_apk(apk_path: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = apk_path {
        let p = PathBuf::from(path);
        if p.exists() {
            info!("Using provided APK: {}", p.display());
            return Ok(p);
        }
        anyhow::bail!("APK not found at: {}", p.display());
    }

    // Check cache
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("open-sober")
        .join("apks");

    let cached_apk = cache_dir.join("roblox-android.apk");
    if cached_apk.exists() {
        info!("Using cached APK: {}", cached_apk.display());
        return Ok(cached_apk);
    }

    // No APK provided and none cached.
    anyhow::bail!(
        "No Roblox APK found. Please download it from an APK mirror or \
         extract it from an Android device, then pass it with --apk <path>.\n\
         Note: The Roblox Android APK is available from APKMirror or Google Play."
    );
}

/// Extract native libraries from an APK.
///
/// Handles both standard APK format (lib/arm64-v8a/*.so) and the newer
/// Roblox bundle format (assets/app.zip -> config.arm64_v8a.apk).
pub fn extract_libs(apk_path: &Path, output_dir: &Path) -> Result<Vec<PathBuf>> {
    let file = std::fs::File::open(apk_path)
        .with_context(|| format!("Failed to open APK: {}", apk_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .context("Failed to read APK as ZIP archive")?;

    let lib_dir = output_dir.join("lib");
    std::fs::create_dir_all(&lib_dir)?;

    let mut extracted = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();

        // Standard APK format: lib/arm64-v8a/*.so
        if name.starts_with("lib/arm64-v8a/") && name.ends_with(".so") {
            let filename = name.strip_prefix("lib/arm64-v8a/").unwrap();
            let out_path = lib_dir.join(filename);

            if out_path.exists() {
                extracted.push(out_path);
                continue;
            }

            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
            extracted.push(out_path);
        }

        // New bundle format: assets/app.zip (contains config.arm64_v8a.apk)
        if name == "assets/app.zip" {
            let tmpdir = std::env::temp_dir().join(format!("open-sober-apk-{}", std::process::id()));
            std::fs::create_dir_all(&tmpdir)?;

            let app_zip_path = tmpdir.join("app.zip");
            let mut app_zip_file = std::fs::File::create(&app_zip_path)?;
            std::io::copy(&mut entry, &mut app_zip_file)?;

            // Open the nested app.zip
            if let Ok(mut nested) = zip::ZipArchive::new(std::fs::File::open(&app_zip_path)?) {
                for j in 0..nested.len() {
                    let mut nested_entry = nested.by_index(j)?;
                    let nested_name = nested_entry.name().to_string();

                    // Extract the ARM64 config APK
                    if nested_name == "config.arm64_v8a.apk" {
                        let config_apk_path = tmpdir.join("config.arm64_v8a.apk");
                        let mut config_file = std::fs::File::create(&config_apk_path)?;
                        std::io::copy(&mut nested_entry, &mut config_file)?;

                        // Extract .so files from the config APK
                        if let Ok(mut config) = zip::ZipArchive::new(std::fs::File::open(&config_apk_path)?) {
                            for k in 0..config.len() {
                                let mut so_entry = config.by_index(k)?;
                                let so_name = so_entry.name().to_string();
                                if so_name.starts_with("lib/arm64-v8a/") && so_name.ends_with(".so") {
                                    let filename = so_name.strip_prefix("lib/arm64-v8a/").unwrap();
                                    let out_path = lib_dir.join(filename);
                                    if !out_path.exists() {
                                        let mut out = std::fs::File::create(&out_path)?;
                                        std::io::copy(&mut so_entry, &mut out)?;
                                    }
                                    extracted.push(out_path);
                                }
                            }
                        }
                        break;
                    }
                }
            }

            // Cleanup temp dir
            let _ = std::fs::remove_dir_all(&tmpdir);
        }
    }

    // Also try extracting from just the cached libs directory
    if extracted.is_empty() {
        let cache_libs = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("open-sober")
            .join("libs");
        if cache_libs.exists() {
            for entry in std::fs::read_dir(&cache_libs)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("so") {
                    let dest = lib_dir.join(path.file_name().unwrap());
                    std::fs::copy(&path, &dest)?;
                    extracted.push(dest);
                }
            }
        }
    }

    info!("Extracted {} native libraries to {}", extracted.len(), lib_dir.display());
    Ok(extracted)
}

/// Extract the source APK's `assets/` directory into `out_dir/assets/` so the
/// JIT AssetManager shims can serve the engine its real UI content
/// (FoundationImages sprite sheets, fonts, GLSL shader packs) — recon-v2's
/// "precondition for a frame". Roblox's assets are gzip-compressed in the APK
/// (DEFLATE: 203/594), so we must decompress via the zip reader, not byte-copy.
/// Mirrors `extract_libs`'s handling of both the flat APK and the Roblox bundle
/// form (assets/app.zip -> config.arm64_v8a.apk). Returns the assets root
/// (out_dir/assets) or None if the APK carried no assets/ entries.
pub fn extract_assets(apk_path: &Path, out_dir: &Path) -> Result<Option<PathBuf>> {
    let file = std::fs::File::open(apk_path)
        .with_context(|| format!("Failed to open APK: {}", apk_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .context("Failed to read APK as ZIP archive")?;

    let assets_root: PathBuf = out_dir.join("assets");
    let mut count = 0usize;

    // Walk one zip's `assets/` entries into `assets_root` (decompressing).
    fn extract_assets_from<Z: std::io::Read + std::io::Seek>(
        archive: &mut zip::ZipArchive<Z>,
        assets_root: &Path,
        count: &mut usize,
    ) -> Result<()> {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            if !name.starts_with("assets/") {
                continue;
            }
            let rel = name.strip_prefix("assets/").unwrap();
            if rel.is_empty() {
                continue; // the `assets/` dir entry itself
            }
            let out_path = assets_root.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out_path)?;
                continue;
            }
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
            *count += 1;
        }
        Ok(())
    }

    // Flat APK: assets/ are top-level.
    if let Err(e) = extract_assets_from(&mut archive, &assets_root, &mut count) {
        // A flat-read failure is not fatal to the lib extraction contract; log
        // and treat as no-assets so callers fall back gracefully.
        warn!("asset extraction (flat) skipped: {e:#}");
    }

    // Roblox bundle form: assets/app.zip -> config.arm64_v8a.apk (the config
    // APK itself may carry assets/ too). Only dive in when the flat pass found
    // nothing, mirroring extract_libs's nested handling.
    if count == 0 {
        let tmpdir = std::env::temp_dir().join(format!("open-sober-assets-{}", std::process::id()));
        std::fs::create_dir_all(&tmpdir)?;
        let app_zip_path = tmpdir.join("app.zip");
        // Extract assets/app.zip out of the primary archive.
        if let Some(mut src) = (0..archive.len())
            .find_map(|i| match archive.by_index(i) {
                Ok(mut e) if e.name() == "assets/app.zip" => {
                    std::fs::File::create(&app_zip_path).ok().map(|mut f| {
                        std::io::copy(&mut e, &mut f).ok();
                        f
                    })
                }
                _ => None,
            })
        {
            drop(src);
            if let Ok(mut nested) = zip::ZipArchive::new(std::fs::File::open(&app_zip_path)?) {
                if let Err(e) = extract_assets_from(&mut nested, &assets_root, &mut count) {
                    warn!("asset extraction (nested) skipped: {e:#}");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    if count == 0 {
        let _ = std::fs::remove_dir_all(&assets_root);
        return Ok(None);
    }
    info!(
        "Extracted {count} assets to {}",
        assets_root.display()
    );
    Ok(Some(assets_root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_config_defaults() {
        let cfg = crate::config::SoConfig::default();
        assert_eq!(cfg.quality, 5);
        assert!(cfg.graphics.width > 0);
    }

    /// extract_assets must decompress the source APK's assets/ entries (Roblox
    /// stores 203/594 DEFLATE-compressed) into the host assets root the JIT
    /// AssetManager shims read — recon-v2's "precondition for a frame". Builds a
    /// real zip with a DEFLATE-compressed asset and asserts byte-exact, correct
    /// relative layout, non-assets entries skipped, and no-assets => None.
    #[test]
    fn extract_assets_decompresses_real_apk_assets_into_root() {
        let dir = std::env::temp_dir().join(format!("os-apkassets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let zip_path = dir.join("test.apk");
        let f = std::fs::File::create(&zip_path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        // CompressionMethod::Deflated makes the writer gzip-compress — the real
        // APK path we must support.
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("assets/shaders/ui.pack", opts).unwrap();
        zw.write_all(b"DEFLATED-SHADER\x10\x20\x30").unwrap();
        zw.start_file("assets/fonts/BuilderIcons-Regular.ttf", opts).unwrap();
        zw.write_all(b"\xF0\x9F\x92\xBB FONT").unwrap();
        // Non-assets entries must be ignored.
        zw.start_file("lib/arm64-v8a/libroblox.so", opts).unwrap();
        zw.write_all(b"NOT-AN-ASSET").unwrap();
        // A directory entry under assets/ (Roblox has dir entries).
        zw.add_directory("assets/ExtraContent", zip::write::SimpleFileOptions::default()).unwrap();
        zw.finish().unwrap();

        let out = dir.join("extract");
        let root = extract_assets(&zip_path, &out).unwrap().expect("has assets");
        assert_eq!(root, out.join("assets"));

        let shader = std::fs::read(root.join("shaders/ui.pack")).unwrap();
        assert_eq!(shader, b"DEFLATED-SHADER\x10\x20\x30", "DEFLATE decompressed byte-exact");
        let font = std::fs::read(root.join("fonts/BuilderIcons-Regular.ttf")).unwrap();
        assert_eq!(font, b"\xF0\x9F\x92\xBB FONT");
        // Non-assets entry never landed.
        assert!(!out.join("lib").exists());
        // Directory entry materialized as a dir.
        assert!(root.join("ExtraContent").is_dir());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An APK with no assets/ yields None (callers fall back to NULL/0 shims).
    #[test]
    fn extract_assets_none_when_apk_has_no_assets() {
        let dir = std::env::temp_dir().join(format!("os-apkassets-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("noassets.apk");
        let f = std::fs::File::create(&zip_path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        zw.start_file("lib/arm64-v8a/libroblox.so", zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(b"x").unwrap();
        zw.finish().unwrap();
        let out = dir.join("out");
        assert!(extract_assets(&zip_path, &out).unwrap().is_none());
        assert!(!out.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}