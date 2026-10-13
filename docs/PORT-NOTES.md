# Port notes — VectorCraft on Android

Upstream: [`storytold/vectorcraft`](https://github.com/storytold/vectorcraft) @
`2ac3ba2b866ad9d34b2252dd8024f73e34f4b2b8` (v0.9.0)
Published APK: `vectorcraft-0.9.0-arm64-v8a-fonts.apk`

**No upstream file is changed.** VectorCraft's UI crate already exposes the seams this port needs, so
the whole thing is the shell crate `apps/vectorcraft-android/`: `src/lib.rs` (379 lines) is the shell,
the entry point and the `Services` implementation, and `src/file_picker.rs` (709 lines) is the in-app
picker *and* the font registration. There is deliberately nothing in `patches/`.

This port predates the sibling ports in this set (PhotoCraft, WordCraft, PdfCraft, GridCraft,
DeckCraft), and the gaps listed at the end of this file are the reason the later ones are shaped the
way they are.

## 1. The shell crate

`vectorcraft_ui_egui::Services` is a plain struct of `Option<Box<dyn Fn…>>` fields that derives
`Default`: a platform supplies what it can and omits the rest. The desktop binary and the browser
shell fill it in their own way; this is the third one. Once it exists, the real engine and the real
egui interface run unmodified.

Load-bearing details, the same in every port of this set:

- **`[lib] name = "vectorcraft"` + `crate-type = ["cdylib"]`** — cargo-apk turns that into
  `lib/arm64-v8a/libvectorcraft.so` and points the injected `android.app.lib_name` at the artifact
  name (`-` → `_`). A mismatch gives an APK that installs and then dies on launch.
- **`options.android_app = Some(app)`** — `eframe::run_native` errors without it.
- **A file logger in the app's own data directory** — `logcat` needs adb, and a sideloaded build has
  none.

## 2. Fonts — two registrations the desktop makes in its own binary

Both are silent when missing, and both are one-shot, so the shell does them *before* the window opens:

- **The platform's font files.** `system_font_dirs()` branches macOS / Windows / else-Linux, and
  Android falls into the Linux branch: `/usr/share/fonts`, `$XDG_DATA_HOME/fonts`, `~/.fonts` — none
  of which exist on a tablet. Missing directories are skipped without a word. The shell registers the
  real ones from `/system/fonts` (with `/product/fonts`, `/system_ext/fonts`, `/vendor/fonts`) via
  `vectorcraft_text::set_platform_font_files`, which takes a plain `fn` and is re-called per scan.
- **VectorCraft's own *Fonts* folder**, via `set_app_font_dir`. Without it the app's font-install path
  fails with the message that names the cause — *"VectorCraft's Fonts folder isn't set here: the
  desktop app and vectorcraft-cli set it"*. It is pointed at the **USB-visible**
  `<App media>/Fonts`, not `Android/data`: Android 11+ hides `Android/data` from MTP, so a `.ttf`
  copied in over a cable would silently never arrive.

Bundle fonts are `include_bytes!`d upstream, so text works even with no system font at all; these two
registrations are what make *other* documents render in the typeface they were set in.

## 3. The desktop title bar does not exist on a tablet

VectorCraft draws its own title bar for Windows and Linux — minimize/maximize/close at the right end
of the app bar, plus drag and resize zones — which is meaningless where the surface is always
fullscreen. The shell sets the `custom_titlebar` flag off. That alone is not enough: with it off,
`chrome::system_title_bar` would fall through to a stored preference of the same name and claim the
OS draws the title bar, which drops the brand mark instead. So the shell pins that preference off too
(`prefs.system_title_bar = false`) before the first frame. The app bar — brand, menus, document
switcher — stays; the caption buttons go.

## 4. File ▸ Exit left the app unusable until it was cleared from Recents

Desktop eframe apps answer Exit with `egui::ViewportCommand::Close` and let the window closing end the
process. **On Android that ends nothing**: `egui-winit` turns it into a plain `ViewportEvent::Close`
and never calls winit's window API, which is stubbed on Android anyway. The native thread returns
while the Activity lives on, so the app looks alive, is dead the moment it is touched, and cannot be
reopened until it is swiped away from Recents.

The shell watches the state the app sets when it leaves (`ui.status == "quit"`, set only after nothing
is unsaved), saves the preferences, flushes the log and ends the process:

```rust
fn quit_if_asked(&self) {
    if self.inner.ui.status != "quit" { return; }
    save_prefs(&self.inner);
    log::info!("quitting");
    log::logger().flush();
    std::process::exit(0);
}
```

It is checked in **both** `logic` and `ui`: a menu click runs during `ui`, and no repaint is
guaranteed afterwards, so waiting for the next `logic` is how Exit appears to do nothing until the
user taps again.

## 5. File ▸ Open did nothing — `src/file_picker.rs`

`rfd`, the desktop's dialog library, has no Android backend at all: its `backend.rs` compiles no
module for a target that is neither Linux/BSD, macOS, Windows nor wasm32, and the build fails with
`the trait bound FileDialog: FilePickerDialogImpl is not satisfied`. Android's own picker is not a
substitute — the Storage Access Framework needs `startActivityForResult`, which `android-activity`
0.6 does not expose. So the picker is drawn with egui, between frames, on the seam.

VectorCraft's seam is the **receiver** shape: `Services::start_pick(request)` stashes the request and
returns a `Receiver<Answer>` the app polls once a frame, then re-runs the command the request came
from with the answer. There is one trap in it, and the shell has to know about it: the app consults
`start_pick` only after `picks::can`, which looks at the *synchronous* `pick_open` / `pick_save` /
`pick_open_multi` / `pick_folder` closures — so those are registered too, returning "cancelled", or
the asynchronous picker is never reached at all. They are only asked on an entry where a dialog shown
in line would hold the window.

Details that matter on a tablet, all of them paid for once already: the dialog is sized from
`ctx.content_rect()` (a portrait tablet is narrower than egui's desktop default) with enlarged
padding and spacing; it keeps asking for repaints while open (the app otherwise asks once a second
while waiting, so a tap lags); the folder listing and the readability probe are cached per visit, not
per frame; a root that cannot be read stays tappable and *names the permission and the Settings path*
it needs, instead of sitting greyed out; the filter is the union of every extension the app asked for,
with **"Show all files"** and a count of what the filter is hiding; and a save answer is
`dir.join(name)`, because the app's atomic write needs the parent to exist.

## 6. Permissions, icon, theme

`MANAGE_EXTERNAL_STORAGE` (granted by hand in Settings ▸ Special access — Google Play restricts it to
file-manager-class apps, which does not apply to a sideload), `READ_MEDIA_IMAGES` for Android 13+,
and `READ_EXTERNAL_STORAGE` / `WRITE_EXTERNAL_STORAGE` capped with `max_sdk_version` for older
releases. The APK ships launcher icons at 48/72/96/144/192 px from the app's own hicolor PNGs (Android
scales) and the same two-variant edge-to-edge theme file as its siblings (`res/values/styles.xml`),
needed because `targetSdkVersion 35` puts the app under a transparent status bar on Android 15.

## Gaps — what this port does *not* have

The later ports in this set fixed these; this one has not been reworked since:

- **No modifier tracking.** There is no `src/modifiers.rs` here. winit's Android backend never emits
  `ModifiersChanged`, and `egui-winit` reads modifiers from that event and nowhere else, so on this
  build `Ctrl`/`Shift`/`Alt` combinations do nothing and `Shift`-click does not extend a selection.
  The sibling ports synthesise the modifiers from key events in `raw_input_hook`.
- **Preferences are written on the exit path, not while the app runs.** `save_prefs` runs when the app
  reaches its quit state; a swipe from Recents never gets there. The sibling ports write the
  preferences from a change-detected watcher (`prefs_watch`), which is what makes the recent-documents
  list survivable.
- **One window.** There is no *New Window* support, so two documents cannot be put side by side.
- **No audio.** `Services::audio_out` stays `None`: `cpal` has no Android backend here.

## 2026-10-13 — Modifier keys (the gap this port was published with)

This port shipped with no modifier handling: winit's Android backend never emits `ModifiersChanged`,
and `egui-winit` reads modifiers from that event and nowhere else, so `Ctrl`/`Shift`/`Alt` were
permanently false — `Ctrl+A`, `Ctrl+S`, `Ctrl+Z` and `Shift`-click did nothing. The sibling ports had
already fixed it; this one had not.

`src/modifiers.rs` is now in this crate and applied from the shell's `raw_input_hook`: the key events
do arrive, so the state is tracked from them, folded into the held state, stamped onto every `Key` and
`PointerButton` event, and re-reported once per frame with a `ModifiersChanged`. State is carried
between frames because the press and the click usually land in different ones. The copy here is the
simple variant (no unit tests, no return value) — the shells that synthesise wheel events use the
version whose `apply()` hands the state back.
## Not verified here

The APK attached to **v0.1.1** was built from exactly this tree on 2026-10-13 (upstream pin + the patch + the port crate), and the checks in the README's verification table were run against it. What that does *not* buy is a runtime claim: it was not launched on a device or emulator as part of publishing, so every behavioural statement below rests on the hand-testing the porter did on an arm64 tablet. Unlike its
siblings, this port's fixes have not been exercised against a second round of hand-testing on the
tablet — treat the runtime side of this one as the least confirmed in the set.
