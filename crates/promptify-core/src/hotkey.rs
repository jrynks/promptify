use std::time::{Duration, Instant};

use crate::pipeline::Mode;

/// Releasing sooner than this after pressing latches recording on until the next press.
pub const TAP_THRESHOLD: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureAction {
    Start(Mode),
    Stop,
    /// A quick tap: keep recording until the same hotkey is pressed again.
    Latch,
    Ignore,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    Holding { mode: Mode, since: Instant },
    Latched { mode: Mode },
}

/// Hold-to-talk with tap-to-toggle fallback for one hotkey per mode.
#[derive(Debug, Default)]
pub struct Gesture {
    state: State,
}

impl Gesture {
    pub fn press(&mut self, mode: Mode, now: Instant) -> GestureAction {
        match self.state {
            State::Idle => {
                self.state = State::Holding { mode, since: now };
                GestureAction::Start(mode)
            }
            State::Latched { mode: latched } if latched == mode => {
                self.state = State::Idle;
                GestureAction::Stop
            }
            // Key auto-repeat, or the other mode's hotkey while recording.
            _ => GestureAction::Ignore,
        }
    }

    pub fn release(&mut self, mode: Mode, now: Instant) -> GestureAction {
        match self.state {
            State::Holding { mode: held, since } if held == mode => {
                if now.saturating_duration_since(since) < TAP_THRESHOLD {
                    self.state = State::Latched { mode };
                    GestureAction::Latch
                } else {
                    self.state = State::Idle;
                    GestureAction::Stop
                }
            }
            _ => GestureAction::Ignore,
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_millis(900);

    #[test]
    fn hold_then_release_stops() {
        let t = Instant::now();
        let mut g = Gesture::default();
        assert_eq!(g.press(Mode::Prompt, t), GestureAction::Start(Mode::Prompt));
        assert_eq!(g.press(Mode::Prompt, t + LONG / 2), GestureAction::Ignore);
        assert_eq!(g.release(Mode::Prompt, t + LONG), GestureAction::Stop);
        assert_eq!(g.release(Mode::Prompt, t + LONG), GestureAction::Ignore);
    }

    #[test]
    fn tap_latches_until_next_press() {
        let t = Instant::now();
        let mut g = Gesture::default();
        g.press(Mode::Dictation, t);
        assert_eq!(g.release(Mode::Dictation, t + Duration::from_millis(100)), GestureAction::Latch);
        assert_eq!(g.press(Mode::Prompt, t + LONG), GestureAction::Ignore);
        assert_eq!(g.press(Mode::Dictation, t + LONG), GestureAction::Stop);
        assert_eq!(g.release(Mode::Dictation, t + LONG), GestureAction::Ignore);
        assert_eq!(g.press(Mode::Prompt, t + LONG * 2), GestureAction::Start(Mode::Prompt));
    }

    #[test]
    fn other_mode_does_not_interrupt_and_reset_returns_to_idle() {
        let t = Instant::now();
        let mut g = Gesture::default();
        g.press(Mode::Prompt, t);
        assert_eq!(g.press(Mode::Dictation, t), GestureAction::Ignore);
        assert_eq!(g.release(Mode::Dictation, t + LONG), GestureAction::Ignore);
        g.reset();
        assert_eq!(g.press(Mode::Dictation, t + LONG), GestureAction::Start(Mode::Dictation));
    }
}
