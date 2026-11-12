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

use egui::{Event, Pos2, TouchPhase};

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
    /// The lone finger that might become a long press.
    hold: Option<Hold>,
    /// A long press fired in this sequence; its release must not click.
    long_fired: bool,
    /// Last time a finger (not a pen) was seen, for the coarse tolerance.
    last_finger: Option<f64>,
}

/// A finger held down, not yet moved past the slop.
#[derive(Clone, Copy, Debug)]
struct Hold {
    id: u64,
    start: Pos2,
    since: f64,
}

/// Hold time for a long press, in seconds.
pub const LONG_PRESS_S: f64 = 0.5;
/// How far a held finger may drift and still be a long press, in points.
const HOLD_SLOP_PT: f32 = 10.0;
/// A finger lifted this recently still counts as the pointer's source.
const COARSE_GRACE_S: f64 = 1.5;

/// What one frame of touch input means for the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    /// Pointer input must not reach the tools (gesture or long press).
    pub suppress: bool,
    /// A long press completed at this screen position.
    pub long_press: Option<Pos2>,
}

impl Touches {
    /// [`Self::frame`] plus long-press detection (#251). `now` is egui time
    /// in seconds. A long press needs one finger held still; it fires once
    /// per touch sequence, never for a pen, and a second finger cancels it.
    pub fn update(&mut self, events: &[Event], now: f64) -> Step {
        let mut seen = self.active.len();
        for ev in events {
            let Event::Touch {
                id,
                phase,
                pos,
                force,
                ..
            } = ev
            else {
                continue;
            };
            if Contact::from_force(*force) == Contact::Finger {
                self.last_finger = Some(now);
            }
            match phase {
                TouchPhase::Start => {
                    let finger = Contact::from_force(*force) == Contact::Finger;
                    self.hold = (seen == 0 && finger).then_some(Hold {
                        id: id.0,
                        start: *pos,
                        since: now,
                    });
                    seen += 1;
                }
                TouchPhase::Move => {
                    if let Some(h) = self.hold
                        && h.id == id.0
                        && (*pos - h.start).length() > HOLD_SLOP_PT
                    {
                        self.hold = None;
                    }
                }
                TouchPhase::End | TouchPhase::Cancel => {
                    seen = seen.saturating_sub(1);
                    if self.hold.is_some_and(|h| h.id == id.0) {
                        self.hold = None;
                    }
                }
            }
        }
        let gesture = self.frame(events);
        if gesture || self.pen_down() {
            self.hold = None;
        }
        let mut long_press = None;
        if let Some(h) = self.hold
            && now - h.since >= LONG_PRESS_S
        {
            long_press = Some(h.start);
            self.hold = None;
            self.long_fired = true;
        }
        let suppress = gesture || self.long_fired;
        if self.active.is_empty() {
            self.long_fired = false;
        }
        Step {
            suppress,
            long_press,
        }
    }

    /// Seconds until a pending long press fires, to schedule a repaint.
    pub fn long_press_due(&self, now: f64) -> Option<f64> {
        self.hold.map(|h| (h.since + LONG_PRESS_S - now).max(0.0))
    }

    /// The pointer is driven by a finger (down now, or lifted a moment ago):
    /// selection uses the larger touch tolerance. Mouse and pen never do.
    pub fn coarse(&self, now: f64) -> bool {
        self.fingers() > 0 || self.last_finger.is_some_and(|t| now - t < COARSE_GRACE_S)
    }

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

    fn at(id: u64, phase: TouchPhase, x: f32) -> Event {
        Event::Touch {
            device_id: TouchDeviceId(0),
            id: TouchId(id),
            phase,
            pos: pos2(x, 0.0),
            force: None,
        }
    }

    #[test]
    fn long_press_fires_once_and_swallows_the_release() {
        let mut s = Touches::default();
        assert_eq!(
            s.update(&[at(1, TouchPhase::Start, 5.0)], 0.0),
            Step::default()
        );
        assert_eq!(s.long_press_due(0.2), Some(LONG_PRESS_S - 0.2));
        let step = s.update(&[], 0.6);
        assert_eq!(step.long_press, Some(pos2(5.0, 0.0)));
        assert!(step.suppress);
        // Held on: no second fire; release is still suppressed (no click).
        assert_eq!(s.update(&[], 1.0).long_press, None);
        assert!(s.update(&[at(1, TouchPhase::End, 5.0)], 1.1).suppress);
        // Next tap is ordinary.
        assert!(!s.update(&[at(2, TouchPhase::Start, 5.0)], 2.0).suppress);
    }

    #[test]
    fn moving_or_a_second_finger_cancels_the_hold() {
        let mut s = Touches::default();
        s.update(&[at(1, TouchPhase::Start, 0.0)], 0.0);
        s.update(&[at(1, TouchPhase::Move, 30.0)], 0.1);
        assert_eq!(s.update(&[], 1.0).long_press, None);

        let mut s = Touches::default();
        s.update(&[at(1, TouchPhase::Start, 0.0)], 0.0);
        s.update(&[at(2, TouchPhase::Start, 50.0)], 0.1);
        assert_eq!(s.update(&[], 1.0).long_press, None);

        // A short tap never fires.
        let mut s = Touches::default();
        s.update(&[at(1, TouchPhase::Start, 0.0)], 0.0);
        s.update(&[at(1, TouchPhase::End, 0.0)], 0.2);
        assert_eq!(s.update(&[], 1.0).long_press, None);
    }

    #[test]
    fn pen_never_long_presses() {
        let mut s = Touches::default();
        let pen = Event::Touch {
            device_id: TouchDeviceId(0),
            id: TouchId(1),
            phase: TouchPhase::Start,
            pos: pos2(0.0, 0.0),
            force: Some(0.4),
        };
        s.update(&[pen], 0.0);
        assert_eq!(s.update(&[], 2.0).long_press, None);
    }

    #[test]
    fn coarse_only_after_a_finger() {
        let mut s = Touches::default();
        assert!(!s.coarse(0.0)); // mouse-only session
        s.update(&[at(1, TouchPhase::Start, 0.0)], 1.0);
        assert!(s.coarse(1.0));
        s.update(&[at(1, TouchPhase::End, 0.0)], 1.1);
        assert!(s.coarse(2.0));
        assert!(!s.coarse(3.0));
        // A pen is precise.
        let mut s = Touches::default();
        let pen = Event::Touch {
            device_id: TouchDeviceId(0),
            id: TouchId(1),
            phase: TouchPhase::Start,
            pos: pos2(0.0, 0.0),
            force: Some(0.4),
        };
        s.update(&[pen], 0.0);
        assert!(!s.coarse(0.0));
    }
}
