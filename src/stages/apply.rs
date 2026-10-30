use crate::model::events::{BorrowKind, TraceEvent};
use crate::model::ids::{FrameId, FunctionId, ObjectId, ScopeId, Sequence, StructId};
use crate::model::memory::{
    Field, HeapObject, Loan, Location, MAX_FRAME_DEPTH, MemoryState, ObjectShape, ObjectState,
    Path, Segment, StackFrame, StackVariable, Value,
};
use crate::model::outcome::{
    Application, ApplicationOutcome, Effect, ErrorClass, Incident, Rejection, RejectionReason,
};
use crate::model::program::{DataType, FunctionDef, ProgramTree, ValueKind};
use crate::model::scope::ScopeKind;

pub fn apply_event(application: Application) -> ApplicationOutcome {
    let Application {
        state,
        event,
        program,
    } = application;
    let mut next = state;
    let received = event.sequence();
    // A trace may skip numbers (dropped telemetry) but never go back: a
    // duplicate would re-apply writes that already happened.
    let ordered = match next.last_sequence {
        Some(last) => received.0 > last.0,
        None => received.0 >= 1,
    };
    let sequence_problem = match ordered {
        true => None,
        false => Some(pinned(
            RejectionReason::OutOfOrderSequence {
                last: next.last_sequence,
                received,
            },
            Location::Trace,
        )),
    };
    match sequence_problem {
        Some(rejection) => reject(next, rejection, received),
        None => {
            // The number is consumed even when the payload is rejected: the rest
            // of the trace must keep flowing after a bad event.
            next.last_sequence = Some(received);
            let outcome = dispatch(&mut next, event, program);
            match outcome {
                Ok(effects) => ApplicationOutcome::Applied {
                    state: next,
                    effects,
                },
                Err(rejection) => reject(next, rejection, received),
            }
        }
    }
}

fn reject(state: MemoryState, rejection: Rejection, sequence: Sequence) -> ApplicationOutcome {
    let incident = Incident::InvalidEvent {
        sequence,
        location: rejection.location,
        reason: rejection.reason,
    };
    let mut next = state;
    next.incidents.push(incident.clone());
    ApplicationOutcome::Rejected {
        state: next,
        incident,
    }
}

fn dispatch(
    state: &mut MemoryState,
    event: &TraceEvent,
    program: &ProgramTree,
) -> Result<Vec<Effect>, Rejection> {
    match event {
        TraceEvent::StartFunction {
            function,
            call_line,
            arguments,
            ..
        } => start_function(state, program, *function, *call_line, arguments),
        TraceEvent::EndFunction { .. } => end_function(state),
        TraceEvent::DeclareVariable {
            line, name, value, ..
        } => declare_variable(state, program, *line, name, value),
        TraceEvent::WriteMemory {
            line, path, value, ..
        } => write_memory(state, program, *line, path, value),
        TraceEvent::CreateObject {
            line,
            object,
            label,
            shape,
            ..
        } => create_object(state, program, *line, *object, label, shape),
        TraceEvent::ReleaseObject { object, .. } => release_object(state, *object),
        TraceEvent::ConditionalJump { to_line, .. } => conditional_jump(state, *to_line),
        TraceEvent::RuntimeError {
            sequence,
            line,
            path,
            class,
            message,
        } => runtime_error(state, *sequence, *line, path, *class, message),
        TraceEvent::EnterScope {
            line, scope, kind, ..
        } => enter_scope(state, *line, *scope, *kind),
        TraceEvent::ExitScope { scope, .. } => exit_scope(state, *scope),
        TraceEvent::Borrow {
            line,
            borrower,
            target,
            kind,
            ..
        } => borrow(state, *line, borrower, target, *kind),
        TraceEvent::ReleaseBorrow { line, borrower, .. } => release_borrow(state, *line, borrower),
        TraceEvent::Drop { line, path, .. } => drop_value(state, *line, path),
        TraceEvent::Move { line, from, to, .. } => move_value(state, *line, from, to),
    }
}

// --------------------------------------------------------------------- frames

fn start_function(
    state: &mut MemoryState,
    program: &ProgramTree,
    function: FunctionId,
    call_line: u32,
    arguments: &[Field],
) -> Result<Vec<Effect>, Rejection> {
    let definition = match program.function(function) {
        Some(definition) => definition,
        None => {
            return Err(pinned(
                RejectionReason::UnknownFunction { function },
                Location::Trace,
            ));
        }
    };
    match state.frames.len() < MAX_FRAME_DEPTH {
        true => push_frame(state, definition, call_line, arguments),
        false => Err(pinned(
            RejectionReason::DepthExceeded {
                maximum: MAX_FRAME_DEPTH,
            },
            Location::Stack,
        )),
    }
}

