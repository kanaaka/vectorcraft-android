//! VectorCraft on Android (arm64-v8a): the real engine and egui shell, running on
//! winit's NativeActivity backend.
//!
//! # What this crate is
//!
//! A shell around the *unmodified* VectorCraft crates — `vectorcraft-engine` (the
//! document model, history and the command registry) and `vectorcraft-ui-egui`
//! (every panel, menu and tool). Nothing in those crates is patched. This crate
//! supplies only what `apps/vectorcraft` would normally supply: the [`Services`]
//! implementation, the preferences file, the window options and the entry point.
//! The same shape the PhotoCraft port used.
//!
//! # Why a separate crate, and not a feature on `apps/vectorcraft`
//!
//! That crate cannot build for Android as written. Under
//! `cfg(not(target_arch = "wasm32"))` — which Android satisfies — it depends on
//! `rfd` (native file dialogs), `arboard` (OS clipboard) and `webbrowser`, and it
//! carries a Win32 DirectWrite font provider and macOS AppKit menus.
//!
//! Verified with `cargo check -p vectorcraft --target aarch64-linux-android`:
//! `arboard` stops first, with `cannot find Clipboard/Get/Set/Clear in module
//! platform` — it has no Android backend at all. `rfd` fails the same way with
//! `the trait bound FileDialog: FilePickerDialogImpl is not satisfied`.
//!
//! Those uses are confined to that crate's own modules, and
//! `vectorcraft_ui_egui::Services` is a plain struct of `Option<Box<dyn Fn…>>`
//! fields that derives `Default`. So the fix is not to patch the app but to build
//! a platform that supplies what it can and omits the rest — which is exactly what
//! `Services` was shaped for.
//!
//! # The feature fix
//!
//! Two eframe features decide whether any of this builds, and both are set in
//! `Cargo.toml` where they belong:
//!
//! - `android-native-activity` **must** be on. Without a backend feature,
//!   `android-activity` is pulled with no backend and the build fails with
//!   `file not found for module activity_impl` plus `Either "game-activity" or
//!   "native-activity" must be enabled as features` — a missing feature, not a
//!   code problem.
//! - `accesskit` **must** be off. eframe `compile_error!`s the pair `accesskit` +
//!   `android-native-activity` (eframe/src/lib.rs:153) because egui's accesskit
//!   integration on Android goes through the GameActivity backend while this
//!   packages NativeActivity. This is the fix the PhotoCraft port needed, where it
//!   took the form of a second, target-gated eframe declaration in
//!   `crates/ui-egui/Cargo.toml`. VectorCraft's `ui-egui` has no eframe dependency
//!   at all — it draws through `egui` and its own CPU compositor — so the trap
//!   cannot be reached from this crate's graph.
//! - `NativeOptions::android_app` must be `Some(..)`: eframe hands it to
//!   `EventLoopBuilderExtAndroid::with_android_app` and returns an error without
//!   it.
//!
//! # What Android cannot do yet
//!
//! Honest gaps, not oversights:
//!
//! - **The file dialogs are drawn by the app itself** ([`file_picker`]). Open…,
//!   Save As…, Place…, Relink to Folder and every preset import go through
//!   `Services::start_pick`, which hands back a receiver the app polls once a frame;
//!   the picker answers on it and what asked runs again with the path. It browses the
//!   app's own folder with no permission at all, and offers a shared-storage root only
//!   when it can actually read it — so Download/Pictures/DCIM/Documents need
//!   "All files access" (or `READ_MEDIA_IMAGES`). Android's *own* picker is still
//!   future work: the Storage Access Framework needs `startActivityForResult`, which
//!   `android-activity` 0.6 does not expose.
//! - **No system clipboard.** `arboard` has no Android backend, so
//!   `system_clipboard` and `clipboard_probe` stay `None`: Copy and Paste fall back
//!   to egui's own handling, and the SVG/PDF/PNG negotiation between apps is gone.
//! - **No open-in-browser or show-in-folder.** `open_url`, `reveal` and `open_file`
//!   need an `Intent` through JNI.
//! - **No printing.** `print` stays `None`, so File ▸ Print saves the job as a PDF
//!   into the app's own folder instead.
//! - **No stylus pressure or tilt.** The same gap PhotoCraft had: a pen degrades to
//!   ordinary touch. Android does expose those on `MotionEvent`, so a backend is
//!   writable, but it does not exist yet.
//!
//! Editing itself is unaffected: the document model, history, the command registry,
//! the CPU compositor and the vector renderer are platform-neutral, and the text
//! crate's `BUNDLED` fonts are `include_bytes!`d, so text works with no system font
//! folder at all.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use vectorcraft_engine::Session;
use vectorcraft_ui_egui::graphics::GraphicsLoss;
use vectorcraft_ui_egui::picks::PickRequest;
use vectorcraft_ui_egui::{FilePick, Services, UiState, VectorcraftApp};

