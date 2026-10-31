//! Read-only view of one replayed step.
//!
//! The [`Timeline`] already stores a `MemoryState` per event; this module only
//! answers the three questions the canvas asks: which box is running, which
//! source line is active, and where the flow went. Nothing here mutates the
//! trace, so there is one source of truth (the events) and this is a view.

use crate::model::events::{ConditionOutcome, TraceEvent};
use crate::model::ids::{FrameId, ScopeId, Sequence};
use crate::model::memory::MemoryState;
use crate::stages::replay::Timeline;

/// The illuminated box: the running frame, or a block nested inside it.
///
/// A scope always belongs to a frame, so [`ExecutionStep::active_frame`]
/// resolves either variant to the frame the UI has to highlight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExecutionNode {
    Frame(FrameId),
    Scope(ScopeId),
}

/// The branch a `ConditionalJump` took. Only jumps produce one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FlowEdge {
    pub from_line: u32,
    pub to_line: u32,
    pub outcome: ConditionOutcome,
}

/// One step of the replay: line, active node, memory snapshot and flow.
#[derive(Clone, PartialEq, Debug)]
pub struct ExecutionStep<'a> {
    /// Number of events applied. `0` is the empty memory before the trace.
    pub index: usize,
    /// Sequence of the event that produced this step, if there is one.
    pub sequence: Option<Sequence>,
    /// Source line the program is on, if known.
    pub line: Option<u32>,
    /// Frame or scope the canvas lights up.
    pub active_node: Option<ExecutionNode>,
    /// Memory after the step. Borrowed from the timeline, never copied.
    pub memory: &'a MemoryState,
    /// Where a conditional jump sent the flow.
    pub flow: Option<FlowEdge>,
}

impl ExecutionStep<'_> {
    /// Frame that owns the active node, which is what the canvas draws.
    pub fn active_frame(&self) -> Option<FrameId> {
        match self.active_node {
            Some(ExecutionNode::Frame(frame)) => Some(frame),
            Some(ExecutionNode::Scope(scope)) => {
                self.memory.scopes.scope(scope).map(|scope| scope.frame)
            }
            None => None,
        }
    }

    /// Innermost open scope, when the active node is the function frame itself.
    pub fn active_scope(&self) -> Option<ScopeId> {
        match self.active_node {
            Some(ExecutionNode::Scope(scope)) => Some(scope),
            _ => None,
        }
    }
}

/// The step before the first event: empty memory, nothing running.
pub fn initial_step(timeline: &Timeline) -> ExecutionStep<'_> {
    ExecutionStep {
        index: 0,
        sequence: None,
        line: None,
        active_node: None,
        memory: timeline
            .state_at(0)
            .expect("the timeline always has step 0"),
        flow: None,
    }
}

/// Step reached after `index` events. `None` when the index is past the trace.
pub fn execution_step<'a>(
    events: &[TraceEvent],
    timeline: &'a Timeline,
    index: usize,
) -> Option<ExecutionStep<'a>> {
    let memory = timeline.state_at(index)?;
    let event = match index {
        0 => None,
        _ => events.get(index - 1),
    };
    Some(ExecutionStep {
        index,
        sequence: event.map(TraceEvent::sequence),
        line: event.and_then(|event| line_of(event, memory)),
        active_node: active_node(memory),
        memory,
        flow: event.and_then(flow_edge),
    })
}

/// Line of the event, falling back to the frame's own cursor for events that do
/// not carry one (`StartFunction`, `EndFunction`, `ExitScope`).
fn line_of(event: &TraceEvent, memory: &MemoryState) -> Option<u32> {
    event.active_line().or_else(|| memory.current_line())
}

/// The innermost open scope of the running frame wins over the frame itself.
fn active_node(memory: &MemoryState) -> Option<ExecutionNode> {
    let frame = memory.frames.last()?;
    match memory.scopes.current() {
        Some(scope) if scope.frame == frame.id => Some(ExecutionNode::Scope(scope.id)),
        _ => Some(ExecutionNode::Frame(frame.id)),
    }
}