fn push_frame(
    state: &mut MemoryState,
    definition: &FunctionDef,
    call_line: u32,
    arguments: &[Field],
) -> Result<Vec<Effect>, Rejection> {
    let mut variables = Vec::new();
    let mut failure: Option<Rejection> = None;
    for argument in arguments {
        if failure.is_some() {
            break;
        }
        match bind_argument(state, definition, argument) {
            Ok(variable) => variables.push(variable),
            Err(rejection) => failure = Some(rejection),
        }
    }
    match failure {
        Some(rejection) => Err(rejection),
        None => {
            if let Some(caller) = state.frames.last_mut() {
                caller.current_line = call_line;
            }
            let frame = StackFrame {
                id: FrameId(state.next_frame_id),
                function: definition.id,
                current_line: definition.start_line,
                variables,
            };
            state.next_frame_id += 1;
            state.frames.push(frame.clone());
            Ok(vec![Effect::FramePushed { frame: frame.id }])
        }
    }
}

fn bind_argument(
    state: &MemoryState,
    definition: &FunctionDef,
    argument: &Field,
) -> Result<StackVariable, Rejection> {
    let declaration = match definition.declaration_named(&argument.name) {
        Some(declaration) => declaration,
        None => {
            return Err(pinned(
                RejectionReason::UnknownVariable {
                    name: argument.name.clone(),
                },
                Location::Stack,
            ));
        }
    };
    match check_assignment(state, &declaration.data_type, &argument.value) {
        Some(reason) => Err(pinned(reason, Location::Stack)),
        None => Ok(StackVariable {
            name: argument.name.clone(),
            value: argument.value.clone(),
        }),
    }
}

fn end_function(state: &mut MemoryState) -> Result<Vec<Effect>, Rejection> {
    let frame = match state.frames.last() {
        Some(frame) => frame.id,
        None => return Err(pinned(RejectionReason::EmptyStack, Location::Stack)),
    };
    let mut effects = Vec::new();
    for closed in state.scopes.close_frame(frame) {
        for name in closed.dropped {
            effects.push(Effect::Dropped {
                path: Path::Stack {
                    frame,
                    variable: name,
                },
            });
        }
    }
    match state.frames.pop() {
        Some(frame) => {
            effects.push(Effect::FramePopped { frame: frame.id });
            Ok(effects)
        }
        None => Err(pinned(RejectionReason::EmptyStack, Location::Stack)),
    }
}

// ------------------------------------------------------------------ variables

fn declare_variable(
    state: &mut MemoryState,
    program: &ProgramTree,
    line: u32,
    name: &str,
    value: &Value,
) -> Result<Vec<Effect>, Rejection> {
    let (frame_id, function, duplicate) = match state.current_frame() {
        Some(frame) => (
            frame.id,
            frame.function,
            frame
                .variables
                .iter()
                .any(|candidate| candidate.name == name),
        ),
        None => return Err(pinned(RejectionReason::EmptyStack, Location::Stack)),
    };
    let path = Path::Stack {
        frame: frame_id,
        variable: name.to_string(),
    };
    let declared = match program.find_declaration(function, name) {
        Some(declaration) => declaration.data_type.clone(),
        None => {
            return Err(pinned(
                RejectionReason::UnknownVariable {
                    name: name.to_string(),
                },
                cell(&path),
            ));
        }
    };
    match duplicate {
        true => Err(pinned(
            RejectionReason::DuplicateVariable {
                name: name.to_string(),
            },
            cell(&path),
        )),
        false => match check_assignment(state, &declared, value) {
            Some(reason) => Err(pinned(reason, cell(&path))),
            None => {
                if let Some(frame) = state.current_frame_mut() {
                    frame.variables.push(StackVariable {
                        name: name.to_string(),
                        value: value.clone(),
                    });
                    frame.current_line = line;
                }
                state.scopes.declare(name);
                Ok(vec![Effect::Wrote { path }, Effect::LineChanged { line }])
            }
        },
    }
}

fn write_memory(
    state: &mut MemoryState,
    program: &ProgramTree,
    line: u32,
    path: &Path,
    value: &Value,
) -> Result<Vec<Effect>, Rejection> {
    let target = match resolve_target(state, program, path) {
        Ok(target) => target,
        Err(rejection) => return Err(rejection),
    };
    match check_assignment(state, &target.expected, value) {
        Some(reason) => Err(pinned(reason, cell(path))),
        None => {
            store(state, &target, value.clone());
            if let Some(frame) = state.current_frame_mut() {
                frame.current_line = line;
            }
            Ok(vec![
                Effect::Wrote { path: path.clone() },
                Effect::LineChanged { line },
            ])
        }
    }
}

