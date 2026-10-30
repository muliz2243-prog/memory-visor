use crate::model::events::TraceEvent;
use crate::model::memory::MemoryState;
use crate::model::outcome::{Application, ApplicationOutcome, Effect, Incident};
use crate::model::program::ProgramTree;
use crate::stages::apply::apply_event;

#[derive(Clone, PartialEq, Debug)]
pub struct TimelineStep {
    pub effects: Vec<Effect>,
    /// Present when the event was rejected. The state did not change.
    pub incident: Option<Incident>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Timeline {
    pub states: Vec<MemoryState>,
    pub steps: Vec<TimelineStep>,
}

impl Timeline {
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Memory after `step` events. `0` is the empty state.
    pub fn state_at(&self, step: usize) -> Option<&MemoryState> {
        self.states.get(step)
    }

    pub fn rejection_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.incident.is_some())
            .count()
    }
}

pub fn build_timeline(program: &ProgramTree, events: &[TraceEvent]) -> Timeline {
    let initial = MemoryState::empty();
    let mut states = Vec::with_capacity(events.len() + 1);
    states.push(initial.clone());
    let mut steps = Vec::with_capacity(events.len());
    let mut current = initial;
    for event in events {
        let outcome = apply_event(Application {
            state: current,
            event,
            program,
        });
        match outcome {
            ApplicationOutcome::Applied { state, effects } => {
                current = state;
                steps.push(TimelineStep {
                    effects,
                    incident: None,
                });
            }
            ApplicationOutcome::Rejected { state, incident } => {
                current = state;
                steps.push(TimelineStep {
                    effects: Vec::new(),
                    incident: Some(incident),
                });
            }
        }
        states.push(current.clone());
    }
    Timeline { states, steps }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ids::{ObjectId, Sequence};
    use crate::model::memory::{ObjectShape, Path, PrimitiveValue, Segment, Value};
    use crate::stages::analyze::{Analysis, analyze_program};

    const SOURCE: &str = "\
fn main() {
    let box: [i32; 2] = [1, 2];
}
";

    fn program() -> ProgramTree {
        match analyze_program(SOURCE) {
            Analysis::Analyzed(tree) => tree,
            Analysis::Failed(error) => panic!("unexpected error: {:?}", error),
        }
    }

    fn int(value: i64) -> Value {
        Value::Primitive(PrimitiveValue::Integer(value))
    }

    #[test]
    fn builds_one_state_per_event() {
        let program = program();
        let main = program.entry;
        let events = vec![
            TraceEvent::StartFunction {
                sequence: Sequence(1),
                function: main,
                call_line: 1,
                arguments: Vec::new(),
            },
            TraceEvent::CreateObject {
                sequence: Sequence(2),
                line: 2,
                object: ObjectId(1),
                label: "box".to_string(),
                shape: ObjectShape::Sequence {
                    element_type: crate::model::program::DataType::Integer,
                    elements: vec![int(1), int(2)],
                },
            },
            TraceEvent::DeclareVariable {
                sequence: Sequence(3),
                line: 2,
                name: "box".to_string(),
                value: Value::Reference(ObjectId(1)),
            },
        ];
        let timeline = build_timeline(&program, &events);
        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline.states.len(), 4);
        assert_eq!(timeline.rejection_count(), 0);
        assert!(timeline.state_at(0).unwrap().frames.is_empty());
        assert_eq!(timeline.state_at(3).unwrap().frames.len(), 1);
    }

    #[test]
    fn replays_deterministically() {
        let program = program();
        let events = vec![
            TraceEvent::StartFunction {
                sequence: Sequence(1),
                function: program.entry,
                call_line: 1,
                arguments: Vec::new(),
            },
            TraceEvent::WriteMemory {
                sequence: Sequence(2),
                line: 2,
                path: Path::Object {
                    object: ObjectId(1),
                    segments: vec![Segment::Index(7)],
                },
                value: int(1),
            },
        ];
        let first = build_timeline(&program, &events);
        let second = build_timeline(&program, &events);
        assert_eq!(first, second);
        assert_eq!(first.rejection_count(), 1);
    }

    #[test]
    fn rejection_keeps_memory_and_pins_the_cell() {
        let program = program();
        let events = vec![
            TraceEvent::StartFunction {
                sequence: Sequence(1),
                function: program.entry,
                call_line: 1,
                arguments: Vec::new(),
            },
            TraceEvent::CreateObject {
                sequence: Sequence(2),
                line: 2,
                object: ObjectId(1),
                label: "box".to_string(),
                shape: ObjectShape::Sequence {
                    element_type: crate::model::program::DataType::Integer,
                    elements: vec![int(1), int(2)],
                },
            },
            TraceEvent::WriteMemory {
                sequence: Sequence(3),
                line: 2,
                path: Path::Object {
                    object: ObjectId(1),
                    segments: vec![Segment::Index(7)],
                },
                value: int(1),
            },
        ];
        let timeline = build_timeline(&program, &events);
        let before = timeline.state_at(2).unwrap();
        let after = timeline.state_at(3).unwrap();
        assert_eq!(before.frames, after.frames);
        assert_eq!(before.heap, after.heap);
        assert_eq!(after.incidents.len(), 1);
        match after.incidents[0].location() {
            crate::model::memory::Location::Cell(Path::Object { object, segments }) => {
                assert_eq!(*object, ObjectId(1));
                assert_eq!(segments.as_slice(), &[Segment::Index(7)]);
            }
            other => panic!("expected a pinned cell, got {:?}", other),
        }
    }
}
