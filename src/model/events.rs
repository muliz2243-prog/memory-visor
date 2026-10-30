use crate::model::ids::{FunctionId, ObjectId, ScopeId, Sequence};
use crate::model::memory::{Field, ObjectShape, Path, Value};
use crate::model::outcome::ErrorClass;
use crate::model::scope::ScopeKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConditionOutcome {
    True,
    False,
}

/// Which kind of loan a reference takes on the data it points at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BorrowKind {
    Immutable,
    Mutable,
}

/// Why a value stopped being alive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropReason {
    /// The scope that owned it closed.
    EndOfScope,
    /// It was overwritten or moved into something else.
    Reassigned,
}

#[derive(Clone, PartialEq, Debug)]
pub enum TraceEvent {
    /// Pushes a frame. `call_line` is the line of the caller, so the caller's
    /// cursor never needs a separate event.
    StartFunction {
        sequence: Sequence,
        function: FunctionId,
        call_line: u32,
        arguments: Vec<Field>,
    },
    /// Pops the current frame. The value is reported for display only.
    EndFunction {
        sequence: Sequence,
        return_value: Option<Value>,
    },
    DeclareVariable {
        sequence: Sequence,
        line: u32,
        name: String,
        value: Value,
    },
    WriteMemory {
        sequence: Sequence,
        line: u32,
        path: Path,
        value: Value,
    },
    CreateObject {
        sequence: Sequence,
        line: u32,
        object: ObjectId,
        label: String,
        shape: ObjectShape,
    },
    ReleaseObject {
        sequence: Sequence,
        object: ObjectId,
    },
    ConditionalJump {
        sequence: Sequence,
        from_line: u32,
        to_line: u32,
        expression: String,
        outcome: ConditionOutcome,
    },
    /// Runtime failure of the observed program. Applied, then pinned.
    RuntimeError {
        sequence: Sequence,
        line: u32,
        path: Option<Path>,
        class: ErrorClass,
        message: String,
    },
    /// A lexical scope starts. Everything declared after this belongs to it and
    /// is dropped when the matching `ExitScope` arrives.
    EnterScope {
        sequence: Sequence,
        line: u32,
        scope: ScopeId,
        kind: ScopeKind,
        parent: Option<ScopeId>,
    },
    /// A scope ends. The engine drops its variables last-declared first.
    ExitScope { sequence: Sequence, scope: ScopeId },
    /// A `&` or `&mut` loan. The target must stay alive while the loan exists.
    Borrow {
        sequence: Sequence,
        line: u32,
        borrower: Path,
        target: Path,
        kind: BorrowKind,
    },
    /// The loan ends, so the target can be lent mutably again.
    ReleaseBorrow {
        sequence: Sequence,
        line: u32,
        borrower: Path,
    },
    /// A value dies. `path` is the cell that no longer holds anything.
    Drop {
        sequence: Sequence,
        line: u32,
        path: Path,
        reason: DropReason,
    },
    /// Ownership moves from one cell to another. The source becomes invalid.
    Move {
        sequence: Sequence,
        line: u32,
        from: Path,
        to: Path,
    },
}

impl TraceEvent {
    pub fn sequence(&self) -> Sequence {
        match self {
            TraceEvent::StartFunction { sequence, .. } => *sequence,
            TraceEvent::EndFunction { sequence, .. } => *sequence,
            TraceEvent::DeclareVariable { sequence, .. } => *sequence,
            TraceEvent::WriteMemory { sequence, .. } => *sequence,
            TraceEvent::CreateObject { sequence, .. } => *sequence,
            TraceEvent::ReleaseObject { sequence, .. } => *sequence,
            TraceEvent::ConditionalJump { sequence, .. } => *sequence,
            TraceEvent::RuntimeError { sequence, .. } => *sequence,
            TraceEvent::EnterScope { sequence, .. } => *sequence,
            TraceEvent::ExitScope { sequence, .. } => *sequence,
            TraceEvent::Borrow { sequence, .. } => *sequence,
            TraceEvent::ReleaseBorrow { sequence, .. } => *sequence,
            TraceEvent::Drop { sequence, .. } => *sequence,
            TraceEvent::Move { sequence, .. } => *sequence,
        }
    }

    /// Short stable label for error messages. Kept in the event so a rejection
    /// never has to invent a name from the fields it happened to receive.
    pub fn name(&self) -> &'static str {
        match self {
            TraceEvent::StartFunction { .. } => "start function",
            TraceEvent::EndFunction { .. } => "end function",
            TraceEvent::DeclareVariable { .. } => "declare variable",
            TraceEvent::WriteMemory { .. } => "write memory",
            TraceEvent::CreateObject { .. } => "create object",
            TraceEvent::ReleaseObject { .. } => "release object",
            TraceEvent::ConditionalJump { .. } => "conditional jump",
            TraceEvent::RuntimeError { .. } => "runtime error",
            TraceEvent::EnterScope { .. } => "enter scope",
            TraceEvent::ExitScope { .. } => "exit scope",
            TraceEvent::Borrow { .. } => "borrow",
            TraceEvent::ReleaseBorrow { .. } => "release borrow",
            TraceEvent::Drop { .. } => "drop",
            TraceEvent::Move { .. } => "move",
        }
    }

    /// Line the program is on once this event has been applied.
    ///
    /// `StartFunction` reports the caller's line and `EndFunction` has none, so
    /// both return `None` and the caller falls back to `MemoryState::current_line`.
    /// A jump lands on `to_line`, which is the instruction about to run.
    pub fn active_line(&self) -> Option<u32> {
        match self {
            TraceEvent::StartFunction { .. }
            | TraceEvent::EndFunction { .. }
            | TraceEvent::ReleaseObject { .. }
            | TraceEvent::ExitScope { .. } => None,
            TraceEvent::DeclareVariable { line, .. }
            | TraceEvent::WriteMemory { line, .. }
            | TraceEvent::CreateObject { line, .. }
            | TraceEvent::RuntimeError { line, .. }
            | TraceEvent::EnterScope { line, .. }
            | TraceEvent::Borrow { line, .. }
            | TraceEvent::ReleaseBorrow { line, .. }
            | TraceEvent::Drop { line, .. }
            | TraceEvent::Move { line, .. } => Some(*line),
            TraceEvent::ConditionalJump { to_line, .. } => Some(*to_line),
        }
    }
}
