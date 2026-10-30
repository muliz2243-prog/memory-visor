use crate::model::events::TraceEvent;
use crate::model::memory::Location;
use crate::model::outcome::{Effect, Incident};
use crate::model::program::ProgramTree;
use crate::shell::format::{format_path, function_name};

pub fn describe_event(event: &TraceEvent, program: &ProgramTree) -> String {
    match event {
        TraceEvent::StartFunction { function, .. } => {
            format!("start {}", function_name(program, *function))
        }
        TraceEvent::EndFunction { .. } => "end function".to_string(),
        TraceEvent::DeclareVariable { name, .. } => format!("declare {}", name),
        TraceEvent::WriteMemory { path, .. } => format!("write {}", format_path(path)),
        TraceEvent::CreateObject { label, .. } => format!("create {}", label),
        TraceEvent::ReleaseObject { object, .. } => format!("release object {}", object.0),
        TraceEvent::ConditionalJump { to_line, .. } => format!("jump to line {}", to_line),
        TraceEvent::RuntimeError { class, .. } => format!("runtime error {:?}", class),
        TraceEvent::EnterScope { kind, .. } => format!("enter {:?} scope", kind),
        TraceEvent::ExitScope { .. } => "exit scope".to_string(),
        TraceEvent::Borrow { kind, .. } => format!("borrow {:?}", kind),
        TraceEvent::ReleaseBorrow { .. } => "release borrow".to_string(),
        TraceEvent::Drop { path, .. } => format!("drop {}", format_path(path)),
        TraceEvent::Move { from, to, .. } => {
            format!("move {} to {}", format_path(from), format_path(to))
        }
    }
}

pub fn describe_effect(effect: &Effect) -> String {
    match effect {
        Effect::Wrote { path } => format!("wrote {}", format_path(path)),
        Effect::CreatedObject { object } => format!("created object {}", object.0),
        Effect::ReleasedObject { object } => format!("released object {}", object.0),
        Effect::FramePushed { frame } => format!("frame {} pushed", frame.0),
        Effect::FramePopped { frame } => format!("frame {} popped", frame.0),
        Effect::LineChanged { line } => format!("line {}", line),
        Effect::IncidentRecorded { location } => {
            format!("incident at {}", describe_location(location))
        }
        Effect::Dropped { path } => format!("dropped {}", format_path(path)),
        Effect::Borrowed { borrower, target } => format!(
            "borrowed {} from {}",
            format_path(borrower),
            format_path(target)
        ),
        Effect::ReleasedBorrow { borrower } => {
            format!("returned the loan on {}", format_path(borrower))
        }
        Effect::Moved { from, to } => {
            format!("moved {} to {}", format_path(from), format_path(to))
        }
    }
}

pub fn describe_location(location: &Location) -> String {
    match location {
        Location::Trace => "the trace".to_string(),
        Location::Stack => "the stack".to_string(),
        Location::Frame(frame) => format!("frame {}", frame.0),
        Location::Cell(path) => format_path(path),
        Location::Line(line) => format!("line {}", line),
        Location::Heap => "the heap".to_string(),
    }
}

/// Human summary of one step: the effects it produced, or why it was rejected.
pub fn describe_step(effects: &[Effect], incident: Option<&Incident>) -> String {
    match incident {
        Some(incident) => format!(
            "REJECTED, pinned on {}: {}",
            describe_location(incident.location()),
            incident.message()
        ),
        None => {
            let labels: Vec<String> = effects.iter().map(describe_effect).collect();
            match labels.is_empty() {
                true => "ok".to_string(),
                false => labels.join(", "),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ids::{FrameId, ObjectId, Sequence};
    use crate::model::memory::Path;
    use crate::model::outcome::RejectionReason;
    use crate::stages::analyze::{Analysis, analyze_program};

    const SOURCE: &str = "\
fn main() {
    let total: i32 = 1;
}
";

    fn program() -> ProgramTree {
        match analyze_program(SOURCE) {
            Analysis::Analyzed(tree) => tree,
            Analysis::Failed(error) => panic!("unexpected error: {:?}", error),
        }
    }

    fn cell() -> Path {
        Path::Stack {
            frame: FrameId(1),
            variable: "total".to_string(),
        }
    }

    #[test]
    fn a_write_effect_names_the_address() {
        let effect = Effect::Wrote { path: cell() };
        assert_eq!(describe_effect(&effect), "wrote frame 1.total");
    }

    #[test]
    fn an_empty_step_reads_as_ok() {
        assert_eq!(describe_step(&[], None), "ok");
    }

    #[test]
    fn a_rejected_step_keeps_the_reason_and_the_place() {
        let incident = Incident::InvalidEvent {
            sequence: Sequence(3),
            location: Location::Cell(cell()),
            reason: RejectionReason::ReleasedObject {
                object: ObjectId(1),
            },
        };
        let text = describe_step(&[], Some(&incident));
        assert!(text.starts_with("REJECTED, pinned on frame 1.total"));
        assert!(text.contains("already released"));
    }

    #[test]
    fn a_start_event_names_the_function() {
        let program = program();
        let event = TraceEvent::StartFunction {
            sequence: Sequence(1),
            function: program.entry,
            call_line: 1,
            arguments: Vec::new(),
        };
        assert_eq!(describe_event(&event, &program), "start main");
    }

    #[test]
    fn a_location_can_be_a_frame_or_the_trace() {
        assert_eq!(describe_location(&Location::Frame(FrameId(4))), "frame 4");
        assert_eq!(describe_location(&Location::Trace), "the trace");
    }
}
