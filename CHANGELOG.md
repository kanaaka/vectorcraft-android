# Changelog

## v0.1.1 — 2026-10-13 (scrolling, modifiers, fonts, picker wording)

| | |
|---|---|
| APK | `vectorcraft-0.9.0-arm64-v8a-20261013.apk` (29.6 MB) — supersedes the v0.1.0 asset, same signing key |
| sha256 | `c7848549067eb1192e7935f45cdbf91490f3366ccfdd73c5d44ce1236f219aa6` |
| Built from | upstream `storytold/vectorcraft` @ `2ac3ba2b866ad9d34b2252dd8024f73e34f4b2b8` + this repository at the commit that released it |
| Package | `ai.storyteller.vectorcraft` — versionName `0.9.0`, versionCode `16779520`, minSdk 26, targetSdk 35 |
| ABI | `arm64-v8a` only |

### Changed
- **Modifier keys work.** `src/modifiers.rs` is now part of this port: the shell tracks Ctrl/Shift/Alt
  from the key events the platform does send (winit's Android backend never emits `ModifiersChanged`,
  which is the only source egui reads) and re-injects the state onto every key and pointer event, once
  per frame. Ctrl+A, Ctrl+S, Ctrl+Z and Shift-click work; this closes the gap v0.1.0 was published
  with.

### Notes
The build was made from exactly the source in this repository (upstream pin + the patch + the port
crate) and signed with the same local test key as v0.1.0, so it installs over it.


## v0.1.0 — first published port (VectorCraft 0.9.0)

| | |
|---|---|
| APK | `vectorcraft-0.9.0-arm64-v8a-fonts.apk` (29.6 MB) |
| sha256 | `f4fa64a2506e1fe8f61c4714c9756527020020790cb15f47fa1bc3e7ed2e9438` |
| Built from | upstream `storytold/vectorcraft` @ `2ac3ba2b866ad9d34b2252dd8024f73e34f4b2b8` + `apps/vectorcraft-android/` |
| Package | `ai.storyteller.vectorcraft` — versionName `0.9.0`, minSdk 26, targetSdk 35 |
| ABI | `arm64-v8a` only |
| Signature | throwaway local test key, not an official key |

### In this release
- Android shell crate: NativeActivity entry point, `Services` implementation, file logger, single
  window, quit that really quits (the stale-close problem described in the notes).
- In-app file picker via `Services::start_pick`, with cached listings, readable-root probing, "Show all
  files" and hidden-file count.
- Platform fonts from `/system/fonts` and VectorCraft's own **USB-visible** *Fonts* folder, via
  `set_platform_font_files` / `set_app_font_dir`.
- The desktop title bar is not drawn on Android (`custom_titlebar` off, and the `system_title_bar`
  preference pinned off so the brand mark survives).
- Launcher icon, label, storage permissions, edge-to-edge theme variant.


### Known issues
See *Known gaps* in the [README](README.md) and [`docs/PORT-NOTES.md`](docs/PORT-NOTES.md).
