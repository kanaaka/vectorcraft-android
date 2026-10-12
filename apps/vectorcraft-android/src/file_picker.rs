//! An in-app file picker for Android.
//!
//! # Why this exists
//!
//! VectorCraft has no Java, so it cannot show Android's own file picker: the
//! Storage Access Framework needs `startActivityForResult` and
//! `android-activity` 0.6 does not expose it. The desktop shells fill
//! `Services::pick_open`/`pick_save`/`pick_folder` with `rfd` (native dialogs);
//! Android has no equivalent, so this module draws the picker with egui instead.
//!
//! # How it plugs in
//!
//! VectorCraft's dialogs do not block the frame. [`PickRequest`]s go through
//! [`Services::start_pick`]: the host is handed the request and returns a
//! `Receiver`; the app keeps running, and each frame [`crate::picks::poll`] looks
//! on that receiver. When paths arrive, **what asked runs again** on the document
//! it asked for and gets the path at once. An empty answer means cancelled.
//!
//! So the contract is: [`start`] stashes the request and returns the receiver, and
//! a later frame's [`draw`] sends the answer. That is exactly what this module
//! does, and it is why the picker can be an ordinary egui window.
//!
//! [`PickRequest`]s are only shown this way when the app is running as an *entry*
//! (a command, a dialog's OK, a panel button) — which every real menu item is.
//! `picks::ask` consults `Services::start_pick` only after `picks::can`, and `can`
//! looks at `pick_open`/`pick_save`/`pick_open_multi`/`pick_folder`, so the shell
//! registers all four as well; they return `None`, i.e. cancelled. They are only
//! ever reached off an entry (a code path that asks for a dialog outside a
//! command), where a dialog shown in line would hold the window — so returning
//! cancelled is the safe answer there, not a stub standing in for the picker.
//!
//! # Permissions
//!
//! Android 10+ forbids reading `/storage/emulated/0/Download` by path. Two routes
//! exist and this picker works with either, because it probes each root and only
//! offers the ones it can actually read:
//!
//! - **App folder** — `external_data_path()`, the app's own directory. Always
//!   readable and writable, no permission at all. Files can be pushed here over
//!   USB.
//! - **Shared storage** — Download / Pictures / DCIM / Documents. Needs
//!   "All files access" (`MANAGE_EXTERNAL_STORAGE`, granted by hand in
//!   Settings ▸ Apps ▸ VectorCraft ▸ Special access) or `READ_MEDIA_IMAGES` for
//!   reading media. Both are declared in `Cargo.toml`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender};

use vectorcraft_ui_egui::picks::{PickAnswer, PickRequest};

/// The app's external files directory, captured in `android_main`.
pub static EXT_DIR: OnceLock<PathBuf> = OnceLock::new();

const SHARED_ROOT: &str = "/storage/emulated/0";

/// Which dialog is open.
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// One file to open.
    Open,
    /// Several files to place on the canvas.
    OpenMany,
    /// A path to save to.
    Save,
    /// A folder.
    Folder,
}

/// The dialog being shown: what was asked, and how to answer it.
struct Pending {
    kind: Kind,
    /// Extensions the listing is filtered to (none: every file).
    extensions: Option<Vec<String>>,
    /// Where the answer goes; the app polls it once a frame.
    answer: Option<Sender<PickAnswer>>,
}

/// The picker's state, shared with the shell that draws it.
#[derive(Default)]
pub struct Shared {
    pending: Option<Pending>,
    /// Directory being browsed.
    dir: PathBuf,
    /// File name for a Save dialog.
    name: String,
    /// A message to show inside the picker (usually a permission problem).
    note: Option<String>,
    /// Chosen files, for a multi-select Open.
    picked: Vec<PathBuf>,
    /// Whether the starting directory was already applied.
    started: bool,
    /// List every file, whatever the app's type filter says — the desktop dialogs'
    /// "All files (*)". A filter must never be able to hide a file the app can open.
    show_all: bool,
    /// The roots and whether each can be read, worked out once when the picker opens:
    /// probing them every frame would be six `read_dir`s per frame.
    roots: Option<Vec<(String, PathBuf, bool)>>,
    /// The browsed folder, read once per visit rather than once per frame.
    listing: Option<Listing>,
}

