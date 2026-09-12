# SH57 — Real AAssetManager (image-backed asset shims) + real APK asset extraction

## What

Recon-v2 (docs/recon-framework-boot-order.md, authoritative param maps / Java
descriptors) names the AssetManager the **"precondition for a frame"**: the real
client reads its UI content — 594 `assets/` entries (FoundationImages sprite
sheets, BuilderIcons fonts, GLSL shader packs, ProximityPrompt/Publication Name
textures) — out of the source APK via `AAssetManager_fromJava` / `AAssetManager_open`
/ `AAsset_getBuffer` / `AAsset_getLength`. If any of those returns NULL/0, the engine
cannot load a single real asset even when a self-driven frame is eventually produced:
every texture/sprite/font/shaders-`pack` load fails NULL.

Before SH57 all four shims returned NULL/0 (both the Rust JIT shims in
`arm64jit/src/shims.rs` and the C `jni_stubs.h` path). This cycle makes them real:

- **`aassetmanager_fromjava`** returns a stable non-NULL manager sentinel.
- **`aassetmanager_open`** reads the guest C-string filename, normalizes an
  `assets/` prefix, and serves the file from the host `SOBER_ASSETS_ROOT` (the
  extracted APK `assets/` dir) as an owned buffer in an open-asset table.
- **`aasset_getlength` / `aasset_getbuffer`** return the length / a stable host
  pointer the guest derefs directly (guest vaddr == host addr in this JIT; the
  Box buffer is never moved after allocation, so the pointer is stable).
- **`aasset_close`** drops the handle (getters then answer 0).

## Wiring

- `apk::extract_assets(apk_path, out_dir)` extracts the APK's `assets/` into
  `out_dir/assets` **decompressing** entries (Roblox stores 203/594 DEFLATE),
  handles both the flat APK and the `assets/app.zip -> config.arm64_v8a.apk`
  bundle form, skips non-assets entries and directory entries (materializes
  dirs), returns `None` for an APK with no assets.
- `main.rs` `--jit` branch calls `extract_assets` and exports `SOBER_ASSETS_ROOT`
  before `launch_jit` (the spawned elfjit inherits it).

## Verification

- `cargo test --workspace` → **502/0** (was 499/0; +1 `aassetmanager_serves_real_asset_bytes_from_assets_root`
  regression in arm64jit, +2 `extract_assets_*` in sober-core). Build clean.
- Productized real-boot (`open-sober play --apk roblox-android.apk --jit`, runs/sh57-asset-run.txt):
  `Extracted 594 assets to .../android-env/assets` + `Serving engine assets from ...`,
  exit 124 stable, byte-exact persist roundtrip, real indexed triangle
  (centroid RGBA(255,0,0,255)), textured quad BL=RED/BR=GREEN/TR=WHITE/TL=BLUE,
  6 quad-loop frames (swaps Ok(0x1)), zero ENOSYS/json-abort/crash.

## Honest scope

The engine still never self-produces a session/frame (the standing type-4
`[0x106829ea8]` producer-vector wall, SH14/SH46/SH53), so in the productized run
it does not yet issue AAssetManager calls — the asset path is served and proven
hermetic, and configured for the run, but is only *reached* once a self-driven
session reads real UI content. This removes the recon-named precondition block;
the standing structural wall is unchanged.