// --------------------------------------------------------------- path walking

struct Target {
    kind: TargetKind,
    expected: DataType,
}

enum TargetKind {
    StackVariable { index: usize },
    HeapField { object: usize, field: usize },
    HeapElement { object: usize, element: usize },
}

fn resolve_target(
    state: &MemoryState,
    program: &ProgramTree,
    path: &Path,
) -> Result<Target, Rejection> {
    match path {
        Path::Stack { frame, variable } => {
            resolve_stack_target(state, program, *frame, variable, path)
        }
        Path::Object { object, segments } => {
            resolve_object_target(state, program, *object, segments, path)
        }
    }
}

fn resolve_stack_target(
    state: &MemoryState,
    program: &ProgramTree,
    requested: FrameId,
    variable: &str,
    path: &Path,
) -> Result<Target, Rejection> {
    let frame = match state.current_frame() {
        Some(frame) => frame,
        None => return Err(pinned(RejectionReason::EmptyStack, Location::Stack)),
    };
    if frame.id != requested {
        return Err(pinned(
            RejectionReason::FrameNotCurrent { frame: requested },
            Location::Frame(requested),
        ));
    }
    let expected = match program.find_declaration(frame.function, variable) {
        Some(declaration) => declaration.data_type.clone(),
        None => return Err(unknown_variable(variable, path)),
    };
    match frame
        .variables
        .iter()
        .position(|candidate| candidate.name == variable)
    {
        Some(index) => Ok(Target {
            kind: TargetKind::StackVariable { index },
            expected,
        }),
        None => Err(unknown_variable(variable, path)),
    }
}

fn resolve_object_target(
    state: &MemoryState,
    program: &ProgramTree,
    object: ObjectId,
    segments: &[Segment],
    path: &Path,
) -> Result<Target, Rejection> {
    let (last, prefix) = match segments.split_last() {
        Some(parts) => parts,
        None => return Err(invalid_path(path)),
    };
    let mut cursor = object;
    for segment in prefix {
        match follow_segment(state, cursor, segment, path) {
            Ok(next) => cursor = next,
            Err(rejection) => return Err(rejection),
        }
    }
    let heap_index = match state.object_index(cursor) {
        Some(index) => index,
        None => return Err(unknown_object(cursor)),
    };
    let heap_object = match state.object(cursor) {
        Some(target) => target,
        None => return Err(unknown_object(cursor)),
    };
    if heap_object.state == ObjectState::Released {
        return Err(pinned(
            RejectionReason::ReleasedObject { object: cursor },
            cell(path),
        ));
    }
    match (&heap_object.shape, last) {
        (ObjectShape::Structure { structure, fields }, Segment::Field(name)) => {
            final_field(program, *structure, fields, name, heap_index, path)
        }
        (
            ObjectShape::Sequence {
                element_type,
                elements,
            },
            Segment::Index(index),
        ) => final_element(*index, element_type, elements.len(), heap_index, path),
        (ObjectShape::Structure { .. }, Segment::Index(_)) => Err(invalid_path(path)),
        (ObjectShape::Sequence { .. }, Segment::Field(_)) => Err(invalid_path(path)),
    }
}

fn final_field(
    program: &ProgramTree,
    structure: StructId,
    fields: &[Field],
    name: &str,
    heap_index: usize,
    path: &Path,
) -> Result<Target, Rejection> {
    let field = match fields.iter().position(|candidate| candidate.name == name) {
        Some(index) => index,
        None => return Err(invalid_path(path)),
    };
    let expected = match program.find_field(structure, name) {
        Some(declaration) => declaration.data_type.clone(),
        None => return Err(invalid_path(path)),
    };
    Ok(Target {
        kind: TargetKind::HeapField {
            object: heap_index,
            field,
        },
        expected,
    })
}

fn final_element(
    index: usize,
    element_type: &DataType,
    length: usize,
    heap_index: usize,
    path: &Path,
) -> Result<Target, Rejection> {
    match index < length {
        true => Ok(Target {
            kind: TargetKind::HeapElement {
                object: heap_index,
                element: index,
            },
            expected: element_type.clone(),
        }),
        false => Err(invalid_path(path)),
    }
}