mod file_picker;

/// The app's private data directory, captured in `android_main` before any service
/// closure runs:
/// `<internal storage>/Android/data/ai.storyteller.vectorcraft/files`.
///
/// `internal_data_path()` needs no runtime permission — it is the app's own
/// sandbox — which is why preferences, swatch libraries and recovery copies land
/// here rather than in shared storage.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// VectorCraft's preferences: an `ui.json` in the app's own folder. The desktop
/// shell keeps the same file in `%APPDATA%\VectorCraft` (or its XDG/macOS
/// equivalent) through `prefs_dir`.
fn prefs_file() -> Option<PathBuf> {
    DATA_DIR.get().map(|d| d.join("ui.json"))
}

/// A folder of the user's own next to the preferences — Swatches, Graphic Styles,
/// Libraries, Data Recovery — created on first use.
fn user_dir(name: &str) -> Option<String> {
    let dir = DATA_DIR.get()?.join(name);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.to_string_lossy().to_string())
}

/// Writes a file the way the desktop does, atomically, so a killed app never leaves
/// half a document behind.
fn write_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    vectorcraft_engine::cmd::fileio::write_atomic(Path::new(path), bytes).map_err(|e| e.to_string())
}

/// The platform-neutral half of the desktop `services()`, plus the file picker.
///
/// Read and write carry the whole Save and Export path, because the app's own folder
/// is always reachable. `start_pick` is what makes the dialogs reachable at all.
///
/// `picks::can` looks at `pick_open`/`pick_save`/`pick_open_multi`/`pick_folder`
/// *before* it will consult `start_pick`, so all four must be registered for a dialog
/// to be asked for. They answer "cancelled": they are reached only off an entry, where
/// a dialog shown in line would hold the window, and the picker itself is `start_pick`.
fn android_services(shared: Rc<RefCell<file_picker::Shared>>) -> Services {
    Services {
        read: Some(Box::new(|p: &str| std::fs::read(p).map_err(|e| e.to_string()))),
        write: Some(Box::new(write_file)),
        // Background Save and Export write from a worker thread.
        write_shared: Some(std::sync::Arc::new(write_file)),
        pick_open: Some(Box::new(|_: &FilePick| None)),
        pick_save: Some(Box::new(|_: &FilePick| None)),
        pick_open_multi: Some(Box::new(Vec::new)),
        pick_folder: Some(Box::new(|| None)),
        // The real dialog: queue the request, draw it with egui, answer on the receiver.
        start_pick: Some(Box::new(move |request: PickRequest| file_picker::start(&shared, request))),
        ..Default::default()
    }
}

/// The saved `ui.json` next to the preferences, if it parses.
fn saved_prefs() -> Option<UiState> {
    let text = std::fs::read_to_string(prefs_file()?).ok()?;
    serde_json::from_str(&text).ok()
}

/// Mirrors the desktop `load_prefs`.
fn load_prefs(app: &mut VectorcraftApp, saved: Option<UiState>) {
    if let Some(ui) = saved {
        app.ui = ui.sanitized();
    }
    vectorcraft_ui_egui::prefs_dialog::restore(app);
}

/// Mirrors the desktop `save_prefs`.
fn save_prefs(app: &VectorcraftApp) {
    let Some(p) = prefs_file() else {
        return;
    };
    let mut ui = app.ui.clone();
    ui.engine_prefs = app.session.prefs.to_json();
    if let Ok(bytes) = serde_json::to_vec_pretty(&ui) {
        let _ = write_file(&p.to_string_lossy(), &bytes);
    }
}

