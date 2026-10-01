//! "Hold Ctrl+Shift alone" as a push-to-talk key. It must never fire for shortcuts such as
//! Ctrl+Shift+T or for the quick Ctrl+Shift tap that switches keyboard layouts on Windows.

use std::time::{Duration, Instant};

/// Both modifiers must be held, with no other key, for this long before recording starts.
pub const ARM_DELAY: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordKey {
    Ctrl,
    Shift,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordAction {
    Press,
    Release,
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    Pending { since: Instant },
    Active,
    /// Another key was involved; wait until both modifiers are up before arming again.
    Spoiled,
}

#[derive(Debug, Default)]
pub struct ModifierChord {
    ctrl: bool,
    shift: bool,
    state: State,
}

impl ModifierChord {
    /// Which modifiers the chord believes are down, so the platform layer can repair missed key-ups.
    pub fn held(&self) -> (bool, bool) {
        (self.ctrl, self.shift)
    }

    pub fn key_down(&mut self, key: ChordKey, now: Instant) -> ChordAction {
        match key {
            ChordKey::Ctrl => self.ctrl = true,
            ChordKey::Shift => self.shift = true,
            ChordKey::Other => {
                if matches!(self.state, State::Pending { .. }) || (self.state == State::Idle && (self.ctrl || self.shift)) {
                    self.state = State::Spoiled;
                }
                return ChordAction::None;
            }
        }
        if self.state == State::Idle && self.ctrl && self.shift {
            self.state = State::Pending { since: now };
        }
        ChordAction::None
    }

    pub fn key_up(&mut self, key: ChordKey) -> ChordAction {
        match key {
            ChordKey::Ctrl => self.ctrl = false,
            ChordKey::Shift => self.shift = false,
            ChordKey::Other => return ChordAction::None,
        }
        let action = if self.state == State::Active { ChordAction::Release } else { ChordAction::None };
        if !self.ctrl && !self.shift {
            self.state = State::Idle;
        } else if self.state != State::Idle {
            self.state = State::Spoiled;
        }
        action
    }

    /// Called periodically; starts recording once the chord has been held alone long enough.
    pub fn tick(&mut self, now: Instant) -> ChordAction {
        if let State::Pending { since } = self.state
            && now.saturating_duration_since(since) >= ARM_DELAY
        {
            self.state = State::Active;
            return ChordAction::Press;
        }
        ChordAction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER: Duration = Duration::from_millis(400);

    #[test]
    fn holding_both_modifiers_alone_presses_then_releases() {
        let t = Instant::now();
        let mut c = ModifierChord::default();
        assert_eq!(c.key_down(ChordKey::Ctrl, t), ChordAction::None);
        assert_eq!(c.key_down(ChordKey::Shift, t), ChordAction::None);
        assert_eq!(c.tick(t + Duration::from_millis(100)), ChordAction::None);
        assert_eq!(c.key_down(ChordKey::Shift, t + Duration::from_millis(200)), ChordAction::None, "auto-repeat");
        assert_eq!(c.tick(t + AFTER), ChordAction::Press);
        assert_eq!(c.tick(t + AFTER * 2), ChordAction::None);
        assert_eq!(c.key_up(ChordKey::Shift), ChordAction::Release);
        assert_eq!(c.key_up(ChordKey::Ctrl), ChordAction::None);
    }

    #[test]
    fn quick_layout_switch_tap_never_fires() {
        let t = Instant::now();
        let mut c = ModifierChord::default();
        c.key_down(ChordKey::Ctrl, t);
        c.key_down(ChordKey::Shift, t);
        assert_eq!(c.key_up(ChordKey::Shift), ChordAction::None);
        assert_eq!(c.key_up(ChordKey::Ctrl), ChordAction::None);
        assert_eq!(c.tick(t + AFTER), ChordAction::None);
    }

    #[test]
    fn shortcuts_with_another_key_never_fire() {
        let t = Instant::now();
        let mut c = ModifierChord::default();
        c.key_down(ChordKey::Ctrl, t);
        c.key_down(ChordKey::Shift, t);
        c.key_down(ChordKey::Other, t + Duration::from_millis(100));
        assert_eq!(c.tick(t + AFTER), ChordAction::None, "Ctrl+Shift+T");
        c.key_up(ChordKey::Other);
        assert_eq!(c.tick(t + AFTER * 3), ChordAction::None, "still spoiled while modifiers are held");
        c.key_up(ChordKey::Shift);
        c.key_down(ChordKey::Shift, t + AFTER * 4);
        assert_eq!(c.tick(t + AFTER * 6), ChordAction::None, "re-pressing one modifier does not re-arm");
        c.key_up(ChordKey::Shift);
        c.key_up(ChordKey::Ctrl);
        c.key_down(ChordKey::Ctrl, t + AFTER * 7);
        c.key_down(ChordKey::Shift, t + AFTER * 7);
        assert_eq!(c.tick(t + AFTER * 8), ChordAction::Press, "clean chord after full release works");
    }

    #[test]
    fn a_key_held_before_the_modifiers_spoils_the_chord() {
        let t = Instant::now();
        let mut c = ModifierChord::default();
        c.key_down(ChordKey::Ctrl, t);
        c.key_down(ChordKey::Other, t);
        c.key_down(ChordKey::Shift, t);
        assert_eq!(c.tick(t + AFTER), ChordAction::None, "Ctrl+A then Shift");
    }

    #[test]
    fn other_keys_while_recording_do_not_stop_it() {
        let t = Instant::now();
        let mut c = ModifierChord::default();
        c.key_down(ChordKey::Ctrl, t);
        c.key_down(ChordKey::Shift, t);
        assert_eq!(c.tick(t + AFTER), ChordAction::Press);
        assert_eq!(c.key_down(ChordKey::Other, t + AFTER), ChordAction::None);
        assert_eq!(c.key_up(ChordKey::Ctrl), ChordAction::Release);
        assert_eq!(c.tick(t + AFTER * 3), ChordAction::None, "no re-press while Shift is still held");
    }
}