/// Follows one intermediate segment. The value there must be a live reference.
fn follow_segment(
    state: &MemoryState,
    object: ObjectId,
    segment: &Segment,
    path: &Path,
) -> Result<ObjectId, Rejection> {
    let heap_object = match state.object(object) {
        Some(target) => target,
        None => return Err(unknown_object(object)),
    };
    if heap_object.state == ObjectState::Released {
        return Err(pinned(
            RejectionReason::ReleasedObject { object },
            cell(path),
        ));
    }
    let value = match (&heap_object.shape, segment) {
        (ObjectShape::Structure { fields, .. }, Segment::Field(name)) => fields
            .iter()
            .find(|candidate| &candidate.name == name)
            .map(|field| &field.value),
        (ObjectShape::Sequence { elements, .. }, Segment::Index(index)) => elements.get(*index),
        (ObjectShape::Structure { .. }, Segment::Index(_)) => None,
        (ObjectShape::Sequence { .. }, Segment::Field(_)) => None,
    };
    match value {
        Some(Value::Reference(next)) => Ok(*next),
        Some(Value::Primitive(_)) => Err(invalid_path(path)),
        None => Err(invalid_path(path)),
    }
}

// ---------------------------------------------------------------- type checks

fn check_assignment(
    state: &MemoryState,
    expected: &DataType,
    value: &Value,
) -> Option<RejectionReason> {
    match value {
        Value::Primitive(primitive) => compare_kind(expected, primitive.kind()),
        Value::Reference(object) => match state.object(*object) {
            None => Some(RejectionReason::UnknownObject { object: *object }),
            Some(target) => match target.state {
                ObjectState::Released => Some(RejectionReason::ReleasedObject { object: *object }),
                ObjectState::Alive => compare_kind(expected, target.shape.kind()),
            },
        },
    }
}

fn compare_kind(expected: &DataType, actual: ValueKind) -> Option<RejectionReason> {
    match actual == expected.kind() {
        true => None,
        false => Some(RejectionReason::TypeMismatch {
            expected: expected.clone(),
            received: actual,
        }),
    }
}

// -------------------------------------------------------------------- storage

fn store(state: &mut MemoryState, target: &Target, value: Value) {
    match target.kind {
        TargetKind::StackVariable { index } => store_stack(state, index, value),
        TargetKind::HeapField { object, field } => store_field(state, object, field, value),
        TargetKind::HeapElement { object, element } => {
            store_element(state, object, element, value);
        }
    }
}

// The three helpers below only run after `resolve_target` produced an index in
// this same state, so the slot exists. `if let` keeps the borrow checker happy
// without adding a second failure path the caller would have to interpret.
fn store_stack(state: &mut MemoryState, index: usize, value: Value) {
    if let Some(frame) = state.frames.last_mut()
        && let Some(slot) = frame.variables.get_mut(index)
    {
        slot.value = value;
    }
}

fn store_field(state: &mut MemoryState, object: usize, field: usize, value: Value) {
    if let Some(target) = state.heap.get_mut(object)
        && let ObjectShape::Structure { fields, .. } = &mut target.shape
        && let Some(slot) = fields.get_mut(field)
    {
        slot.value = value;
    }
}

fn store_element(state: &mut MemoryState, object: usize, element: usize, value: Value) {
    if let Some(target) = state.heap.get_mut(object)
        && let ObjectShape::Sequence { elements, .. } = &mut target.shape
        && let Some(slot) = elements.get_mut(element)
    {
        *slot = value;
    }
}

// ---------------------------------------------------------------------- heap

fn create_object(
    state: &mut MemoryState,
    program: &ProgramTree,
    line: u32,
    object: ObjectId,
    label: &str,
    shape: &ObjectShape,
) -> Result<Vec<Effect>, Rejection> {
    let problem = match state.object(object).is_some() {
        true => Some(pinned(
            RejectionReason::DuplicateObject { object },
            Location::Heap,
        )),
        false => check_shape(state, program, object, shape),
    };
    match problem {
        Some(rejection) => Err(rejection),
        None => {
            state.heap.push(HeapObject {
                id: object,
                label: label.to_string(),
                state: ObjectState::Alive,
                shape: shape.clone(),
            });
            if let Some(frame) = state.current_frame_mut() {
                frame.current_line = line;
            }
            Ok(vec![
                Effect::CreatedObject { object },
                Effect::LineChanged { line },
            ])
        }
    }
}

fn check_shape(
    state: &MemoryState,
    program: &ProgramTree,
    object: ObjectId,
    shape: &ObjectShape,
) -> Option<Rejection> {
    match shape {
        ObjectShape::Structure { structure, fields } => {
            check_structure(state, program, object, *structure, fields)
        }
        ObjectShape::Sequence {
            element_type,
            elements,
        } => check_elements(state, object, element_type, elements),
    }
}

