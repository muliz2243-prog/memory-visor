use crate::model::events::TraceEvent;
use crate::model::ids::Sequence;

/// A trace is only replayable if time moves forward.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HistoryError {
    /// The new event does not carry a strictly greater sequence.
    OutOfOrder {
        last: Option<Sequence>,
        received: Sequence,
    },
}

#[derive(Clone, PartialEq, Debug)]
pub struct History {
    steps: Vec<TraceEvent>,
    /// Between `0` and `len`, inclusive.
    cursor: usize,
}

impl History {
    pub fn new() -> Self {
        History {
            steps: Vec::new(),
            cursor: 0,
        }
    }

    /// Appends an event and leaves the cursor at the live edge. Sequences must
    /// start at 1 and strictly increase; anything else is refused untouched.
    pub fn push(&mut self, event: TraceEvent) -> Result<usize, HistoryError> {
        let received = event.sequence();
        let last = self.last_sequence();
        let valid = match last {
            Some(last) => received > last,
            None => received.0 > 0,
        };
        if !valid {
            return Err(HistoryError::OutOfOrder { last, received });
        }
        self.steps.push(event);
        self.cursor = self.steps.len();
        Ok(self.cursor)
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Moves the cursor, clamped to `0..=len`. Returns the position it ended at.
    pub fn set_cursor(&mut self, step: usize) -> usize {
        self.cursor = step.min(self.steps.len());
        self.cursor
    }

    /// Advances one event. Returns the event the cursor now sits after.
    // `next` is not Iterator::next: it moves a cursor and borrows the tape, it
    // does not consume it. The name matches the transport controls.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<&TraceEvent> {
        if self.cursor >= self.steps.len() {
            return None;
        }
        self.cursor += 1;
        self.current()
    }

    /// Rewinds one event. Returns the event the cursor now sits after, or
    /// `None` when it was already at the start.
    pub fn prev(&mut self) -> Option<&TraceEvent> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        self.current()
    }

    /// Event applied to reach `step`. Step 0 is the empty tape, so it has none.
    pub fn at(&self, step: usize) -> Option<&TraceEvent> {
        step.checked_sub(1).and_then(|index| self.steps.get(index))
    }

    pub fn current(&self) -> Option<&TraceEvent> {
        self.at(self.cursor)
    }

    pub fn events(&self) -> &[TraceEvent] {
        &self.steps
    }

    pub fn last_sequence(&self) -> Option<Sequence> {
        self.steps.last().map(TraceEvent::sequence)
    }

    pub fn at_start(&self) -> bool {
        self.cursor == 0
    }

    pub fn at_end(&self) -> bool {
        self.cursor == self.steps.len()
    }
}

impl Default for History {
    fn default() -> Self {
        History::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::events::ConditionOutcome;

    fn jump(sequence: u64) -> TraceEvent {
        TraceEvent::ConditionalJump {
            sequence: Sequence(sequence),
            from_line: 1,
            to_line: 2,
            expression: "ready".to_string(),
            outcome: ConditionOutcome::True,
        }
    }

    #[test]
    fn a_new_history_is_empty_and_before_the_first_step() {
        let history = History::new();
        assert!(history.is_empty());
        assert_eq!(history.len(), 0);
        assert_eq!(history.cursor(), 0);
        assert!(history.at_start());
        assert!(history.at_end());
        assert_eq!(history.current(), None);
        assert_eq!(history.last_sequence(), None);
    }

    #[test]
    fn pushing_leaves_the_cursor_at_the_live_edge() {
        let mut history = History::new();
        assert_eq!(history.push(jump(1)), Ok(1));
        assert_eq!(history.push(jump(2)), Ok(2));
        assert_eq!(history.len(), 2);
        assert_eq!(history.cursor(), 2);
        assert!(history.at_end());
        assert_eq!(history.last_sequence(), Some(Sequence(2)));
    }

    #[test]
    fn a_repeated_or_backwards_sequence_is_refused_untouched() {
        let mut history = History::new();
        history.push(jump(1)).expect("first event");
        history.push(jump(3)).expect("third event");

        let repeated = history.push(jump(3));
        assert_eq!(
            repeated,
            Err(HistoryError::OutOfOrder {
                last: Some(Sequence(3)),
                received: Sequence(3),
            })
        );
        let backwards = history.push(jump(2));
        assert_eq!(
            backwards,
            Err(HistoryError::OutOfOrder {
                last: Some(Sequence(3)),
                received: Sequence(2),
            })
        );
        assert_eq!(history.len(), 2, "a refused event must not be recorded");
        assert_eq!(history.cursor(), 2);
    }

    #[test]
    fn the_first_sequence_must_be_at_least_one() {
        let mut history = History::new();
        assert_eq!(
            history.push(jump(0)),
            Err(HistoryError::OutOfOrder {
                last: None,
                received: Sequence(0),
            })
        );
        assert!(history.is_empty());
    }

    #[test]
    fn advancing_and_rewinding_walk_the_tape() {
        let mut history = History::new();
        history.push(jump(1)).expect("first");
        history.push(jump(2)).expect("second");
        history.set_cursor(0);

        assert_eq!(history.next().map(TraceEvent::sequence), Some(Sequence(1)));
        assert_eq!(history.cursor(), 1);
        assert_eq!(history.next().map(TraceEvent::sequence), Some(Sequence(2)));
        assert_eq!(history.cursor(), 2);
        assert_eq!(history.next(), None);
        assert!(history.at_end());

        assert_eq!(history.prev().map(TraceEvent::sequence), Some(Sequence(1)));
        assert_eq!(history.cursor(), 1);
        assert_eq!(history.prev().map(TraceEvent::sequence), None);
        assert_eq!(history.cursor(), 0);
        assert!(history.at_start());
    }

    #[test]
    fn the_cursor_is_clamped_at_both_ends() {
        let mut history = History::new();
        history.push(jump(1)).expect("first");
        assert_eq!(history.set_cursor(99), 1);
        assert_eq!(history.set_cursor(0), 0);
        assert_eq!(history.at(99), None);
    }

    #[test]
    fn any_step_can_be_inspected_without_moving_the_cursor() {
        let mut history = History::new();
        history.push(jump(4)).expect("first");
        history.push(jump(7)).expect("second");
        history.set_cursor(1);

        assert_eq!(history.at(0), None);
        assert_eq!(history.at(1).map(TraceEvent::sequence), Some(Sequence(4)));
        assert_eq!(history.at(2).map(TraceEvent::sequence), Some(Sequence(7)));
        assert_eq!(history.at(3), None);
        assert_eq!(history.cursor(), 1, "at() must not move the cursor");
        assert_eq!(history.events().len(), 2);
    }
}