/// A minimal file logger, so a sideloaded APK leaves a readable trace.
///
/// `logcat` is only reachable over adb; a file in the app's own data directory can
/// be pulled with any file manager, which is what a sideloaded build needs.
struct FileLogger {
    sink: Mutex<Option<std::fs::File>>,
}

impl log::Log for FileLogger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if let Ok(mut guard) = self.sink.lock() {
            if let Some(f) = guard.as_mut() {
                use std::io::Write;
                let _ = writeln!(f, "[{}] {} {}", record.level(), record.target(), record.args());
            }
        }
    }
    fn flush(&self) {}
}

static LOGGER: OnceLock<FileLogger> = OnceLock::new();

fn init_logging(dir: &Path) {
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("vectorcraft.log")) else {
        return;
    };
    let logger = LOGGER.get_or_init(|| FileLogger { sink: Mutex::new(None) });
    if let Ok(mut guard) = logger.sink.lock() {
        *guard = Some(file);
    }
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// The Android entry point.
///
/// `android-activity` spawns a dedicated thread for this and calls it with the
/// activity handle. `#[unsafe(no_mangle)]` with the Rust ABI is the required shape
/// (edition 2024 requires the `unsafe(..)` wrapper around `no_mangle`), and it is
/// the symbol the injected NativeActivity looks for.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    let data_dir = app.internal_data_path().unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = DATA_DIR.set(data_dir.clone());
    init_logging(&data_dir);
    log::info!("VectorCraft {} starting on Android; data dir {}", env!("CARGO_PKG_VERSION"), data_dir.display());
    // The app's external files directory: always readable and writable with no
    // permission, so the picker offers it as a root and Save can fall back to it.
    if let Some(ext) = app.external_data_path() {
        let _ = std::fs::create_dir_all(&ext);
        let _ = file_picker::EXT_DIR.set(ext);
    }
    // Create the USB-visible media folder so it is there for files to be copied into:
    // Android 11+ hides `Android/data` from MTP, so this is the one the user can reach.
    if let Some(media) = file_picker::media_dir() {
        let _ = std::fs::create_dir_all(&media);
    }
    // Fonts. Two registrations the desktop does in `prefs_dir` and this shell must do itself:
    //
    //  * Android's own fonts live in `/system/fonts`, which `system_font_dirs` does not know —
    //    its Linux branch looks in `/usr/share/fonts` and friends, none of which exist here — so
    //    without this a document set in the system's typeface falls back to a bundled family.
    //  * VectorCraft's Fonts folder, which is what `text.addFontFiles` copies a font into. It is
    //    the USB-visible one, so a font can be dropped in over a cable; without it the app's own
    //    Add step fails with "VectorCraft's Fonts folder isn't set here".
    file_picker::register_fonts();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("VectorCraft").with_app_id("ai.storyteller.vectorcraft"),
        // Required on Android: eframe hands this to
        // `EventLoopBuilderExtAndroid::with_android_app` and errors without it.
        android_app: Some(app),
        ..Default::default()
    };

    let result = vectorcraft_engine::guard::catch_panic(|| {
        eframe::run_native(
            "VectorCraft",
            options,
            Box::new(|cc| {
                let shared = file_picker::channel();
                let mut app = VectorcraftApp::new(Session::new(), android_services(shared.clone()));
                load_prefs(&mut app, saved_prefs());
                // User Defined swatch and graphic style libraries live next to the preferences.
                app.session.swatch_libraries.set_user_dir(user_dir("Swatches"));
                app.session.style_libraries.set_user_dir(user_dir("Graphic Styles"));
                app.session.libraries.set_dir(user_dir("Libraries"));
                // Data Recovery copies too, so unsaved work survives the app being killed.
                app.session.recovery.set_default_folder(user_dir("Data Recovery"));
                // No caption buttons. `custom_titlebar` is what turns the app bar into a
                // *window* title bar: minimize/maximize/close at its right end, a drag area
                // that moves the window and resize zones along its edges. None of that means
                // anything on Android, where the surface is always fullscreen and there is no
                // OS window to move or resize. Leaving it off keeps the app bar itself (brand
                // mark, Home, menus, search, workspace switcher) and drops the buttons.
                app.custom_titlebar = false;
                // With it off, `chrome::system_title_bar` would fall through to this preference
                // and claim the OS draws the title bar — which on Android it never does; the
                // only visible effect would be losing the brand mark. It is a Windows/Linux
                // switch, so the Android build holds it off.
                app.session.prefs.system_title_bar = false;
                let loss = GraphicsLoss::default();
                if let Some(rs) = &cc.wgpu_render_state {
                    let info = rs.adapter.get_info();
                    log::info!("wgpu adapter: {info:?}");
                    app.graphics_adapter = Some(format!("{} ({:?}, {:?})", info.name.trim(), info.backend, info.device_type));
                    let (loss, ctx) = (loss.clone(), cc.egui_ctx.clone());
                    rs.device.set_device_lost_callback(move |reason, msg| loss.report(&ctx, format!("{reason:?}: {msg}")));
                } else {
                    log::warn!("no wgpu render state");
                }
                Ok(Box::new(Shell { inner: app, loss, shared }))
            }),
        )
    });

    match result {
        Ok(Ok(())) => log::info!("VectorCraft exited normally"),
        Ok(Err(e)) => log::error!("VectorCraft failed to run: {e}"),
        Err(e) => log::error!("VectorCraft recovered from a panic: {e}"),
    }
}