fn check_structure(
    state: &MemoryState,
    program: &ProgramTree,
    object: ObjectId,
    structure: StructId,
    fields: &[Field],
) -> Option<Rejection> {
    let definition = match program.structure(structure) {
        Some(definition) => definition,
        None => {
            return Some(pinned(
                RejectionReason::UnknownStructure { structure },
                Location::Heap,
            ));
        }
    };
    let mut failure: Option<Rejection> = None;
    for field in fields {
        if failure.is_some() {
            break;
        }
        let location = Location::Cell(Path::Object {
            object,
            segments: vec![Segment::Field(field.name.clone())],
        });
        match definition
            .fields
            .iter()
            .find(|declared| declared.name == field.name)
        {
            Some(declared) => {
                if let Some(reason) = check_assignment(state, &declared.data_type, &field.value) {
                    failure = Some(pinned(reason, location));
                }
            }
            None => {
                failure = Some(pinned(
                    RejectionReason::UnknownField {
                        structure,
                        field: field.name.clone(),
                    },
                    location,
                ));
            }
        }
    }
    for declared in &definition.fields {
        if failure.is_some() {
            break;
        }
        if !fields.iter().any(|field| field.name == declared.name) {
            failure = Some(pinned(
                RejectionReason::MissingField {
                    structure,
                    field: declared.name.clone(),
                },
                Location::Heap,
            ));
        }
    }
    failure
}

fn check_elements(
    state: &MemoryState,
    object: ObjectId,
    element_type: &DataType,
    elements: &[Value],
) -> Option<Rejection> {
    let mut failure: Option<Rejection> = None;
    for (index, value) in elements.iter().enumerate() {
        if failure.is_some() {
            break;
        }
        if let Some(reason) = check_assignment(state, element_type, value) {
            failure = Some(pinned(
                reason,
                Location::Cell(Path::Object {
                    object,
                    segments: vec![Segment::Index(index)],
                }),
            ));
        }
    }
    failure
}

fn release_object(state: &mut MemoryState, object: ObjectId) -> Result<Vec<Effect>, Rejection> {
    let index = match state.object_index(object) {
        Some(index) => index,
        None => return Err(unknown_object(object)),
    };
    match state.heap[index].state {
        ObjectState::Released => Err(pinned(
            RejectionReason::ReleasedObject { object },
            Location::Heap,
        )),
        ObjectState::Alive => {
            state.heap[index].state = ObjectState::Released;
            Ok(vec![Effect::ReleasedObject { object }])
        }
    }
}

// ------------------------------------------------------------------- control

fn conditional_jump(state: &mut MemoryState, line: u32) -> Result<Vec<Effect>, Rejection> {
    match state.current_frame_mut() {
        Some(frame) => {
            frame.current_line = line;
            Ok(vec![Effect::LineChanged { line }])
        }
        None => Err(pinned(RejectionReason::EmptyStack, Location::Stack)),
    }
}

fn runtime_error(
    state: &mut MemoryState,
    sequence: Sequence,
    line: u32,
    path: &Option<Path>,
    class: ErrorClass,
    message: &str,
) -> Result<Vec<Effect>, Rejection> {
    let location = match path {
        Some(path) => Location::Cell(path.clone()),
        None => Location::Line(line),
    };
    match state.current_frame_mut() {
        Some(frame) => frame.current_line = line,
        None => return Err(pinned(RejectionReason::EmptyStack, Location::Stack)),
    }
    state.incidents.push(Incident::RuntimeError {
        sequence,
        location: location.clone(),
        class,
        message: message.to_string(),
    });
    Ok(vec![
        Effect::LineChanged { line },
        Effect::IncidentRecorded { location },
    ])
}

// ------------------------------------------------------------------ lifecycle

fn enter_scope(
    state: &mut MemoryState,
    line: u32,
    scope: ScopeId,
    kind: ScopeKind,
) -> Result<Vec<Effect>, Rejection> {
    let frame = match state.current_frame() {
        Some(frame) => frame.id,
        None => return Err(pinned(RejectionReason::EmptyStack, Location::Stack)),
    };
    if state.scopes.open_as(scope, kind, frame, line).is_none() {
        return Err(pinned(
            RejectionReason::DuplicateScope { scope },
            Location::Line(line),
        ));
    }
    set_line(state, line);
    Ok(vec![Effect::LineChanged { line }])
}

