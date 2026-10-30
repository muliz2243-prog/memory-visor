/// Index into `ProgramTree::functions`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FunctionId(pub u32);

/// Index into `ProgramTree::structs`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StructId(pub u32);

/// Identifier of an open stack frame. Assigned by the engine, never reused.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FrameId(pub u32);

/// Identifier of a lexical scope. Assigned by the engine, never reused.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ScopeId(pub u32);

/// Identifier of a heap object. Assigned by the trace producer so the real
/// telemetry keeps the same ids it had at runtime.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ObjectId(pub u32);

/// Monotonic position of an event inside a trace. Starts at 1.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Sequence(pub u64);
