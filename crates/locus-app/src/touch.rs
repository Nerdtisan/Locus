// SPDX-License-Identifier: AGPL-3.0-or-later
//! Touch and pen input classification (epic #30, #250; SPEC §0 "touch/pen is
//! an M4 epic").
//!
//! The canvas keeps its mouse behaviour exactly; this module only decides
//! *whether pointer input may reach the tools*. egui turns the first finger
//! (or the pen) into an ordinary primary pointer, so one-finger draw/select
//! needs no special handling. What needs handling is the second contact:
//!
//! * two fingers are a pinch/pan gesture, never a tool action;
//! * once a second finger has landed, nothing draws until every finger is
//!   up (a palm or a stray second touch can't start or finish a stroke);
//! * a pen contact is precise and wins: other touches (a resting palm) are
//!   ignored while it is down, and pressure is never read.
//!
//! No `Ui` here, so the state machine is unit-tested with plain events.

use std::collections::BTreeMap;

use egui::{Event, TouchPhase};

/// What kind of contact produced the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contact {
    Finger,
    Pen,
}

impl Contact {
    /// egui doesn't expose the device type, only an optional `force`. A pen
    /// reports a graded pressure strictly between 0 and 1; fingers report
    /// none, 0 or a saturated 1. A heuristic, but a wrong guess only changes
    /// which precise/coarse rules apply, never what the document does.
    pub fn from_force(force: Option<f32>) -> Self {
        match force {
            Some(f) if f > 0.0 && f < 1.0 => Self::Pen,
            _ => Self::Finger,
        }
    }
}

/// Tracks live contacts across frames.
#[derive(Debug, Default)]
pub struct Touches {
    active: BTreeMap<u64, Contact>,
    /// A second finger landed during this touch sequence.
    latched: bool,
}

impl Touches {
    /// Fold one frame's events in. Returns `true` when pointer input must not
    /// reach the tools this frame (a multi-finger sequence is in progress,
    /// including the frame its last finger lifts, so the release can't count
    /// as a click).
    pub fn frame<'a>(&mut self, events: impl IntoIterator<Item = &'a Event>) -> bool {
        for ev in events {
            if let Event::Touch {
                id, phase, force, ..
            } = ev
            {
                match phase {
                    TouchPhase::Start => {
                        self.active.insert(id.0, Contact::from_force(*force));
                    }
                    TouchPhase::Move => {
                        // A later force reading can reveal a pen.
                        if let Some(c) = self.active.get_mut(&id.0)
                            && Contact::from_force(*force) == Contact::Pen
                        {
                            *c = Contact::Pen;
                        }
                    }
                    TouchPhase::End | TouchPhase::Cancel => {
                        self.active.remove(&id.0);
                    }
                }
            }
        }
        if self.fingers() >= 2 && !self.pen_down() {
            self.latched = true;
        }
        let suppress = self.latched;
        if self.active.is_empty() {
            self.latched = false;
        }
        suppress
    }

    pub fn fingers(&self) -> usize {
        self.active
            .values()
            .filter(|c| **c == Contact::Finger)
            .count()
    }

    pub fn pen_down(&self) -> bool {
        self.active.values().any(|c| *c == Contact::Pen)
    }

    /// Any touch or pen contact is down right now.
    #[cfg(test)]
    pub fn any(&self) -> bool {
        !self.active.is_empty()
    }

    /// The pinch/pan gesture may drive the view: two or more fingers and no
    /// pen (a palm beside a pen must not zoom the canvas).
    pub fn gesture_allowed(&self) -> bool {
        self.fingers() >= 2 && !self.pen_down()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{TouchDeviceId, TouchId, pos2};

    fn t(id: u64, phase: TouchPhase, force: Option<f32>) -> Event {
        Event::Touch {
            device_id: TouchDeviceId(0),
            id: TouchId(id),
            phase,
            pos: pos2(0.0, 0.0),
            force,
        }
    }

    #[test]
    fn one_finger_reaches_the_tools() {
        let mut s = Touches::default();
        assert!(!s.frame(&[t(1, TouchPhase::Start, None)]));
        assert!(!s.frame(&[t(1, TouchPhase::Move, None)]));
        assert!(!s.frame(&[t(1, TouchPhase::End, None)]));
        assert!(!s.any());
    }

    #[test]
    fn second_finger_suppresses_until_all_are_up() {
        let mut s = Touches::default();
        assert!(!s.frame(&[t(1, TouchPhase::Start, None)]));
        assert!(s.frame(&[t(2, TouchPhase::Start, None)]));
        assert!(s.gesture_allowed());
        // Lifting the second finger leaves the first one unable to draw.
        assert!(s.frame(&[t(2, TouchPhase::End, None)]));
        assert!(!s.gesture_allowed());
        assert!(s.frame(&[t(1, TouchPhase::Move, None)]));
        // The frame the last finger lifts is still suppressed (no click)...
        assert!(s.frame(&[t(1, TouchPhase::End, None)]));
        // ...and the next touch is a normal one again.
        assert!(!s.frame(&[t(3, TouchPhase::Start, None)]));
    }

    #[test]
    fn cancel_ends_a_contact() {
        let mut s = Touches::default();
        s.frame(&[t(1, TouchPhase::Start, None), t(2, TouchPhase::Start, None)]);
        s.frame(&[
            t(1, TouchPhase::Cancel, None),
            t(2, TouchPhase::Cancel, None),
        ]);
        assert!(!s.any());
        assert!(!s.frame(&[]));
    }

    #[test]
    fn pen_is_precise_and_ignores_a_resting_palm() {
        let mut s = Touches::default();
        assert!(!s.frame(&[t(1, TouchPhase::Start, Some(0.4))]));
        assert!(s.pen_down());
        // A palm lands while the pen is down: no gesture, no suppression.
        assert!(!s.frame(&[t(2, TouchPhase::Start, None)]));
        assert!(!s.gesture_allowed());
        assert!(!s.frame(&[t(1, TouchPhase::Move, Some(0.9))]));
    }

    #[test]
    fn pressure_never_changes_the_outcome() {
        for f in [0.1, 0.5, 0.99] {
            let mut s = Touches::default();
            assert!(!s.frame(&[t(1, TouchPhase::Start, Some(f))]));
            assert!(s.pen_down());
        }
    }

    #[test]
    fn force_classification() {
        assert_eq!(Contact::from_force(None), Contact::Finger);
        assert_eq!(Contact::from_force(Some(0.0)), Contact::Finger);
        assert_eq!(Contact::from_force(Some(1.0)), Contact::Finger);
        assert_eq!(Contact::from_force(Some(0.3)), Contact::Pen);
    }
}