fn exit_scope(state: &mut MemoryState, scope: ScopeId) -> Result<Vec<Effect>, Rejection> {
    let current = match state.scopes.current() {
        Some(current) => current.id,
        None => {
            return Err(pinned(
                RejectionReason::ScopeNotCurrent { scope },
                Location::Stack,
            ));
        }
    };
    if current != scope {
        return Err(pinned(
            RejectionReason::ScopeNotCurrent { scope },
            Location::Stack,
        ));
    }
    let closed = match state.scopes.close() {
        Some(closed) => closed,
        None => {
            return Err(pinned(
                RejectionReason::ScopeNotCurrent { scope },
                Location::Stack,
            ));
        }
    };
    let frame = closed.scope.frame;
    let mut effects = Vec::with_capacity(closed.dropped.len());
    for name in &closed.dropped {
        if let Some(target) = state.frames.iter_mut().find(|target| target.id == frame) {
            target.variables.retain(|variable| &variable.name != name);
        }
        effects.push(Effect::Dropped {
            path: Path::Stack {
                frame,
                variable: name.clone(),
            },
        });
    }
    Ok(effects)
}

fn drop_value(state: &mut MemoryState, line: u32, path: &Path) -> Result<Vec<Effect>, Rejection> {
    set_line(state, line);
    match path {
        Path::Stack { frame, variable } => {
            let target = match state.frames.iter_mut().find(|target| target.id == *frame) {
                Some(target) => target,
                None => {
                    return Err(pinned(
                        RejectionReason::FrameNotCurrent { frame: *frame },
                        Location::Cell(path.clone()),
                    ));
                }
            };
            let before = target.variables.len();
            target.variables.retain(|local| &local.name != variable);
            if target.variables.len() == before {
                return Err(unknown_variable(variable, path));
            }
        }
        Path::Object { object, segments } if segments.is_empty() => {
            let mut effects = release_object(state, *object)?;
            effects.push(Effect::Dropped { path: path.clone() });
            return Ok(effects);
        }
        _ => return Err(invalid_path(path)),
    }
    Ok(vec![
        Effect::LineChanged { line },
        Effect::Dropped { path: path.clone() },
    ])
}

fn borrow(
    state: &mut MemoryState,
    line: u32,
    borrower: &Path,
    target: &Path,
    kind: BorrowKind,
) -> Result<Vec<Effect>, Rejection> {
    set_line(state, line);
    if !cell_exists(state, target) {
        return Err(invalid_path(target));
    }
    let conflict = state.loans.iter().any(|loan| {
        loan.target == *target && (kind == BorrowKind::Mutable || loan.kind == BorrowKind::Mutable)
    });
    if conflict {
        return Err(pinned(
            RejectionReason::ConflictingBorrow {
                target: target.clone(),
            },
            cell(target),
        ));
    }
    state.loans.push(Loan {
        borrower: borrower.clone(),
        target: target.clone(),
        kind,
    });
    Ok(vec![
        Effect::LineChanged { line },
        Effect::Borrowed {
            borrower: borrower.clone(),
            target: target.clone(),
        },
    ])
}

fn release_borrow(
    state: &mut MemoryState,
    line: u32,
    borrower: &Path,
) -> Result<Vec<Effect>, Rejection> {
    set_line(state, line);
    let position = match state
        .loans
        .iter()
        .position(|loan| loan.borrower == *borrower)
    {
        Some(position) => position,
        None => {
            return Err(pinned(
                RejectionReason::UnknownBorrow {
                    borrower: borrower.clone(),
                },
                cell(borrower),
            ));
        }
    };
    state.loans.remove(position);
    Ok(vec![
        Effect::LineChanged { line },
        Effect::ReleasedBorrow {
            borrower: borrower.clone(),
        },
    ])
}

