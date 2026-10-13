//! Modifier keys, tracked by the shell because the platform never reports them.
//!
//! # The gap
//!
//! winit 0.30.13's Android backend emits exactly six window events — `Focused`, `Resized`,
//! `ScaleFactorChanged`, `RedrawRequested`, `Touch` and `KeyboardInput`
//! (`src/platform_impl/android/mod.rs`) — and **never** `WindowEvent::ModifiersChanged`. egui-winit
//! updates its modifier state from that one event and nowhere else, and egui 0.36 carries
//! modifiers as an *event* (`RawInput` has no `modifiers` field any more), so on Android
//! `ctx.input(|i| i.modifiers)` is the all-false default for the whole run:
//!
//! - Shift+click selects a single object instead of extending the selection,
//! - Ctrl+C / Ctrl+V / Ctrl+S and every other modifier shortcut does nothing,
//! - Alt keytips never come up.
//!
//! The *key events themselves* do arrive, so the modifiers can be tracked from them — which is what
//! a desktop backend does internally. This module does that and reports the state the way egui
//! expects: `Event::ModifiersChanged` (egui's `InputState` reads modifiers from that event alone)
//! plus the per-event `modifiers` copies that `Event::Key` and `Event::PointerButton` carry.
//!
//! Call [`Tracked::apply`] from the shell's `eframe::App::raw_input_hook`, *before* forwarding to
//! the app's own hook, so the app sees the same frames it would on the desktop.
//!
//! # What it does not do
//!
//! The Super/Windows key is deliberately not tracked: on Windows and Linux egui's `command`
//! modifier is Ctrl and `mac_cmd` is documented as macOS-only, so there is no modifier here for it
//! to become. Alt is tracked and reported — it is what opens keytips — but egui's own note that
//! Alt+Shift is how macOS types special characters does not apply on Android.

/// The three modifiers that mean something on this platform, held between frames.
///
/// Between frames because the key press and the click are usually in *different* frames: the user
/// holds Shift down, and only then taps a cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tracked {
    alt: bool,
    ctrl: bool,
    shift: bool,
}

impl Tracked {
    /// Fold this frame's events into the tracked state and report it back into them.
    pub fn apply(&mut self, raw: &mut egui::RawInput) {
        let events = std::mem::take(&mut raw.events);
        let mut out = Vec::with_capacity(events.len() + 1);
        for mut event in events {
            self.observe(&event);
            // Every event that carries a copy gets the state as it stands at that point in the
            // batch, so a Shift press and a click in the same frame are ordered correctly.
            let modifiers = self.reported();
            match &mut event {
                egui::Event::Key { modifiers: m, .. } | egui::Event::PointerButton { modifiers: m, .. } => *m = modifiers,
                _ => {}
            }
            out.push(event);
        }
        // egui's `InputState` takes modifiers from this event and nowhere else, and a frame with no
        // key events in it — the vast majority — still has to report the state it is holding.
        out.push(egui::Event::ModifiersChanged(self.reported()));
        raw.events = out;
    }

    /// The state as egui wants to see it.
    fn reported(&self) -> egui::Modifiers {
        // On Windows and Linux the shortcut modifier is Ctrl, and `mac_cmd` must stay off there.
        egui::Modifiers { alt: self.alt, ctrl: self.ctrl, shift: self.shift, mac_cmd: false, command: self.ctrl }
    }

