use crate::model::events::BorrowKind;
use crate::model::ids::{FrameId, FunctionId, ObjectId, Sequence, StructId};
use crate::model::outcome::Incident;
use crate::model::program::{DataType, ValueKind};
use crate::model::scope::ScopeRegistry;

/// The UI refuses to draw deeper stacks instead of running out of screen.
pub const MAX_FRAME_DEPTH: usize = 256;

#[derive(Clone, PartialEq, Debug)]
pub enum PrimitiveValue {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Character(char),
    Text(String),
}

impl PrimitiveValue {
    pub fn kind(&self) -> ValueKind {
        match self {
            PrimitiveValue::Integer(_) => ValueKind::Integer,
            PrimitiveValue::Float(_) => ValueKind::Float,
            PrimitiveValue::Boolean(_) => ValueKind::Boolean,
            PrimitiveValue::Character(_) => ValueKind::Character,
            PrimitiveValue::Text(_) => ValueKind::Text,
        }
    }
}

/// What a slot holds. A reference is an object id, which is what the UI turns
/// into an arrow.
#[derive(Clone, PartialEq, Debug)]
pub enum Value {
    Primitive(PrimitiveValue),
    Reference(ObjectId),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Field {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectState {
    Alive,
    Released,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ObjectShape {
    Structure {
        structure: StructId,
        fields: Vec<Field>,
    },
    Sequence {
        element_type: DataType,
        elements: Vec<Value>,
    },
}

impl ObjectShape {
    pub fn kind(&self) -> ValueKind {
        match self {
            ObjectShape::Structure { structure, .. } => ValueKind::Structure(*structure),
            ObjectShape::Sequence { .. } => ValueKind::Sequence,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct HeapObject {
    pub id: ObjectId,
    pub label: String,
    pub state: ObjectState,
    pub shape: ObjectShape,
}

#[derive(Clone, PartialEq, Debug)]
pub struct StackVariable {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, PartialEq, Debug)]
pub struct StackFrame {
    pub id: FrameId,
    pub function: FunctionId,
    pub current_line: u32,
    pub variables: Vec<StackVariable>,
}

/// One step of a path into a heap object.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Segment {
    Field(String),
    Index(usize),
}

/// Address of a writable cell or of a referenced cell.
///
/// The stack variant carries the frame because two frames can hold a variable
/// with the same name. Without it, two different cells would share one address.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Path {
    Stack {
        frame: FrameId,
        variable: String,
    },
    Object {
        object: ObjectId,
        segments: Vec<Segment>,
    },
}

/// Where an incident is pinned so the UI can mark the exact box that failed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Location {
    /// The trace itself: ordering, unknown function. Nothing in memory to blame.
    Trace,
    /// The stack as a whole.
    Stack,
    Frame(FrameId),
    Cell(Path),
    Line(u32),
    /// The heap as a whole.
    Heap,
}

/// Data a reference keeps alive, plus who holds it.
#[derive(Clone, PartialEq, Debug)]
pub struct Loan {
    pub borrower: Path,
    pub target: Path,
    pub kind: BorrowKind,
}

#[derive(Clone, PartialEq, Debug)]
pub struct MemoryState {
    pub frames: Vec<StackFrame>,
    pub heap: Vec<HeapObject>,
    /// Lexical scopes, open and closed. Closing one drops its variables.
    pub scopes: ScopeRegistry,
    /// Live `&`/`&mut` loans. A mutable loan excludes every other loan.
    pub loans: Vec<Loan>,
    pub last_sequence: Option<Sequence>,
    /// Grows with the trace. Part of the snapshot, so scrubbing back also
    /// removes the incidents that had not happened yet.
    pub incidents: Vec<Incident>,
    pub next_frame_id: u32,
}

impl MemoryState {
    pub fn empty() -> Self {
        MemoryState {
            frames: Vec::new(),
            heap: Vec::new(),
            scopes: ScopeRegistry::new(),
            loans: Vec::new(),
            last_sequence: None,
            incidents: Vec::new(),
            next_frame_id: 1,
        }
    }

    pub fn current_frame(&self) -> Option<&StackFrame> {
        self.frames.last()
    }

    pub fn current_frame_mut(&mut self) -> Option<&mut StackFrame> {
        self.frames.last_mut()
    }

    pub fn current_line(&self) -> Option<u32> {
        self.current_frame().map(|frame| frame.current_line)
    }

    pub fn object(&self, id: ObjectId) -> Option<&HeapObject> {
        self.heap.iter().find(|object| object.id == id)
    }

    pub fn object_index(&self, id: ObjectId) -> Option<usize> {
        self.heap.iter().position(|object| object.id == id)
    }

    /// Incident pinned exactly at this location, if any.
    pub fn incident_at(&self, location: &Location) -> Option<&Incident> {
        self.incidents
            .iter()
            .find(|incident| incident.location() == location)
    }
}
