use crate::model::events::{BorrowKind, ConditionOutcome, TraceEvent};
use crate::model::ids::{FrameId, FunctionId, ObjectId, ScopeId, Sequence, StructId};
use crate::model::memory::{Field, ObjectShape, Path, PrimitiveValue, Segment, Value};
use crate::model::outcome::ErrorClass;
use crate::model::program::{DataType, ProgramTree};
use crate::model::scope::ScopeKind;

/// Struct/array program the window opens with: two heap objects and one
/// struct field write, enough to exercise the layout and the trace panel.
pub const SAMPLE_SOURCE: &str = "\
struct Point {
    x: i32,
    y: i32,
}

fn build_point(x: i32, y: i32) -> Point {
    let result: Point = Point { x, y };
    return result;
}

fn sum_scores(scores: [i32; 3]) -> i32 {
    let total: i32 = 0;
    total = scores[0] + scores[1] + scores[2];
    return total;
}

fn main() {
    let origin: Point = build_point(0, 0);
    let scores: [i32; 3] = [90, 75, 88];
    let total: i32 = sum_scores(scores);
    origin.x = 5;
}
";

const POINT: ObjectId = ObjectId(1);
const SCORES: ObjectId = ObjectId(2);

/// Program for the lifecycle scene: a function scope, one nested block and a
/// value moved out of the block's lifetime.
pub const LIFECYCLE_SOURCE: &str = "\
fn main() {
    let owner: i32 = 7;
    let first: i32 = 1;
    {
        let inner: i32 = 3;
        let second: i32 = 2;
        // borrows and drops are recorded, not written here
    }
    let result: i32 = owner + 1;
}
";

/// Hand-written lifecycle trace. `ExitScope` is what drops: the engine walks the
/// scope backwards, so the trace does not need an explicit `Drop` per variable.
pub fn lifecycle_trace(program: &ProgramTree) -> Vec<TraceEvent> {
    let main = function_id(program, "main");
    let function_scope = ScopeId(1);
    let block_scope = ScopeId(2);
    let frame = FrameId(1);
    vec![
        start(1, main, 1),
        TraceEvent::EnterScope {
            sequence: Sequence(2),
            line: 1,
            scope: function_scope,
            kind: ScopeKind::Function,
            parent: None,
        },
        declare(3, 2, "owner", integer(7)),
        declare(4, 3, "first", integer(1)),
        borrow(5, 4, "first", "owner", BorrowKind::Immutable, frame),
        TraceEvent::EnterScope {
            sequence: Sequence(6),
            line: 4,
            scope: block_scope,
            kind: ScopeKind::Block,
            parent: Some(function_scope),
        },
        declare(7, 5, "inner", integer(3)),
        declare(8, 6, "second", integer(2)),
        borrow(9, 7, "second", "owner", BorrowKind::Immutable, frame),
        release(10, 7, "first", frame),
        release(11, 7, "second", frame),
        // Both immutable loans are gone, so the owner can be lent mutably.
        borrow(12, 7, "first", "owner", BorrowKind::Mutable, frame),
        release(13, 7, "first", frame),
        // Closing the block drops `second` and then `inner`.
        TraceEvent::ExitScope {
            sequence: Sequence(14),
            scope: block_scope,
        },
        declare(15, 9, "result", integer(8)),
        // Closing the function drops `result`, `first` and `owner`.
        TraceEvent::ExitScope {
            sequence: Sequence(16),
            scope: function_scope,
        },
        TraceEvent::EndFunction {
            sequence: Sequence(17),
            return_value: None,
        },
    ]
}

fn borrow(
    sequence: u64,
    line: u32,
    borrower: &str,
    target: &str,
    kind: BorrowKind,
    frame: FrameId,
) -> TraceEvent {
    TraceEvent::Borrow {
        sequence: Sequence(sequence),
        line,
        borrower: Path::Stack {
            frame,
            variable: borrower.to_string(),
        },
        target: Path::Stack {
            frame,
            variable: target.to_string(),
        },
        kind,
    }
}

