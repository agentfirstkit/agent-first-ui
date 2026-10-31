//! Retained event frames and reconnect replay boundaries.
use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct EventFrame {
    pub(super) sequence: u64,
    pub(super) encoded: Arc<str>,
}

pub(super) struct EventHistory {
    frames: VecDeque<EventFrame>,
    last_sequence: u64,
    capacity: usize,
}

pub(super) struct EventReplay {
    pub(super) frames: Vec<EventFrame>,
    pub(super) reset: Option<(u64, u64)>,
}

impl EventHistory {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(capacity),
            last_sequence: 0,
            capacity,
        }
    }

    pub(super) fn next_sequence(&self) -> u64 {
        self.last_sequence.saturating_add(1)
    }

    pub(super) fn retain(&mut self, frame: EventFrame) {
        if self.frames.len() == self.capacity {
            self.frames.pop_front();
        }
        self.last_sequence = frame.sequence;
        self.frames.push_back(frame);
    }

    pub(super) fn replay(&self, after: u64) -> EventReplay {
        let Some(oldest) = self.frames.front().map(|frame| frame.sequence) else {
            return EventReplay {
                frames: Vec::new(),
                reset: (after > 0).then_some((0, 0)),
            };
        };
        let latest = self.frames.back().map_or(oldest, |frame| frame.sequence);
        if after > latest || after.saturating_add(1) < oldest {
            return EventReplay {
                frames: Vec::new(),
                reset: Some((oldest, latest)),
            };
        }
        EventReplay {
            frames: self
                .frames
                .iter()
                .filter(|frame| frame.sequence > after)
                .cloned()
                .collect(),
            reset: None,
        }
    }
}
