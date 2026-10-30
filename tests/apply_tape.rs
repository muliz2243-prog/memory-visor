use memory_visor::model::events::TraceEvent;
use memory_visor::model::ids::{FrameId, FunctionId, ObjectId, Sequence, StructId};
use memory_visor::model::memory::{
    Field, Location, MAX_FRAME_DEPTH, MemoryState, ObjectShape, ObjectState, Path, PrimitiveValue,
    Segment, Value,
};
use memory_visor::model::outcome::{
    Application, ApplicationOutcome, Effect, Incident, RejectionReason,
};
use memory_visor::model::program::{DataType, ProgramTree};
use memory_visor::stages::analyze::{Analysis, analyze_program};
use memory_visor::stages::apply::apply_event;
#[cfg(test)]
mod reducer_tape {
    use super::*;

    const SOURCE: &str = "\
struct Point {
    x: i32,
}

fn helper(a: i32) -> i32 {
    let total: i32 = a;
    return total;
}

fn main() {
    let origin: Point = Point { x: 0 };
    let total: i32 = 0;
}
";

    fn program() -> ProgramTree {
        match analyze_program(SOURCE) {
            Analysis::Analyzed(tree) => tree,
            Analysis::Failed(error) => panic!("unexpected error: {:?}", error),
        }
    }

    fn helper_id(program: &ProgramTree) -> FunctionId {
        program.function_named("helper").expect("helper exists")
    }

    fn point_shape() -> ObjectShape {
        ObjectShape::Structure {
            structure: StructId(0),
            fields: vec![field("x", int(0))],
        }
    }

    fn field(name: &str, value: Value) -> Field {
        Field {
            name: name.to_string(),
            value,
        }
    }

    fn int(value: i64) -> Value {
        Value::Primitive(PrimitiveValue::Integer(value))
    }

    fn text(value: &str) -> Value {
        Value::Primitive(PrimitiveValue::Text(value.to_string()))
    }

    fn start_main(program: &ProgramTree) -> TraceEvent {
        TraceEvent::StartFunction {
            sequence: Sequence(1),
            function: program.entry,
            call_line: 10,
            arguments: Vec::new(),
        }
    }

    fn create_point(sequence: u64) -> TraceEvent {
        TraceEvent::CreateObject {
            sequence: Sequence(sequence),
            line: 11,
            object: ObjectId(1),
            label: "Point#1".to_string(),
            shape: point_shape(),
        }
    }

    fn run(state: MemoryState, event: &TraceEvent, program: &ProgramTree) -> ApplicationOutcome {
        apply_event(Application {
            state,
            event,
            program,
        })
    }

    fn applied(outcome: ApplicationOutcome) -> (MemoryState, Vec<Effect>) {
        match outcome {
            ApplicationOutcome::Applied { state, effects } => (state, effects),
            ApplicationOutcome::Rejected { incident, .. } => {
                panic!("expected an applied event, got {:?}", incident)
            }
        }
    }

    fn rejected(outcome: ApplicationOutcome) -> (MemoryState, Incident) {
        match outcome {
            ApplicationOutcome::Rejected { state, incident } => (state, incident),
            ApplicationOutcome::Applied { effects, .. } => {
                panic!("expected a rejection, got {:?}", effects)
            }
        }
    }

    fn reject_reason(incident: &Incident) -> &RejectionReason {
        match incident {
            Incident::InvalidEvent { reason, .. } => reason,
            Incident::RuntimeError { message, .. } => panic!("not a rejection: {}", message),
        }
    }

    #[test]
    fn start_function_pushes_a_frame_and_moves_the_caller() {
        let program = program();
        let (state, effects) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        assert_eq!(state.frames.len(), 1);
        assert_eq!(state.frames[0].current_line, 10);
        assert_eq!(effects, vec![Effect::FramePushed { frame: FrameId(1) }]);

        let call = TraceEvent::StartFunction {
            sequence: Sequence(2),
            function: helper_id(&program),
            call_line: 11,
            arguments: vec![field("a", int(4))],
        };
        let (state, _) = applied(run(state, &call, &program));
        assert_eq!(state.frames.len(), 2);
        assert_eq!(state.frames[0].current_line, 11);
        assert_eq!(state.frames[1].current_line, 5);
        assert_eq!(state.frames[1].variables[0].name, "a");
    }

    #[test]
    fn writing_a_field_changes_the_heap_cell() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let (state, _) = applied(run(state, &create_point(2), &program));
        let write = TraceEvent::WriteMemory {
            sequence: Sequence(3),
            line: 11,
            path: Path::Object {
                object: ObjectId(1),
                segments: vec![Segment::Field("x".to_string())],
            },
            value: int(7),
        };
        let (state, effects) = applied(run(state, &write, &program));
        assert_eq!(
            effects[0],
            Effect::Wrote {
                path: Path::Object {
                    object: ObjectId(1),
                    segments: vec![Segment::Field("x".to_string())]
                }
            }
        );
        let object = state.object(ObjectId(1)).expect("object");
        match &object.shape {
            ObjectShape::Structure { fields, .. } => assert_eq!(fields[0].value, int(7)),
            other => panic!("expected a structure, got {:?}", other),
        }
    }

    #[test]
    fn a_type_mismatch_is_pinned_on_the_variable() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let declare = TraceEvent::DeclareVariable {
            sequence: Sequence(2),
            line: 12,
            name: "total".to_string(),
            value: text("not a number"),
        };
        let (state, incident) = rejected(run(state, &declare, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::TypeMismatch { .. }
        ));
        assert_eq!(
            incident.location(),
            &Location::Cell(Path::Stack {
                frame: FrameId(1),
                variable: "total".to_string()
            })
        );
        assert!(state.frames[0].variables.is_empty());
    }

    #[test]
    fn writing_to_a_released_object_is_pinned_on_the_field() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let (state, _) = applied(run(state, &create_point(2), &program));
        let release = TraceEvent::ReleaseObject {
            sequence: Sequence(3),
            object: ObjectId(1),
        };
        let (state, _) = applied(run(state, &release, &program));
        let write = TraceEvent::WriteMemory {
            sequence: Sequence(4),
            line: 11,
            path: Path::Object {
                object: ObjectId(1),
                segments: vec![Segment::Field("x".to_string())],
            },
            value: int(9),
        };
        let (after, incident) = rejected(run(state, &write, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::ReleasedObject { .. }
        ));
        assert_eq!(
            incident.location(),
            &Location::Cell(Path::Object {
                object: ObjectId(1),
                segments: vec![Segment::Field("x".to_string())]
            })
        );
        assert_eq!(after.heap[0].state, ObjectState::Released);
    }

    #[test]
    fn a_duplicate_object_is_rejected() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let (state, _) = applied(run(state, &create_point(2), &program));
        let (after, incident) = rejected(run(state, &create_point(3), &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::DuplicateObject { .. }
        ));
        assert_eq!(after.heap.len(), 1);
    }

    #[test]
    fn a_duplicate_variable_is_rejected() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let declare = TraceEvent::DeclareVariable {
            sequence: Sequence(2),
            line: 12,
            name: "total".to_string(),
            value: int(0),
        };
        let (state, _) = applied(run(state, &declare, &program));
        let again = TraceEvent::DeclareVariable {
            sequence: Sequence(3),
            line: 12,
            name: "total".to_string(),
            value: int(1),
        };
        let (_, incident) = rejected(run(state, &again, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::DuplicateVariable { .. }
        ));
    }

    #[test]
    fn ending_without_a_frame_is_rejected() {
        let program = program();
        let end = TraceEvent::EndFunction {
            sequence: Sequence(1),
            return_value: None,
        };
        let (_, incident) = rejected(run(MemoryState::empty(), &end, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::EmptyStack
        ));
    }

    #[test]
    fn a_duplicate_sequence_does_not_touch_memory() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let declare = TraceEvent::DeclareVariable {
            sequence: Sequence(2),
            line: 12,
            name: "total".to_string(),
            value: int(1),
        };
        let (state, _) = applied(run(state, &declare, &program));
        let stale = TraceEvent::DeclareVariable {
            sequence: Sequence(2),
            line: 12,
            name: "other".to_string(),
            value: int(2),
        };
        let (state, incident) = rejected(run(state, &stale, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::OutOfOrderSequence { .. }
        ));
        assert_eq!(state.frames[0].variables.len(), 1);
        assert_eq!(state.last_sequence, Some(Sequence(2)));
    }

    #[test]
    fn the_depth_limit_is_reported_not_hit() {
        let program = program();
        let mut state = MemoryState::empty();
        let mut sequence = 1;
        while state.frames.len() < MAX_FRAME_DEPTH {
            let event = TraceEvent::StartFunction {
                sequence: Sequence(sequence),
                function: program.entry,
                call_line: 10,
                arguments: Vec::new(),
            };
            let outcome = applied(run(state, &event, &program));
            state = outcome.0;
            sequence += 1;
        }
        let extra = TraceEvent::StartFunction {
            sequence: Sequence(sequence),
            function: program.entry,
            call_line: 10,
            arguments: Vec::new(),
        };
        let (state, incident) = rejected(run(state, &extra, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::DepthExceeded { .. }
        ));
        assert_eq!(state.frames.len(), MAX_FRAME_DEPTH);
    }

    #[test]
    fn an_unknown_function_is_pinned_on_the_trace() {
        let program = program();
        let event = TraceEvent::StartFunction {
            sequence: Sequence(1),
            function: FunctionId(99),
            call_line: 1,
            arguments: Vec::new(),
        };
        let (_, incident) = rejected(run(MemoryState::empty(), &event, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::UnknownFunction { .. }
        ));
        assert_eq!(incident.location(), &Location::Trace);
    }

    #[test]
    fn a_struct_object_must_carry_its_declared_fields() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let wrong = TraceEvent::CreateObject {
            sequence: Sequence(2),
            line: 11,
            object: ObjectId(1),
            label: "Point#1".to_string(),
            shape: ObjectShape::Structure {
                structure: StructId(0),
                fields: vec![field("z", int(0))],
            },
        };
        let (_, incident) = rejected(run(state, &wrong, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::UnknownField { .. } | RejectionReason::MissingField { .. }
        ));
    }

    #[test]
    fn sequence_elements_must_match_the_declared_type() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let wrong = TraceEvent::CreateObject {
            sequence: Sequence(2),
            line: 11,
            object: ObjectId(2),
            label: "scores".to_string(),
            shape: ObjectShape::Sequence {
                element_type: DataType::Integer,
                elements: vec![text("ninety")],
            },
        };
        let (_, incident) = rejected(run(state, &wrong, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::TypeMismatch { .. }
        ));
    }

    #[test]
    fn a_write_to_a_frame_that_is_not_running_is_rejected() {
        let program = program();
        let (state, _) = applied(run(MemoryState::empty(), &start_main(&program), &program));
        let write = TraceEvent::WriteMemory {
            sequence: Sequence(2),
            line: 12,
            path: Path::Stack {
                frame: FrameId(99),
                variable: "total".to_string(),
            },
            value: int(1),
        };
        let (_, incident) = rejected(run(state, &write, &program));
        assert!(matches!(
            reject_reason(&incident),
            RejectionReason::FrameNotCurrent { .. }
        ));
        assert_eq!(incident.location(), &Location::Frame(FrameId(99)));
    }
}