fn release(sequence: u64, line: u32, borrower: &str, frame: FrameId) -> TraceEvent {
    TraceEvent::ReleaseBorrow {
        sequence: Sequence(sequence),
        line,
        borrower: Path::Stack {
            frame,
            variable: borrower.to_string(),
        },
    }
}

pub fn sample_trace(program: &ProgramTree) -> Vec<TraceEvent> {
    let main = function_id(program, "main");
    let build_point = function_id(program, "build_point");
    let sum_scores = function_id(program, "sum_scores");
    let point = struct_id(program, "Point");
    vec![
        start(1, main, 17),
        start(2, build_point, 18),
        TraceEvent::CreateObject {
            sequence: Sequence(3),
            line: 7,
            object: POINT,
            label: "Point#1".to_string(),
            shape: ObjectShape::Structure {
                structure: point,
                fields: vec![field("x", integer(0)), field("y", integer(0))],
            },
        },
        declare(4, 7, "result", reference(POINT)),
        TraceEvent::EndFunction {
            sequence: Sequence(5),
            return_value: Some(reference(POINT)),
        },
        declare(6, 18, "origin", reference(POINT)),
        TraceEvent::CreateObject {
            sequence: Sequence(7),
            line: 19,
            object: SCORES,
            label: "scores[3]".to_string(),
            shape: ObjectShape::Sequence {
                element_type: DataType::Integer,
                elements: vec![integer(90), integer(75), integer(88)],
            },
        },
        declare(8, 19, "scores", reference(SCORES)),
        TraceEvent::StartFunction {
            sequence: Sequence(9),
            function: sum_scores,
            call_line: 20,
            arguments: vec![field("scores", reference(SCORES))],
        },
        declare(10, 12, "total", integer(0)),
        TraceEvent::WriteMemory {
            sequence: Sequence(11),
            line: 13,
            path: element(SCORES, 0),
            value: integer(90),
        },
        TraceEvent::ConditionalJump {
            sequence: Sequence(12),
            from_line: 13,
            to_line: 14,
            expression: "total > 0".to_string(),
            outcome: ConditionOutcome::True,
        },
        TraceEvent::EndFunction {
            sequence: Sequence(13),
            return_value: Some(integer(253)),
        },
        declare(14, 20, "total", integer(253)),
        TraceEvent::WriteMemory {
            sequence: Sequence(15),
            line: 21,
            path: object_field(POINT, "x"),
            value: integer(5),
        },
        TraceEvent::ReleaseObject {
            sequence: Sequence(16),
            object: POINT,
        },
        // Writing to a released object: the engine rejects it and pins the
        // incident on that exact field.
        TraceEvent::WriteMemory {
            sequence: Sequence(17),
            line: 21,
            path: object_field(POINT, "x"),
            value: integer(9),
        },
        // The observed program then fails on the array: the incident is pinned
        // on the object as a whole.
        TraceEvent::RuntimeError {
            sequence: Sequence(18),
            line: 21,
            path: Some(Path::Object {
                object: SCORES,
                segments: Vec::new(),
            }),
            class: ErrorClass::IndexOutOfRange,
            message: "index 5 out of bounds for length 3".to_string(),
        },
    ]
}

fn function_id(program: &ProgramTree, name: &str) -> FunctionId {
    program
        .function_named(name)
        .unwrap_or_else(|| panic!("sample source must define '{}'", name))
}

fn struct_id(program: &ProgramTree, name: &str) -> StructId {
    program
        .structs
        .iter()
        .find(|structure| structure.name == name)
        .map(|structure| structure.id)
        .unwrap_or_else(|| panic!("sample source must define struct '{}'", name))
}

fn start(sequence: u64, function: FunctionId, call_line: u32) -> TraceEvent {
    TraceEvent::StartFunction {
        sequence: Sequence(sequence),
        function,
        call_line,
        arguments: Vec::new(),
    }
}

