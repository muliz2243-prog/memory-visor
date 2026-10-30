use crate::model::ids::{FrameId, FunctionId, ObjectId, Sequence};
use crate::model::memory::{Location, MemoryState, Path};
use crate::model::program::{DataType, ProgramTree, ValueKind};

/// Error raised by the observed program itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorClass {
    IndexOutOfRange,
    DivisionByZero,
    Overflow,
    NullReference,
}

/// Something that went wrong, pinned to a place in the drawing.
#[derive(Clone, PartialEq, Debug)]
pub enum Incident {
    /// The trace asked for something the engine cannot do.
    InvalidEvent {
        sequence: Sequence,
        location: Location,
        reason: RejectionReason,
    },
    /// The observed program failed at runtime. The event was applied.
    RuntimeError {
        sequence: Sequence,
        location: Location,
        class: ErrorClass,
        message: String,
    },
}

impl Incident {
    pub fn location(&self) -> &Location {
        match self {
            Incident::InvalidEvent { location, .. } => location,
            Incident::RuntimeError { location, .. } => location,
        }
    }

    pub fn sequence(&self) -> Sequence {
        match self {
            Incident::InvalidEvent { sequence, .. } => *sequence,
            Incident::RuntimeError { sequence, .. } => *sequence,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Incident::InvalidEvent { reason, .. } => {
                format!("invalid event: {}", reason.message())
            }
            Incident::RuntimeError { class, message, .. } => {
                format!("runtime error {:?}: {}", class, message)
            }
        }
    }
}

/// Why an event was not applied. Every variant has a real trace defect behind it.
#[derive(Clone, PartialEq, Debug)]
pub enum RejectionReason {
    OutOfOrderSequence {
        last: Option<Sequence>,
        received: Sequence,
    },
    UnknownFunction {
        function: FunctionId,
    },
    EmptyStack,
    DepthExceeded {
        maximum: usize,
    },
    DuplicateVariable {
        name: String,
    },
    UnknownVariable {
        name: String,
    },
    FrameNotCurrent {
        frame: FrameId,
    },
    UnknownObject {
        object: ObjectId,
    },
    DuplicateObject {
        object: ObjectId,
    },
    ReleasedObject {
        object: ObjectId,
    },
    InvalidPath {
        path: Path,
    },
    TypeMismatch {
        expected: DataType,
        received: ValueKind,
    },
    UnknownStructure {
        structure: crate::model::ids::StructId,
    },
    UnknownField {
        structure: crate::model::ids::StructId,
        field: String,
    },
    MissingField {
        structure: crate::model::ids::StructId,
        field: String,
    },
    /// Lifecycle event the engine does not fold into memory yet.
    UnsupportedEvent {
        event: &'static str,
    },
    /// A scope id was announced twice.
    DuplicateScope {
        scope: crate::model::ids::ScopeId,
    },
    /// The trace closed a scope that is not the running one.
    ScopeNotCurrent {
        scope: crate::model::ids::ScopeId,
    },
    /// A second loan would break the first one.
    ConflictingBorrow {
        target: Path,
    },
    /// Nothing was lent under that name.
    UnknownBorrow {
        borrower: Path,
    },
    /// `Move` needs two stack slots in this engine.
    UnsupportedMove {
        from: Path,
        to: Path,
    },
}

impl RejectionReason {
    pub fn message(&self) -> String {
        match self {
            RejectionReason::OutOfOrderSequence { last, received } => match last {
                Some(last) => format!(
                    "trace is at sequence {} but received {} (must increase)",
                    last.0, received.0
                ),
                None => format!(
                    "trace starts at sequence {} (must be at least 1)",
                    received.0
                ),
            },
            RejectionReason::UnknownFunction { function } => {
                format!("no function with id {}", function.0)
            }
            RejectionReason::EmptyStack => "no open frame".to_string(),
            RejectionReason::DepthExceeded { maximum } => {
                format!("frame depth limit of {} reached", maximum)
            }
            RejectionReason::DuplicateVariable { name } => {
                format!("variable '{}' already exists in this frame", name)
            }
            RejectionReason::UnknownVariable { name } => {
                format!("variable '{}' is not declared here", name)
            }
            RejectionReason::FrameNotCurrent { frame } => {
                format!("frame {} is not the running frame", frame.0)
            }
            RejectionReason::UnknownObject { object } => {
                format!("no object with id {}", object.0)
            }
            RejectionReason::DuplicateObject { object } => {
                format!("object id {} was already created", object.0)
            }
            RejectionReason::ReleasedObject { object } => {
                format!("object {} is already released", object.0)
            }
            RejectionReason::InvalidPath { path } => format!("path is not writable: {:?}", path),
            RejectionReason::TypeMismatch { expected, received } => {
                format!("expected {:?}, received {:?}", expected, received)
            }
            RejectionReason::UnknownStructure { structure } => {
                format!("no struct with id {}", structure.0)
            }
            RejectionReason::UnknownField { structure, field } => {
                format!("struct {} has no field '{}'", structure.0, field)
            }
            RejectionReason::MissingField { structure, field } => {
                format!(
                    "object of struct {} is missing field '{}'",
                    structure.0, field
                )
            }
            RejectionReason::UnsupportedEvent { event } => {
                format!("'{}' is not folded into memory yet", event)
            }
            RejectionReason::DuplicateScope { scope } => {
                format!("scope {} was already opened", scope.0)
            }
            RejectionReason::ScopeNotCurrent { scope } => {
                format!("scope {} is not the running scope", scope.0)
            }
            RejectionReason::ConflictingBorrow { target } => {
                format!("cannot borrow {:?} while another loan is live", target)
            }
            RejectionReason::UnknownBorrow { borrower } => {
                format!("no live loan for {:?}", borrower)
            }
            RejectionReason::UnsupportedMove { from, to } => {
                format!("cannot move {:?} into {:?}", from, to)
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Rejection {
    pub reason: RejectionReason,
    pub location: Location,
}

/// What the UI has to animate. Effects carry no timing: the edge decides speed.
#[derive(Clone, PartialEq, Debug)]
pub enum Effect {
    Wrote {
        path: Path,
    },
    CreatedObject {
        object: ObjectId,
    },
    ReleasedObject {
        object: ObjectId,
    },
    FramePushed {
        frame: FrameId,
    },
    FramePopped {
        frame: FrameId,
    },
    LineChanged {
        line: u32,
    },
    IncidentRecorded {
        location: Location,
    },
    /// A value left its cell because its scope ended or it was moved.
    Dropped {
        path: Path,
    },
    /// A loan started or ended. The UI draws and erases the arrow.
    Borrowed {
        borrower: Path,
        target: Path,
    },
    ReleasedBorrow {
        borrower: Path,
    },
    /// Ownership moved; `from` is now empty.
    Moved {
        from: Path,
        to: Path,
    },
}

/// One entry of the reducer: the state moves in, the next state moves out.
#[derive(Clone, PartialEq, Debug)]
pub struct Application<'a> {
    pub state: MemoryState,
    pub event: &'a crate::model::events::TraceEvent,
    pub program: &'a ProgramTree,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ApplicationOutcome {
    Applied {
        state: MemoryState,
        effects: Vec<Effect>,
    },
    Rejected {
        state: MemoryState,
        incident: Incident,
    },
}