/// One folder's entries, kept between frames.
struct Listing {
    dir: PathBuf,
    /// `Err` when the folder can't be read at all — shared storage without the permission.
    entries: Result<(Vec<PathBuf>, Vec<PathBuf>), String>,
}

/// Build the state the shell draws from.
pub fn channel() -> Rc<RefCell<Shared>> {
    Rc::new(RefCell::new(Shared::default()))
}

/// Every extension the filters offer, or `None` when they offer none (then every
/// file is listed).
fn extensions_of(filters: &[(&'static str, &'static [&'static str])]) -> Option<Vec<String>> {
    let exts: Vec<String> = filters.iter().flat_map(|(_, e)| e.iter().map(|s| (*s).to_string())).collect();
    (!exts.is_empty()).then_some(exts)
}

/// The `Services::start_pick` body: queue the request, return the receiver at once.
pub fn start(shared: &Rc<RefCell<Shared>>, request: PickRequest) -> Option<Receiver<PickAnswer>> {
    let (kind, extensions, suggested) = match &request {
        PickRequest::Open(pick) => (Kind::Open, extensions_of(&pick.filters), None),
        PickRequest::OpenMany => {
            let place: Vec<_> = vectorcraft_engine::cmd::fileio::place_filters().collect();
            (Kind::OpenMany, extensions_of(&place), None)
        }
        PickRequest::Save(pick) => (Kind::Save, None, Some(pick.name.clone())),
        PickRequest::Folder => (Kind::Folder, None, None),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mut s = shared.borrow_mut();
    s.pending = Some(Pending { kind, extensions, answer: Some(tx) });
    // A dialog already open is answered first; this one takes its place.
    s.name = suggested.unwrap_or_default();
    s.picked.clear();
    s.started = false;
    s.note = None;
    // Re-probe and re-read for every dialog. Cached for the app's lifetime, a permission
    // granted in Settings — or a folder whose first read failed — would keep its old
    // answer until the app was killed, which looks exactly like the picker ignoring the
    // grant.
    s.roots = None;
    s.listing = None;
    Some(rx)
}

/// `<external storage>/Android/media/<package>`: the app's own media folder.
///
/// Derived from `external_data_path()` (`…/Android/data/<package>/files`) so the package
/// name isn't written down twice.
///
/// This is the one folder that is both part of the app's own storage (**no permission**)
/// and left **visible over USB/MTP** by Android 11+ — `Android/data` is hidden there, so a
/// file the user copies into the app folder with a cable silently never arrives. Dropping
/// it here instead works.
pub fn media_dir() -> Option<PathBuf> {
    media_dir_from(EXT_DIR.get()?)
}

/// VectorCraft's own Fonts folder: `<App media (USB)>/Fonts`.
///
/// This is what `text.addFontFiles` copies a font into and what the font scan reads, so it is
/// how a font gets installed here at all — the desktop sets the same folder next to its
/// preferences (`prefs_dir::install_fonts`). It is the USB-visible one on purpose: a user can
/// drop a `.ttf` into it over a cable, while `Android/data` is hidden from MTP. Without it the
/// app's own Add step fails with "VectorCraft's Fonts folder isn't set here".
pub fn fonts_dir() -> Option<PathBuf> {
    Some(media_dir()?.join("Fonts"))
}

/// Android's own font files, for [`vectorcraft_text::set_platform_font_files`].
///
/// The text crate's `system_font_dirs` takes its Linux branch on Android — `/usr/share/fonts`,
/// `$XDG_DATA_HOME/fonts`, `~/.fonts`, `/run/host/fonts` — and none of those exist here, so
/// without this the app knows only its four bundled families and every document set in the
/// system's own typeface falls back. Called again by each font scan, so a font a font manager
/// installs later is picked up too.
#[cfg(target_os = "android")]
fn android_system_font_files() -> Vec<String> {
    const DIRS: &[&str] = &["/system/fonts", "/product/fonts", "/system_ext/fonts", "/vendor/fonts"];
    let mut files: Vec<String> = Vec::new();
    for dir in DIRS {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
            if matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
                files.push(path.to_string_lossy().to_string());
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// Register the two font sources Android would otherwise leave the app without: the system's
/// own fonts, and VectorCraft's Fonts folder. Both are one-shot (only the first call counts) and
/// must happen before the first font scan — so, before the window opens.
///
/// The desktop does the same pair in `prefs_dir`: `install_fonts` sets the app's Fonts folder, and
/// Windows lists DirectWrite's collection as the platform's files.
#[cfg(target_os = "android")]
pub fn register_fonts() {
    vectorcraft_text::set_platform_font_files(android_system_font_files);
    if let Some(fonts) = fonts_dir() {
        let _ = std::fs::create_dir_all(&fonts);
        log::info!("fonts: VectorCraft's own folder is {}", fonts.display());
        vectorcraft_text::set_app_font_dir(fonts);
    } else {
        log::warn!("fonts: no VectorCraft Fonts folder to register; adding fonts will fail");
    }
}


/// The derivation, on its own so it can be pinned by a test: `…/Android/data/<package>/files`
/// → `…/Android/media/<package>`.
fn media_dir_from(external_files: &Path) -> Option<PathBuf> {
    let package_dir = external_files.parent()?;
    let android = package_dir.parent()?.parent()?;
    Some(android.join("media").join(package_dir.file_name()?))
}

/// Roots to offer, as (label, path).
fn roots() -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    if let Some(d) = EXT_DIR.get() {
        v.push(("App folder".to_string(), d.clone()));
    }
    if let Some(m) = media_dir() {
        v.push(("App media (USB)".to_string(), m));
    }
    if let Some(d) = EXT_DIR.get().and_then(|d| d.parent()) {
        // Sibling of the app's own files, still owned by the app.
        v.push(("App storage".to_string(), d.to_path_buf()));
    }
    for (label, sub) in [("Download", "Download"), ("Pictures", "Pictures"), ("DCIM", "DCIM"), ("Documents", "Documents")] {
        v.push((label.to_string(), Path::new(SHARED_ROOT).join(sub)));
    }
    v.push(("Internal storage".to_string(), PathBuf::from(SHARED_ROOT)));
    v
}

/// The best directory to open first: the app folder is always writable, so it is
/// the only safe default — desktop paths arriving in `initial_dir` never exist here.
fn default_dir() -> PathBuf {
    EXT_DIR.get().cloned().unwrap_or_else(|| PathBuf::from(SHARED_ROOT))
}

fn readable(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok()
}

/// Shared storage outside the app's own folders — `/storage/emulated/0/…` but not
/// `/storage/emulated/0/Android/…`.
///
/// This is where the OS lies to the app: without "All files access" a shared folder still
/// **opens** (`read_dir` succeeds, so [`readable`] says yes and no lock is shown), but its
/// **contents are filtered away** — an empty listing for a folder full of files. Reproduced
/// in the emulator: `/sdcard/Documents` held five files, the picker read zero. So an empty
/// listing here is not evidence of an empty folder, and the picker must say so.
fn is_shared_storage(dir: &Path) -> bool {
    let shared = Path::new(SHARED_ROOT);
    dir.starts_with(shared) && !dir.starts_with(shared.join("Android"))
}

/// Everything in `dir`, folders first, both sorted, hidden files left out.
///
/// Unfiltered on purpose: the type filter is applied in memory by [`keep`], so the whole
/// folder is read once per visit and the picker can still say how many files a filter is
/// hiding.
fn entries(dir: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for entry in rd.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }
    dirs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()));
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()));
    Ok((dirs, files))
}