fn declare(sequence: u64, line: u32, name: &str, value: Value) -> TraceEvent {
    TraceEvent::DeclareVariable {
        sequence: Sequence(sequence),
        line,
        name: name.to_string(),
        value,
    }
}

fn field(name: &str, value: Value) -> Field {
    Field {
        name: name.to_string(),
        value,
    }
}

fn object_field(object: ObjectId, name: &str) -> Path {
    Path::Object {
        object,
        segments: vec![Segment::Field(name.to_string())],
    }
}

fn element(object: ObjectId, index: usize) -> Path {
    Path::Object {
        object,
        segments: vec![Segment::Index(index)],
    }
}

fn integer(value: i64) -> Value {
    Value::Primitive(PrimitiveValue::Integer(value))
}

fn reference(object: ObjectId) -> Value {
    Value::Reference(object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages::analyze::{Analysis, analyze_program};
    use crate::stages::replay::build_timeline;

    fn program() -> ProgramTree {
        match analyze_program(SAMPLE_SOURCE) {
            Analysis::Analyzed(tree) => tree,
            Analysis::Failed(error) => panic!("sample source does not parse: {:?}", error),
        }
    }

    #[test]
    fn the_sample_matches_its_own_line_numbers() {
        let program = program();
        let main = program.function(program.entry).expect("main");
        let build_point = program.function_named("build_point").expect("build_point");
        assert_eq!(main.start_line, 17);
        assert_eq!(program.function(build_point).expect("def").start_line, 6);
    }

    #[test]
    fn the_trace_is_ordered_and_complete() {
        let program = program();
        let events = sample_trace(&program);
        assert_eq!(events.len(), 18);
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.sequence(), Sequence(index as u64 + 1));
        }
    }

    fn lifecycle_program() -> ProgramTree {
        match analyze_program(LIFECYCLE_SOURCE) {
            Analysis::Analyzed(tree) => tree,
            Analysis::Failed(error) => panic!("lifecycle source does not parse: {:?}", error),
        }
    }

    #[test]
    fn the_lifecycle_trace_replays_without_rejections() {
        let program = lifecycle_program();
        let events = lifecycle_trace(&program);
        assert_eq!(events.len(), 17);
        let timeline = build_timeline(&program, &events);
        assert_eq!(timeline.rejection_count(), 0);
        assert_eq!(timeline.len(), events.len());
    }

    #[test]
    fn closing_the_block_drops_its_variables_last_declared_first() {
        use crate::model::outcome::Effect;

        let program = lifecycle_program();
        let events = lifecycle_trace(&program);
        let timeline = build_timeline(&program, &events);

        let inside = timeline.state_at(14).expect("step 14 exists");
        let names: Vec<&str> = inside.frames[0]
            .variables
            .iter()
            .map(|variable| variable.name.as_str())
            .collect();
        assert_eq!(names, vec!["owner", "first"]);

        let dropped: Vec<&str> = timeline.steps[13]
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Dropped {
                    path: Path::Stack { variable, .. },
                } => Some(variable.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(dropped, vec!["second", "inner"]);
    }

    #[test]
    fn the_mutable_loan_waits_until_the_immutable_ones_are_released() {
        let program = lifecycle_program();
        let events = lifecycle_trace(&program);
        let timeline = build_timeline(&program, &events);

        let borrowed = timeline.state_at(9).expect("step 9 exists");
        assert_eq!(borrowed.loans.len(), 2);
        assert!(
            borrowed
                .loans
                .iter()
                .all(|loan| loan.kind == BorrowKind::Immutable)
        );

        let mutable = timeline.state_at(12).expect("step 12 exists");
        assert_eq!(mutable.loans.len(), 1);
        assert_eq!(mutable.loans[0].kind, BorrowKind::Mutable);
    }
}