/// Wraps the real app so the platform can act on the frame around it: the lost-device
/// report, and saving the preferences on exit.
///
/// The same shape as the web build's `WebShell`
/// (`apps/vectorcraft-web/src/web.rs`): `VectorcraftApp` stays untouched and the
/// platform-specific layer hangs off the outside. Every method is forwarded, so the
/// app sees no difference.
struct Shell {
    inner: VectorcraftApp,
    /// Reported by wgpu when the window's graphics device is lost (a driver reset).
    loss: GraphicsLoss,
    /// The in-app file picker's state, drawn after the app so it floats above it.
    shared: Rc<RefCell<file_picker::Shared>>,
}

impl Shell {
    /// File ▸ Exit, and the system asking the app to close.
    ///
    /// Both end up as `app.quit` → `unsaved::close_all`, which asks about each modified document
    /// and, once none is left, forgets the recovery copies and sets `ui.status = "quit"` with the
    /// comment "the host closes the window". The desktop shell answers with
    /// `ViewportCommand::Close`.
    ///
    /// That answer is not available here. An Android app is closed by finishing its activity,
    /// which needs Java, and winit's Android backend stubs out the window API a close would go
    /// through — `set_visible`, `focus_window` and `pre_drag_notify` are no-ops, `drag_window`
    /// returns an error, `set_fullscreen` only warns — and winit 0.30 has no `request_close` at
    /// all, on any backend. So the process ends, exactly as the desktop shell ends it on macOS
    /// (`end_before_teardown`). Nothing can be unsaved by this point: `close_all` sets the status
    /// only after finding no modified document, so what is left to do is what `on_exit` would
    /// have done — save the preferences — and it is done here because a process exit runs no
    /// `on_exit`.
    fn quit_if_asked(&self) {
        if self.inner.ui.status != "quit" {
            return;
        }
        save_prefs(&self.inner);
        log::info!("quitting");
        log::logger().flush();
        std::process::exit(0);
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(why) = self.loss.take() {
            // eframe can't give a window a new device: the user saves and starts again.
            let kept = self.inner.graphics_lost(&why);
            self.inner.status(if kept {
                "The graphics device was lost: unsaved changes are kept for Data Recovery. Save your documents and restart VectorCraft"
            } else {
                "The graphics device was lost: save your documents and restart VectorCraft"
            });
        }
        self.inner.logic(ctx);
        self.quit_if_asked();
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.inner.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.inner.ui(ui);
        // After the app, so the picker floats above the editor.
        file_picker::draw(ui.ctx(), &self.shared);
        // File ▸ Exit is a menu click, so it is set during this call: check here, in the same
        // frame, rather than leaving it to the next `logic` — no repaint is guaranteed after a
        // click, and waiting for one is how Exit ends up doing nothing.
        self.quit_if_asked();
    }

    fn on_exit(&mut self) {
        save_prefs(&self.inner);
    }
}
