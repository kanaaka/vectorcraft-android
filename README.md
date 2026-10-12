# VectorCraft for Android (arm64-v8a)

An unofficial Android port of **[VectorCraft](https://github.com/storytold/vectorcraft)** — the real engine
and egui interface, packaged as a NativeActivity APK for sideloading on arm64 devices.

| | |
|---|---|
| Upstream project | [`storytold/vectorcraft`](https://github.com/storytold/vectorcraft) — Apache-2.0 OR MIT |
| Pinned commit | `2ac3ba2` (upstream version `0.9.0`) |
| Android package | `ai.storyteller.vectorcraft` |
| Published APK | `vectorcraft-0.9.0-arm64-v8a-fonts.apk` — 29.6 MB, arm64-v8a, signed |
| APK sha256 | `f4fa64a2506e1fe8f61c4714c9756527020020790cb15f47fa1bc3e7ed2e9438` |
| Upstream files changed | No upstream files changed — the port is `apps/vectorcraft-android/` alone, so there is nothing to apply |
| New port crate | `apps/vectorcraft-android/` — 1088 lines of Rust plus resources |

## What this repository is

This is **not a fork** of VectorCraft. It holds only the Android layer, so it stays small, readable,
and obviously separable from upstream:

```
apps/vectorcraft-android/     the new shell crate — copy it into your checkout of upstream
patches/                the diff against upstream — apply it after checking out the pin
docs/PORT-NOTES.md      what was changed and why, fix by fix, with how each one was checked
scripts/                cross-compile environment, build and verify scripts (from shared-android-port)
CHANGELOG.md            what is in the published APK
```

The Android layer is not a reimplementation. It is a shell crate that fills the same `Services`
struct the desktop and web shells fill, so the real engine and the real egui UI run unchanged.

## Build it

Prerequisites: Rust (rustup) with the `aarch64-linux-android` target, Android SDK + NDK r27, a
JDK 21, and `cargo install cargo-apk`.

```bash
git clone https://github.com/storytold/vectorcraft.git
cd vectorcraft
git fetch --depth 1 origin 2ac3ba2b866ad9d34b2252dd8024f73e34f4b2b8
git checkout FETCH_HEAD                        # the exact commit this port targets
# this port changes no upstream file, so there is no patch to apply
cp -r /path/to/vectorcraft-android apps/             # the shell crate from this repository

# any keystore of your own signs an installable APK; a throwaway test key is fine for sideloading
keytool -genkeypair -keystore ks.jks -alias vectorcraft -keyalg RSA -keysize 2048 -validity 10000 \
        -storepass android -keypass android -dname "CN=Local Test, O=Local, C=US"
export CARGO_APK_RELEASE_KEYSTORE="$PWD/ks.jks" CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android
cargo apk build --release -p vectorcraft-android
```

`scripts/android-env.sh` is the cross-compile environment this port was built with (the NDK linker
and the C compiler the native dependencies need), `scripts/build-apk.sh` wraps the build, and
`scripts/verify-apk.sh` checks the artifact: `aapt2 dump badging`, `apksigner verify`, and `llvm-nm`
on the `.so` inside the APK.

## Install

```bash
adb install -r vectorcraft-0.9.0-arm64-v8a-fonts.apk
```
or copy the APK to the device and open it (installing from unknown sources must be allowed).

On first run, grant **Settings ▸ Apps ▸ VectorCraft ▸ Special access ▸ All files access** if you want
the in-app picker to reach files outside the app's own folders. Without it, Android 10+ refuses a
`read_dir` of shared storage by path, and the picker can only offer the app's own directory.

## Verification status

Everything below was checked on the machine this port was published from, in October 2026.
A tick means the check was *run*, not inferred; the wording says how far it goes.

| Claim | How it was checked | Result |
|---|---|---|
| No upstream file is changed | the port is only `apps/vectorcraft-android/`; there is no patch to apply | n/a |
| The assembled tree equals the tree the APK was built from | file-by-file SHA-256 (line endings normalised) over every patched file and every file of the port crate | ✅ 9 files identical, none differing |
| The port crate compiles for Android | `cargo check -p vectorcraft-android --target aarch64-linux-android` | ✅ exit 0 |
| APK is signed and is the right artifact | `apksigner verify --verbose`, `aapt2 dump badging`, sha256 | ✅ v2/v3 signatures true, `native-code: 'arm64-v8a'`, package `ai.storyteller.vectorcraft` |
| Behaviour on real hardware | hand-testing on the porter's arm64 tablet | ⚠️ the fixes below were found and driven by that testing; each published binary is not individually re-verified here |

**Before trusting the APK:** the source side is verified as listed above; runtime behaviour only as far
as [`docs/PORT-NOTES.md`](docs/PORT-NOTES.md) says per fix, plus the tablet testing that produced those
fixes. There is no automated device test in this repository.

## Known gaps

- **No modifier keys.** Ctrl/Shift/Alt combinations do nothing on this build, and Shift-click does
  not extend a selection: the platform reports no modifiers and this port does not synthesise them
  (its siblings do, see `modifiers.rs` there).
- **Preferences survive only a clean exit.** They are written when the app reaches its quit state, so
  a swipe from Recents can lose UI state including the recent-documents list.
- **One window at a time** — no *New Window*.
- **No audio** — `cpal` has no Android backend here.
- **Runtime verification is the weakest in this set.** The APK was not re-tested on a device while
  publishing; see *Verification status* in the README for exactly what was checked.


## The rest of this set

- [photocraft-android](https://github.com/kanaaka/photocraft-android) — PhotoCraft 0.7.0
- [wordcraft-android](https://github.com/kanaaka/wordcraft-android) — WordCraft 0.5.0
- [pdfcraft-android](https://github.com/kanaaka/pdfcraft-android) — PdfCraft 0.6.0
- [gridcraft-android](https://github.com/kanaaka/gridcraft-android) — GridCraft 0.5.0
- [deckcraft-android](https://github.com/kanaaka/deckcraft-android) — DeckCraft 0.5.0
- [shared-android-port](https://github.com/kanaaka/shared-android-port) — build/verify scripts + the cross-compile environment

## Credits and licence

VectorCraft is © its authors at [`storytold/vectorcraft`](https://github.com/storytold/vectorcraft) and is
dual-licensed Apache-2.0 / MIT; both licence texts are kept here. The Android layer in this
repository is an unofficial modification by [@kanaaka](https://github.com/kanaaka), carrying the
same licence. Not affiliated with or endorsed by the upstream project. See
[`ATTRIBUTION.md`](ATTRIBUTION.md).
