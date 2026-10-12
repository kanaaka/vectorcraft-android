# No patch

This port changes **no upstream file**. VectorCraft's UI crate already exposes the seams the Android
shell needs — `Services::start_pick` for the dialogs, `vectorcraft_text::set_platform_font_files` and
`set_app_font_dir` for fonts — so everything the port needs lives in `apps/vectorcraft-android/`, and
building it is a matter of copying that directory into a checkout of the pinned commit.

Every other port in this set needs at least a small patch here; this one does not.