/// Whether the app's filter offers `path`. The picker lists the **union** of every
/// extension the app passed, so a format the app reads is never hidden behind another
/// one's filter — `.ai` sits beside `.svg` and `.pdf` in the Open dialog's list. `None`
/// means every file, as does "Show all files".
fn keep(path: &Path, extensions: Option<&Vec<String>>) -> bool {
    match extensions {
        None => true,
        Some(exts) => {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            ext.is_some_and(|e| exts.iter().any(|w| w.eq_ignore_ascii_case(&e)))
        }
    }
}

fn file_label(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn title(kind: Kind) -> &'static str {
    match kind {
        Kind::Open => "Open — Android",
        Kind::OpenMany => "Place — Android",
        Kind::Save => "Save As — Android",
        Kind::Folder => "Choose Folder — Android",
    }
}

/// Draw the picker if a dialog is queued. Called from the shell after the app's own
/// `ui`, so the window floats on top of the editor.
pub fn draw(ctx: &egui::Context, shared: &Rc<RefCell<Shared>>) {
    let mut s = shared.borrow_mut();
    let Some(kind) = s.pending.as_ref().map(|p| p.kind) else {
        return;
    };
    // First frame: pick a starting directory.
    if !s.started {
        s.started = true;
        s.dir = default_dir();
        if kind == Kind::Save && s.name.is_empty() {
            s.name = "untitled.svg".to_string();
        }
    }

    let mut answer: Option<PickAnswer> = None;
    let mut cancel = false;
    let mut navigate: Option<PathBuf> = None;
    // Set by Refresh: the caches must not be written back at the end of the frame.
    let mut refresh = false;

    let dir = s.dir.clone();
    // Probe the roots once, not six `read_dir`s every frame.
    if s.roots.is_none() {
        s.roots = Some(roots().into_iter().map(|(label, path)| {
            let ok = readable(&path);
            (label, path, ok)
        }).collect());
    }
    // Read the folder once per visit rather than once per frame: the type filter then runs
    // over memory, and the picker can say how many files it is hiding.
    if s.listing.as_ref().is_none_or(|l| l.dir != dir) {
        s.listing = Some(Listing { dir: dir.clone(), entries: entries(&dir) });
    }
    let roots_now = s.roots.clone().unwrap_or_default();
    // Read before the window: the file loop filters by it while mutating `s`.
    let show_all = s.show_all;
    // Moved out while the window draws, so the closure can hold these and `s` at once.
    let listing = s.listing.take();
    let exts = s.pending.as_ref().and_then(|p| p.extensions.clone());
    let multi = kind == Kind::OpenMany;
    let all_files: &[PathBuf] = match listing.as_ref().map(|l| &l.entries) {
        Some(Ok((_, files))) => files,
        _ => &[],
    };
    let listed_count = if kind == Kind::Folder {
        all_files.len()
    } else {
        all_files.iter().filter(|f| show_all || keep(f, exts.as_ref())).count()
    };
    let hidden = all_files.len() - listed_count;
    // A tablet screen is smaller than the desktop default, and in portrait narrower
    // than the 620 points PhotoCraft's picker hard-coded. `content_rect` is the area
    // egui considers safe (it excludes an OS status bar where the backend reports one).
    let screen = ctx.content_rect();
    let (w, h) = ((screen.width() * 0.92).min(720.0), (screen.height() * 0.82).min(620.0));

    egui::Window::new(title(kind))
        .collapsible(false)
        .resizable(true)
        .default_width(w)
        .default_height(h)
        .default_pos(screen.center() - egui::vec2(w, h) / 2.0)
        .show(ctx, |ui| {
            // Finger-sized rows: the default spacing is sized for a mouse.
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            ui.spacing_mut().button_padding = egui::vec2(10.0, 8.0);

            // Roots. A locked one stays tappable and says why: greyed out with a lock, it
            // reads as "your files aren't here" and gives no way forward.
            ui.horizontal_wrapped(|ui| {
                for (label, path, ok) in roots_now.iter() {
                    let here = *path == dir;
                    let text = if *ok { label.clone() } else { format!("{label} \u{1F512}") };
                    if ui.add_enabled(!here, egui::Button::new(text)).clicked() {
                        if *ok {
                            navigate = Some(path.clone());
                        } else {
                            s.note = Some(format!(
                                "{label} can't be read. Grant \u{201C}All files access\u{201D} in Settings \u{25B8} Apps \u{25B8} VectorCraft \u{25B8} Special access, then reopen this dialog."
                            ));
                        }
                    }
                }
            });
            ui.separator();

            // Current path + up
            ui.horizontal_wrapped(|ui| {
                if ui.button("\u{2B06} Up").clicked()
                    && let Some(parent) = dir.parent()
                {
                    navigate = Some(parent.to_path_buf());
                }
                if ui.button("\u{1F4C1} App folder").clicked() {
                    navigate = Some(default_dir());
                }
                // Grant the permission, come back and tap Refresh: the roots and the folder
                // are probed again rather than keeping the answer from before the grant.
                if ui.button("\u{21BB} Refresh").clicked() {
                    refresh = true;
                }
                ui.label(egui::RichText::new(dir.display().to_string()).monospace().small());
                ui.label(egui::RichText::new(format!("({} files)", all_files.len())).weak().small());
            });

            // The type filter, and the way out of it. The desktop dialogs always offer
            // "All files"; without one, anything the app's filter list omits is invisible
            // with no hint that it exists.
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut s.show_all, "Show all files");
                if hidden > 0 && !s.show_all {
                    ui.colored_label(
                        egui::Color32::from_rgb(150, 150, 150),
                        format!("{hidden} file(s) hidden by the file-type filter"),
                    );
                }
            });

            if let Some(note) = &s.note {
                ui.colored_label(egui::Color32::from_rgb(220, 160, 60), note);
            }

            ui.separator();
            let list_h = (h - 210.0).max(120.0);
            egui::ScrollArea::vertical().max_height(list_h).show(ui, |ui| match listing.as_ref().map(|l| &l.entries) {
                Some(Err(e)) => {
                    ui.label("Cannot read this folder.");
                    ui.label(egui::RichText::new(e.clone()).monospace().weak().small());
                    ui.label("Grant \"All files access\" in Settings \u{25B8} Apps \u{25B8} VectorCraft \u{25B8} Special access to browse shared storage.");
                }
                Some(Ok((dirs, files))) => {
                    if dirs.is_empty() && listed_count == 0 {
                        ui.label(if hidden > 0 {
                            format!("(no matching files \u{2014} {hidden} hidden by the file-type filter)")
                        } else {
                            "(no files visible here)".to_string()
                        });
                        // The commonest trap on a tablet: the folder really does have files, but
                        // the system is hiding its contents, so it reads as empty.
                        if hidden == 0 && is_shared_storage(&dir) {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 160, 60),
                                "If this folder has files that don't appear, grant \u{201C}All files access\u{201D} in Settings \u{25B8} Apps \u{25B8} VectorCraft \u{25B8} Special access, then tap Refresh. Without it the system hides shared folders' contents from VectorCraft.",
                            );
                        }
                    }
                    for d in dirs {
                        if ui.button(format!("\u{1F4C1} {}", file_label(d))).clicked() {
                            navigate = Some(d.clone());
                        }
                    }
                    if kind != Kind::Folder {
                        for f in files.iter().filter(|f| show_all || keep(f, exts.as_ref())) {
                            let chosen = s.picked.iter().any(|p| p == f);
                            let mark = if chosen { "\u{2611}" } else { "\u{2610}" };
                            let label = if multi { format!("{mark} {}", file_label(f)) } else { format!("   {}", file_label(f)) };
                            if ui.button(label).clicked() {
                                if kind == Kind::Save {
                                    s.name = file_label(f);
                                } else if multi {
                                    if chosen {
                                        s.picked.retain(|p| p != f);
                                    } else {
                                        s.picked.push(f.clone());
                                    }
                                } else {
                                    answer = Some(vec![f.to_string_lossy().to_string()]);
                                }
                            }
                        }
                    }
                }
                // No folder read yet.
                None => {
                    ui.label("(empty folder)");
                }
            });

            ui.separator();
            ui.horizontal_wrapped(|ui| match kind {
                Kind::Save => {
                    ui.label("File name:");
                    ui.add(egui::TextEdit::singleline(&mut s.name).desired_width((w - 260.0).max(160.0)));
                    if ui.button("Save").clicked() {
                        let name = s.name.trim().to_string();
                        if name.is_empty() {
                            s.note = Some("Enter a file name.".to_string());
                        } else {
                            answer = Some(vec![dir.join(name).to_string_lossy().to_string()]);
                        }
                    }
                }
                Kind::Folder => {
                    if ui.button("Choose this folder").clicked() {
                        answer = Some(vec![dir.to_string_lossy().to_string()]);
                    }
                }
                Kind::OpenMany => {
                    let n = s.picked.len();
                    let label = if n == 0 { "Place".to_string() } else { format!("Place {n} file(s)") };
                    if ui.add_enabled(n > 0, egui::Button::new(label)).clicked() {
                        answer = Some(s.picked.iter().map(|p| p.to_string_lossy().to_string()).collect());
                    }
                }
                Kind::Open => {
                    if ui.button("Open").clicked() {
                        // A file has to be tapped in the list above for an Open dialog.
                        s.note = Some("Tap a file to open it.".to_string());
                    }
                }
            });
            if ui.button("Cancel").clicked() {
                cancel = true;
            }

            // Honest, and it corrects what the first build said: Android 11+ blocks copying
            // into `Android/data` over USB/MTP, so "push files here" was wrong advice.
            if dir == default_dir() {
                ui.label(egui::RichText::new("The app's own folder: files here open and save with no permission. Android 11+ blocks copying into Android/data over USB \u{2014} to bring files in, put them in Download and grant \u{201C}All files access\u{201D}.").weak().small());
            }
        });

    // Back into the cache; a navigation made it stale and the next frame re-reads it.
    // Refresh clears both caches instead of keeping this frame's answer, so the probe and
    // the folder read run again — that is the whole point of the button.
    if refresh {
        s.roots = None;
        s.listing = None;
        s.note = None;
    } else {
        s.listing = listing;
    }

    if let Some(dir) = navigate {
        s.dir = dir;
        s.note = None;
    }
    if let Some(paths) = answer {
        if let Some(pending) = s.pending.take()
            && let Some(tx) = pending.answer
        {
            let _ = tx.send(paths);
        }
        s.picked.clear();
    } else if cancel {
        // Cancelled: an empty answer is what the app reads as one.
        if let Some(pending) = s.pending.take()
            && let Some(tx) = pending.answer
        {
            let _ = tx.send(Vec::new());
        }
        s.picked.clear();
    }
    // The app looks at the receiver once a second while it waits; this keeps the
    // picker itself responsive between those frames.
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectorcraft_engine::cmd::fileio;

    /// A scratch folder with the given files in it.
    fn scratch(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vectorcraft-android-picker-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        dir
    }

    /// The picker lists the union of every filter the app passes, so a format the app
    /// says it can read is never hidden behind another filter's list. `.ai` is the one
    /// reported missing on a tablet, so it is pinned here.
    #[test]
    fn the_open_dialog_lists_ai_files() {
        let filters: Vec<_> = fileio::open_filters().collect();
        let exts = extensions_of(&filters).expect("the Open dialog filters offer extensions");
        assert!(exts.iter().any(|e| e == "ai"), "\"ai\" is not among the Open dialog's extensions: {exts:?}");

        let dir = scratch("ai", &["logo.ai", "poster.AI", "drawing.svg", "notes.txt"]);
        let (dirs, files) = entries(&dir).unwrap();
        let listed: Vec<String> = files.iter().filter(|f| keep(f, Some(&exts))).map(|p| file_label(p)).collect();
        assert!(dirs.is_empty(), "no subfolders were made");
        assert!(listed.contains(&"logo.ai".to_string()), "a .ai file the app can read must be listed: {listed:?}");
        assert!(listed.contains(&"poster.AI".to_string()), "the extension match is case-insensitive: {listed:?}");
        assert!(listed.contains(&"drawing.svg".to_string()), "{listed:?}");
        // Not a format the app reads, so hidden — which is exactly why the picker says how
        // many it is hiding and offers "Show all files".
        assert!(!listed.contains(&"notes.txt".to_string()), "a format the app doesn't read is filtered out: {listed:?}");

        // The folder read is unfiltered, so the count of hidden files is knowable...
        assert!(files.iter().any(|p| file_label(p) == "notes.txt"), "the folder read still sees it: {files:?}");
        // ...and "Show all files" reaches anything the filter would drop.
        assert!(keep(&dir.join("notes.txt"), None), "no filter offers every file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unreadable folder is reported, not silently empty — that is what "my files
    /// aren't there" looks like when the permission is missing.
    #[test]
    fn an_unreadable_folder_is_an_error_not_an_empty_one() {
        let missing = std::env::temp_dir().join("vectorcraft-android-picker-does-not-exist");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(entries(&missing).is_err(), "a folder that can't be read is an Err, so the picker can explain");
    }

    /// Hidden files stay out of the listing, as on the desktop.
    #[test]
    fn dotfiles_are_left_out() {
        let dir = scratch("hidden", &["shown.ai", ".hidden.ai"]);
        let (_, files) = entries(&dir).unwrap();
        let names: Vec<String> = files.iter().map(|p| file_label(p)).collect();
        assert_eq!(names, vec!["shown.ai".to_string()], "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The USB-visible root's path. `Android/data` is hidden from MTP on Android 11+, so
    /// this folder is the one a user can actually copy a file into; the package name is
    /// derived rather than written down a second time, and this is what pins it.
    #[test]
    fn the_usb_visible_root_is_derived_from_the_app_folder() {
        let ext = Path::new("/storage/emulated/0/Android/data/ai.storyteller.vectorcraft/files");
        assert_eq!(
            media_dir_from(ext),
            Some(PathBuf::from("/storage/emulated/0/Android/media/ai.storyteller.vectorcraft"))
        );
        assert_eq!(media_dir_from(Path::new("/nowhere")), None, "a path without the Android layout has no media folder");
    }

    /// Shared storage is where the OS hides a folder's contents; the app's own `Android/…`
    /// folders are not, so an empty listing there really is empty.
    #[test]
    fn shared_storage_is_recognised_so_an_empty_listing_can_be_explained() {
        for d in ["/storage/emulated/0/Documents", "/storage/emulated/0/Download", "/storage/emulated/0"] {
            assert!(is_shared_storage(Path::new(d)), "{d} is shared storage");
        }
        for d in [
            "/storage/emulated/0/Android/data/ai.storyteller.vectorcraft/files",
            "/storage/emulated/0/Android/media/ai.storyteller.vectorcraft",
            "/data/user/0/ai.storyteller.vectorcraft/files",
        ] {
            assert!(!is_shared_storage(Path::new(d)), "{d} is the app's own");
        }
    }

    /// What the picker is actually handed, end to end: run the real `file.open` command
    /// and capture the `PickRequest` the app builds. This is the filter list the picker
    /// must list `.ai` under, so testing [`fileio::open_filters`] directly would not have
    /// caught a route that asked with a narrower one.
    #[test]
    fn file_open_hands_the_picker_filters_that_include_ai() {
        use crate::android_services;
        use std::sync::mpsc;
        use vectorcraft_ui_egui::VectorcraftApp;

        let seen: Rc<RefCell<Vec<PickRequest>>> = Rc::default();
        let recorder = seen.clone();
        // `super::channel()` is this module's picker state, not a message channel.
        let mut services = android_services(super::channel());
        // Stand in for the picker: record what was asked, answer "still waiting".
        services.start_pick = Some(Box::new(move |request: PickRequest| {
            recorder.borrow_mut().push(request);
            let (_tx, rx) = mpsc::channel::<PickAnswer>();
            Some(rx)
        }));
        let mut app = VectorcraftApp::new(vectorcraft_engine::Session::new(), services);
        // The command runs as an "entry", which is what lets the app ask the host at all.
        let out = app.run("file.open", serde_json::json!({}));

        // The dialog is deferred, so the command gives up for now — `Err("cancelled")` is the
        // contract, and `picks::poll` scrubs that status while the picker is open. What
        // matters is that the host was asked, and with which filters.
        assert_eq!(out, Err("cancelled".to_string()), "a deferred dialog leaves the command cancelled for now");
        let asked = seen.borrow();
        assert_eq!(asked.len(), 1, "file.open asked the host for exactly one dialog: {asked:?}");
        let PickRequest::Open(pick) = &asked[0] else {
            panic!("file.open asks to open a file, not {:?}", asked[0]);
        };
        let exts = extensions_of(&pick.filters).expect("the Open dialog filters offer extensions");
        assert!(exts.iter().any(|e| e == "ai"), "file.open's filters must offer .ai: {exts:?}");
    }
}