/// Moves one stack slot into another. Heap moves need a shape check the
/// recorder cannot express yet, so they are rejected instead of guessed.
fn move_value(
    state: &mut MemoryState,
    line: u32,
    from: &Path,
    to: &Path,
) -> Result<Vec<Effect>, Rejection> {
    set_line(state, line);
    let (source_frame, source_name) = match from {
        Path::Stack { frame, variable } => (*frame, variable.clone()),
        _ => return Err(unsupported_move(from, to)),
    };
    let (target_frame, target_name) = match to {
        Path::Stack { frame, variable } => (*frame, variable.clone()),
        _ => return Err(unsupported_move(from, to)),
    };
    let value = state
        .frames
        .iter()
        .find(|frame| frame.id == source_frame)
        .and_then(|frame| {
            frame
                .variables
                .iter()
                .find(|local| local.name == source_name)
        })
        .map(|local| local.value.clone());
    let value = match value {
        Some(value) => value,
        None => return Err(unknown_variable(&source_name, from)),
    };
    let target = state
        .frames
        .iter_mut()
        .find(|frame| frame.id == target_frame)
        .and_then(|frame| {
            frame
                .variables
                .iter_mut()
                .find(|local| local.name == target_name)
        });
    match target {
        Some(local) => local.value = value,
        None => return Err(unknown_variable(&target_name, to)),
    }
    if let Some(frame) = state
        .frames
        .iter_mut()
        .find(|frame| frame.id == source_frame)
    {
        frame.variables.retain(|local| local.name != source_name);
    }
    Ok(vec![
        Effect::LineChanged { line },
        Effect::Moved {
            from: from.clone(),
            to: to.clone(),
        },
        Effect::Dropped { path: from.clone() },
    ])
}

fn set_line(state: &mut MemoryState, line: u32) {
    if let Some(frame) = state.current_frame_mut() {
        frame.current_line = line;
    }
}

fn cell_exists(state: &MemoryState, path: &Path) -> bool {
    match path {
        Path::Stack { frame, variable } => state.frames.iter().any(|target| {
            target.id == *frame && target.variables.iter().any(|local| &local.name == variable)
        }),
        Path::Object { object, .. } => state.object(*object).is_some(),
    }
}

fn unsupported_move(from: &Path, to: &Path) -> Rejection {
    pinned(
        RejectionReason::UnsupportedMove {
            from: from.clone(),
            to: to.clone(),
        },
        cell(from),
    )
}

// ------------------------------------------------------------------- helpers

fn pinned(reason: RejectionReason, location: Location) -> Rejection {
    Rejection { reason, location }
}

fn cell(path: &Path) -> Location {
    Location::Cell(path.clone())
}

fn invalid_path(path: &Path) -> Rejection {
    pinned(
        RejectionReason::InvalidPath { path: path.clone() },
        Location::Cell(path.clone()),
    )
}

fn unknown_variable(variable: &str, path: &Path) -> Rejection {
    pinned(
        RejectionReason::UnknownVariable {
            name: variable.to_string(),
        },
        cell(path),
    )
}