    /// Update from one event.
    fn observe(&mut self, event: &egui::Event) {
        match event {
            // A modifier key going down or coming up. Physical keys: winit reports the left and
            // right keys separately, and either of them is "the" modifier.
            egui::Event::Key { key, pressed, .. } => match key {
                egui::Key::ShiftLeft | egui::Key::ShiftRight => self.shift = *pressed,
                egui::Key::ControlLeft | egui::Key::ControlRight => self.ctrl = *pressed,
                egui::Key::AltLeft | egui::Key::AltRight => self.alt = *pressed,
                _ => {}
            },
            // The platform reported them itself. It never does on Android, but egui-winit
            // synthesises one when the window loses focus, and that report is authoritative: it is
            // how a modifier that was held while the app went to the background stops being stuck.
            egui::Event::ModifiersChanged(reported) => {
                self.shift = reported.shift;
                self.ctrl = reported.ctrl;
                self.alt = reported.alt;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers, PointerButton, RawInput};

    fn key(k: Key, pressed: bool) -> Event {
        Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: Modifiers::default() }
    }

    fn click() -> Event {
        Event::PointerButton {
            pos: egui::pos2(10.0, 10.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::default(),
        }
    }

    /// Only the synthesised event carries them: egui reads modifiers from `ModifiersChanged` and
    /// from nowhere else, so a frame must never end without one.
    fn reported(raw: &RawInput) -> Modifiers {
        match raw.events.last() {
            Some(Event::ModifiersChanged(m)) => *m,
            _ => panic!("every frame must end with ModifiersChanged: {:?}", raw.events),
        }
    }

    fn run(t: &mut Tracked, events: Vec<Event>) -> RawInput {
        let mut raw = RawInput { events, ..Default::default() };
        t.apply(&mut raw);
        raw
    }

    /// The `modifiers` copy that a key or pointer event carries (only those two variants have one).
    fn carried(event: &Event) -> Modifiers {
        match event {
            Event::Key { modifiers, .. } | Event::PointerButton { modifiers, .. } => *modifiers,
            other => panic!("no modifiers on {other:?}"),
        }
    }

    /// The whole point: a frame with no key events in it still has to report the state being held.
    #[test]
    fn a_held_modifier_is_reported_in_frames_with_no_events() {
        let mut t = Tracked::default();
        run(&mut t, vec![key(Key::ShiftLeft, true)]);
        let quiet = run(&mut t, vec![]);
        assert!(reported(&quiet).shift, "Shift is still held");
        assert!(!reported(&quiet).ctrl);
        // An empty raw input's only event is the report, so the frame is not empty of key state.
        assert_eq!(quiet.events.len(), 1, "{:?}", quiet.events);
    }

    /// Shift down, then a click: the click must carry Shift, and the press/release must settle.
    #[test]
    fn a_click_carries_the_modifier_that_was_held() {
        let mut t = Tracked::default();
        let press = run(&mut t, vec![click()]);
        assert!(!carried(&press.events[0]).shift, "no modifier yet");
        run(&mut t, vec![key(Key::ShiftLeft, true)]);
        let click2 = run(&mut t, vec![click()]);
        assert!(carried(&click2.events[0]).shift, "the click carries Shift");
        run(&mut t, vec![key(Key::ShiftLeft, false)]);
        let after = run(&mut t, vec![click()]);
        assert!(!carried(&after.events[0]).shift, "released");
    }

    /// Order inside one batch: the press must apply to the events after it, not before.
    #[test]
    fn the_state_is_the_one_at_that_point_in_the_batch() {
        let mut t = Tracked::default();
        let raw = run(&mut t, vec![click(), key(Key::ControlLeft, true), click()]);
        assert!(!carried(&raw.events[0]).ctrl, "the first click came before the press");
        assert!(carried(&raw.events[2]).ctrl, "the second came after it");
    }

    /// Ctrl must become egui's *command* modifier, or Ctrl+S is not a shortcut — and `mac_cmd`
    /// must stay off, as it is on Windows and Linux.
    #[test]
    fn ctrl_is_the_command_modifier_and_not_mac_cmd() {
        let mut t = Tracked::default();
        run(&mut t, vec![key(Key::ControlLeft, true)]);
        let m = reported(&run(&mut t, vec![]));
        assert!(m.ctrl && m.command, "Ctrl is command: {m:?}");
        assert!(!m.mac_cmd, "mac_cmd stays off off-macOS: {m:?}");
    }

    /// A report from the platform is authoritative — that is how a modifier held while the app
    /// went to the background stops being stuck.
    #[test]
    fn a_platform_report_overrides_the_tracked_state() {
        let mut t = Tracked::default();
        run(&mut t, vec![key(Key::AltLeft, true)]);
        assert!(reported(&run(&mut t, vec![])).alt, "Alt is held");
        run(&mut t, vec![Event::ModifiersChanged(Modifiers::default())]);
        assert!(!reported(&run(&mut t, vec![])).alt, "the platform says nothing is held");
    }

    /// Both physical keys are "the" modifier, as a desktop backend has it.
    #[test]
    fn either_shift_key_is_shift() {
        for k in [Key::ShiftLeft, Key::ShiftRight] {
            let mut t = Tracked::default();
            run(&mut t, vec![key(k, true)]);
            assert!(reported(&run(&mut t, vec![])).shift, "{k:?}");
        }
    }

    /// Nothing else is invented.
    #[test]
    fn an_ordinary_key_changes_no_modifier() {
        let mut t = Tracked::default();
        let _ = run(&mut t, vec![key(Key::A, true)]);
        let m = reported(&run(&mut t, vec![]));
        assert_eq!(m, Modifiers::default(), "{m:?}");
    }
}