fn flow_edge(event: &TraceEvent) -> Option<FlowEdge> {
    match event {
        TraceEvent::ConditionalJump {
            from_line,
            to_line,
            outcome,
            ..
        } => Some(FlowEdge {
            from_line: *from_line,
            to_line: *to_line,
            outcome: *outcome,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::recorder::record;
    use crate::stages::replay::build_timeline;

    const BRANCHING: &str = "fn main() {\n let limit: i32 = 4;\n if limit > 2 {\n let big: i32 = 1;\n } else {\n let small: i32 = 2;\n }\n}";

    fn trace(source: &str) -> (Vec<TraceEvent>, Timeline) {
        let recording = record(source).expect("the source records");
        let timeline = build_timeline(&recording.program, &recording.events);
        (recording.events, timeline)
    }

    fn jump_step(events: &[TraceEvent]) -> usize {
        events
            .iter()
            .position(|event| matches!(event, TraceEvent::ConditionalJump { .. }))
            .expect("the trace has a jump")
            + 1
    }

    #[test]
    fn the_initial_step_has_no_node_and_empty_memory() {
        let (_, timeline) = trace("fn main() {\n let a: i32 = 1;\n}");
        let step = initial_step(&timeline);
        assert_eq!(step.index, 0);
        assert_eq!(step.sequence, None);
        assert_eq!(step.line, None);
        assert_eq!(step.active_node, None);
        assert!(step.flow.is_none());
        assert!(step.memory.frames.is_empty());
    }

    #[test]
    fn a_running_function_lights_its_frame() {
        let (events, timeline) = trace("fn main() {\n let a: i32 = 1;\n}");
        let step = execution_step(&events, &timeline, 1).expect("step 1 exists");
        assert_eq!(step.active_node, Some(ExecutionNode::Frame(FrameId(1))));
        assert_eq!(step.active_frame(), Some(FrameId(1)));
        assert_eq!(step.line, Some(1));
    }

    #[test]
    fn a_declaration_reports_its_line_and_value() {
        let (events, timeline) = trace("fn main() {\n let a: i32 = 1;\n}");
        let step = execution_step(&events, &timeline, 3).expect("step 3 exists");
        assert_eq!(step.line, Some(2));
        assert_eq!(step.sequence, Some(Sequence(3)));
        let frame = step.memory.frames.last().expect("main is running");
        assert!(frame.variables.iter().any(|variable| variable.name == "a"));
    }

    #[test]
    fn a_conditional_jump_reports_the_edge_it_took() {
        let (events, timeline) = trace(BRANCHING);
        let step = execution_step(&events, &timeline, jump_step(&events)).expect("the jump");
        let flow = step.flow.expect("the jump has an edge");
        assert_eq!(flow.outcome, ConditionOutcome::True);
        assert_eq!(flow.from_line, 3);
        assert_eq!(flow.to_line, 4);
        assert_eq!(step.line, Some(4));
    }

    #[test]
    fn entering_a_branch_lights_the_block_scope_inside_the_frame() {
        let (events, timeline) = trace(BRANCHING);
        let step =
            execution_step(&events, &timeline, jump_step(&events) + 1).expect("the scope opens");
        assert_eq!(step.active_node, Some(ExecutionNode::Scope(ScopeId(2))));
        assert_eq!(step.active_scope(), Some(ScopeId(2)));
        assert_eq!(step.active_frame(), Some(FrameId(1)));
    }

    #[test]
    fn the_false_branch_reports_the_other_edge() {
        let source = "fn main() {\n let limit: i32 = 1;\n if limit > 2 {\n let big: i32 = 1;\n } else {\n let small: i32 = 2;\n }\n}";
        let (events, timeline) = trace(source);
        let step = execution_step(&events, &timeline, jump_step(&events)).expect("the jump");
        let flow = step.flow.expect("the jump has an edge");
        assert_eq!(flow.outcome, ConditionOutcome::False);
        assert_eq!(flow.from_line, 3);
        assert_eq!(flow.to_line, 6);
        assert_eq!(step.line, Some(6));
    }

    #[test]
    fn an_index_past_the_trace_has_no_step() {
        let (events, timeline) = trace("fn main() {\n let a: i32 = 1;\n}");
        assert!(execution_step(&events, &timeline, events.len()).is_some());
        assert!(execution_step(&events, &timeline, events.len() + 1).is_none());
    }
}