fn unknown_object(object: ObjectId) -> Rejection {
    pinned(RejectionReason::UnknownObject { object }, Location::Heap)
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::model::memory::{PrimitiveValue, StackVariable};

    fn program() -> ProgramTree {
        ProgramTree {
            entry: FunctionId(0),
            functions: Vec::new(),
            structs: Vec::new(),
        }
    }

    fn started() -> MemoryState {
        let mut state = MemoryState::empty();
        state.frames.push(StackFrame {
            id: FrameId(1),
            function: FunctionId(0),
            current_line: 1,
            variables: Vec::new(),
        });
        state.next_frame_id = 2;
        state
    }

    fn local(name: &str, value: i64) -> StackVariable {
        StackVariable {
            name: name.to_string(),
            value: Value::Primitive(PrimitiveValue::Integer(value)),
        }
    }

    fn apply(state: MemoryState, event: TraceEvent) -> ApplicationOutcome {
        let program = program();
        apply_event(Application {
            state,
            event: &event,
            program: &program,
        })
    }

    fn applied(state: MemoryState, event: TraceEvent) -> (MemoryState, Vec<Effect>) {
        match apply(state, event) {
            ApplicationOutcome::Applied { state, effects } => (state, effects),
            ApplicationOutcome::Rejected { incident, .. } => {
                panic!("unexpected rejection: {:?}", incident)
            }
        }
    }

    /// The reason of a rejection. Panics when the event is accepted, so a test
    /// that expects a guard cannot pass by accident.
    fn rejection(state: MemoryState, event: TraceEvent) -> RejectionReason {
        match apply(state, event) {
            ApplicationOutcome::Rejected { incident, .. } => match incident {
                Incident::InvalidEvent { reason, .. } => reason,
                other => panic!("unexpected incident: {:?}", other),
            },
            ApplicationOutcome::Applied { .. } => panic!("the event must be rejected"),
        }
    }

    fn stack(variable: &str) -> Path {
        Path::Stack {
            frame: FrameId(1),
            variable: variable.to_string(),
        }
    }

    #[test]
    fn exit_scope_drops_variables_last_declared_first() {
        let mut state = started();
        state
            .scopes
            .open_as(ScopeId(1), ScopeKind::Block, FrameId(1), 1)
            .expect("scope opens");
        state.frames[0].variables = vec![local("a", 1), local("b", 2), local("c", 3)];
        for name in ["a", "b", "c"] {
            state.scopes.declare(name);
        }

        let (next, effects) = applied(
            state,
            TraceEvent::ExitScope {
                sequence: Sequence(1),
                scope: ScopeId(1),
            },
        );

        assert!(next.frames[0].variables.is_empty());
        assert!(next.scopes.is_empty());
        let dropped: Vec<&str> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Dropped {
                    path: Path::Stack { variable, .. },
                } => Some(variable.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(dropped, vec!["c", "b", "a"]);
    }

    #[test]
    fn a_mutable_loan_conflicts_with_a_live_immutable_one() {
        let mut state = started();
        state.frames[0].variables = vec![local("value", 1), local("first", 0), local("second", 0)];
        let target = stack("value");

        let (state, _) = applied(
            state,
            TraceEvent::Borrow {
                sequence: Sequence(1),
                line: 2,
                borrower: stack("first"),
                target: target.clone(),
                kind: BorrowKind::Immutable,
            },
        );
        let (state, _) = applied(
            state,
            TraceEvent::Borrow {
                sequence: Sequence(2),
                line: 3,
                borrower: stack("second"),
                target: target.clone(),
                kind: BorrowKind::Immutable,
            },
        );
        assert_eq!(state.loans.len(), 2);

        let rejected = apply(
            state,
            TraceEvent::Borrow {
                sequence: Sequence(3),
                line: 4,
                borrower: stack("second"),
                target,
                kind: BorrowKind::Mutable,
            },
        );
        assert!(matches!(rejected, ApplicationOutcome::Rejected { .. }));
    }

    #[test]
    fn releasing_a_missing_loan_is_rejected() {
        let state = started();
        let rejected = apply(
            state,
            TraceEvent::ReleaseBorrow {
                sequence: Sequence(1),
                line: 2,
                borrower: stack("nobody"),
            },
        );
        assert!(matches!(rejected, ApplicationOutcome::Rejected { .. }));
    }

    #[test]
    fn moving_a_stack_slot_empties_the_source() {
        let mut state = started();
        state.frames[0].variables = vec![local("source", 7), local("target", 0)];

        let (next, effects) = applied(
            state,
            TraceEvent::Move {
                sequence: Sequence(1),
                line: 2,
                from: stack("source"),
                to: stack("target"),
            },
        );

        assert_eq!(next.frames[0].variables.len(), 1);
        assert_eq!(next.frames[0].variables[0].name, "target");
        assert_eq!(
            next.frames[0].variables[0].value,
            Value::Primitive(PrimitiveValue::Integer(7))
        );
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Moved { .. }))
        );
    }

    #[test]
    fn opening_a_scope_that_already_exists_is_rejected() {
        let mut state = started();
        state
            .scopes
            .open_as(ScopeId(1), ScopeKind::Function, FrameId(1), 1)
            .expect("first scope");

        let reason = rejection(
            state,
            TraceEvent::EnterScope {
                sequence: Sequence(1),
                line: 2,
                scope: ScopeId(1),
                kind: ScopeKind::Block,
                parent: Some(ScopeId(1)),
            },
        );

        assert_eq!(
            reason,
            RejectionReason::DuplicateScope { scope: ScopeId(1) }
        );
    }

    #[test]
    fn closing_a_scope_that_is_not_on_top_is_rejected() {
        let mut state = started();
        state
            .scopes
            .open_as(ScopeId(1), ScopeKind::Function, FrameId(1), 1)
            .expect("outer scope");
        state
            .scopes
            .open_as(ScopeId(2), ScopeKind::Block, FrameId(1), 2)
            .expect("inner scope");

        let reason = rejection(
            state,
            TraceEvent::ExitScope {
                sequence: Sequence(1),
                scope: ScopeId(1),
            },
        );

        assert_eq!(
            reason,
            RejectionReason::ScopeNotCurrent { scope: ScopeId(1) }
        );
    }

    #[test]
    fn moving_into_a_heap_cell_is_not_supported() {
        let mut state = started();
        state.frames[0].variables = vec![local("source", 7)];
        let from = stack("source");
        let to = Path::Object {
            object: ObjectId(1),
            segments: Vec::new(),
        };

        let reason = rejection(
            state,
            TraceEvent::Move {
                sequence: Sequence(1),
                line: 2,
                from: from.clone(),
                to: to.clone(),
            },
        );

        assert_eq!(reason, RejectionReason::UnsupportedMove { from, to });
    }
}
