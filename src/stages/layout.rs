use crate::model::ids::{FrameId, ObjectId, ScopeId};
use crate::model::memory::{
    HeapObject, MemoryState, ObjectShape, Path, Segment, StackFrame, Value,
};
use crate::model::scope::ScopeKind;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }
}

/// All sizes are points. The UI may override them; the defaults keep the two
/// columns apart and every row the same height.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LayoutConfig {
    pub origin_y: f32,
    pub stack_x: f32,
    pub heap_x: f32,
    pub column_width: f32,
    pub header_height: f32,
    pub slot_height: f32,
    pub padding: f32,
    pub frame_gap: f32,
    pub object_gap: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        LayoutConfig {
            origin_y: 24.0,
            stack_x: 24.0,
            heap_x: 340.0,
            column_width: 240.0,
            header_height: 28.0,
            slot_height: 24.0,
            padding: 8.0,
            frame_gap: 16.0,
            object_gap: 16.0,
        }
    }
}

/// One drawable row. `path` is the same address used by effects and incidents,
/// so the UI can ask "is this cell the one that failed?" without guessing.
#[derive(Clone, PartialEq, Debug)]
pub struct SlotBox {
    pub path: Path,
    pub rect: Rect,
}

#[derive(Clone, PartialEq, Debug)]
pub struct FrameBox {
    pub frame: FrameId,
    pub rect: Rect,
    pub header: Rect,
    pub slots: Vec<SlotBox>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ObjectBox {
    pub object: ObjectId,
    pub rect: Rect,
    pub header: Rect,
    pub slots: Vec<SlotBox>,
}

/// Straight line from a slot to the object it points at.
#[derive(Clone, PartialEq, Debug)]
pub struct Connector {
    pub source: Path,
    pub target: ObjectId,
    pub from: Point,
    pub to: Point,
}

/// Band that groups the variables of one open lexical scope. Nested scopes are
/// inset by `depth`, so the hierarchy is visible without a tree panel.
#[derive(Clone, PartialEq, Debug)]
pub struct ScopeBox {
    pub scope: ScopeId,
    pub kind: ScopeKind,
    pub depth: u32,
    pub rect: Rect,
}

#[derive(Clone, PartialEq, Debug)]
pub struct MemoryLayout {
    pub frames: Vec<FrameBox>,
    pub objects: Vec<ObjectBox>,
    pub connectors: Vec<Connector>,
    pub scopes: Vec<ScopeBox>,
    pub content: Size,
}

impl MemoryLayout {
    /// Rectangle of a cell, or `None` when the path is not drawn.
    pub fn cell(&self, path: &Path) -> Option<&Rect> {
        self.frames
            .iter()
            .flat_map(|frame| frame.slots.iter())
            .chain(self.objects.iter().flat_map(|object| object.slots.iter()))
            .find(|slot| &slot.path == path)
            .map(|slot| &slot.rect)
    }

    pub fn frame(&self, frame: FrameId) -> Option<&FrameBox> {
        self.frames
            .iter()
            .find(|candidate| candidate.frame == frame)
    }

    pub fn object(&self, object: ObjectId) -> Option<&ObjectBox> {
        self.objects
            .iter()
            .find(|candidate| candidate.object == object)
    }

    pub fn scope(&self, scope: ScopeId) -> Option<&ScopeBox> {
        self.scopes
            .iter()
            .find(|candidate| candidate.scope == scope)
    }
}

pub fn layout_memory(state: &MemoryState, config: &LayoutConfig) -> MemoryLayout {
    let frames = layout_frames(state, config);
    let objects = layout_objects(state, config);
    let connectors = connect(state, &frames, &objects, config);
    let scopes = layout_scopes(state, &frames, config);
    let content = measure(&frames, &objects);
    MemoryLayout {
        frames,
        objects,
        connectors,
        scopes,
        content,
    }
}

// ---------------------------------------------------------------------- frames

/// One band per open scope, inset by its depth. A scope with no variables yet
/// still gets a thin band under the frame header, so it is visible the moment
/// it opens.
fn layout_scopes(state: &MemoryState, frames: &[FrameBox], config: &LayoutConfig) -> Vec<ScopeBox> {
    let mut scopes = Vec::new();
    for id in state.scopes.open_scopes() {
        let scope = match state.scopes.scope(*id) {
            Some(scope) => scope,
            None => continue,
        };
        let frame = match frames.iter().find(|frame| frame.frame == scope.frame) {
            Some(frame) => frame,
            None => continue,
        };
        let inset = scope.depth as f32 * config.padding;
        let x = frame.rect.x + inset;
        let width = (frame.rect.width - inset * 2.0).max(config.padding);
        let mut top = frame.header.bottom() + config.padding / 2.0;
        let mut bottom = top + config.slot_height / 2.0;
        let mut found = false;
        for slot in &frame.slots {
            let variable = match &slot.path {
                Path::Stack { variable, .. } => variable,
                Path::Object { .. } => continue,
            };
            if !scope.variables.iter().any(|name| name == variable) {
                continue;
            }
            top = match found {
                true => top.min(slot.rect.y),
                false => slot.rect.y,
            };
            bottom = match found {
                true => bottom.max(slot.rect.bottom()),
                false => slot.rect.bottom(),
            };
            found = true;
        }
        scopes.push(ScopeBox {
            scope: scope.id,
            kind: scope.kind,
            depth: scope.depth,
            rect: Rect {
                x,
                y: top,
                width,
                height: bottom - top,
            },
        });
    }
    scopes
}

fn layout_frames(state: &MemoryState, config: &LayoutConfig) -> Vec<FrameBox> {
    let mut frames = Vec::with_capacity(state.frames.len());
    let mut y = config.origin_y;
    for frame in &state.frames {
        let height = config.header_height
            + frame.variables.len() as f32 * config.slot_height
            + config.padding;
        let rect = Rect {
            x: config.stack_x,
            y,
            width: config.column_width,
            height,
        };
        let header = Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: config.header_height,
        };
        let slots = frame_slots(frame, &rect, config);
        frames.push(FrameBox {
            frame: frame.id,
            rect,
            header,
            slots,
        });
        y += height + config.frame_gap;
    }
    frames
}

fn frame_slots(frame: &StackFrame, rect: &Rect, config: &LayoutConfig) -> Vec<SlotBox> {
    let mut slots = Vec::with_capacity(frame.variables.len());
    let mut y = rect.y + config.header_height;
    for variable in &frame.variables {
        slots.push(SlotBox {
            path: Path::Stack {
                frame: frame.id,
                variable: variable.name.clone(),
            },
            rect: row_rect(rect, y, config),
        });
        y += config.slot_height;
    }
    slots
}

// ---------------------------------------------------------------------- heap

fn layout_objects(state: &MemoryState, config: &LayoutConfig) -> Vec<ObjectBox> {
    let mut objects = Vec::with_capacity(state.heap.len());
    let mut y = config.origin_y;
    for object in &state.heap {
        let height =
            config.header_height + slot_count(object) as f32 * config.slot_height + config.padding;
        let rect = Rect {
            x: config.heap_x,
            y,
            width: config.column_width,
            height,
        };
        let header = Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: config.header_height,
        };
        let slots = object_slots(object, &rect, config);
        objects.push(ObjectBox {
            object: object.id,
            rect,
            header,
            slots,
        });
        y += height + config.object_gap;
    }
    objects
}

fn slot_count(object: &HeapObject) -> usize {
    match &object.shape {
        ObjectShape::Structure { fields, .. } => fields.len(),
        ObjectShape::Sequence { elements, .. } => elements.len(),
    }
}

fn object_slots(object: &HeapObject, rect: &Rect, config: &LayoutConfig) -> Vec<SlotBox> {
    let mut slots = Vec::new();
    let mut y = rect.y + config.header_height;
    match &object.shape {
        ObjectShape::Structure { fields, .. } => {
            for field in fields {
                slots.push(SlotBox {
                    path: Path::Object {
                        object: object.id,
                        segments: vec![Segment::Field(field.name.clone())],
                    },
                    rect: row_rect(rect, y, config),
                });
                y += config.slot_height;
            }
        }
        ObjectShape::Sequence { elements, .. } => {
            for (index, _) in elements.iter().enumerate() {
                slots.push(SlotBox {
                    path: Path::Object {
                        object: object.id,
                        segments: vec![Segment::Index(index)],
                    },
                    rect: row_rect(rect, y, config),
                });
                y += config.slot_height;
            }
        }
    }
    slots
}

// -------------------------------------------------------------------- reading

/// Value stored at a drawn cell. Only the paths the layout produces resolve:
/// stack variables and one field or one index.
pub fn value_at<'a>(state: &'a MemoryState, path: &Path) -> Option<&'a Value> {
    match path {
        Path::Stack { frame, variable } => stack_value(state, *frame, variable),
        Path::Object { object, segments } => object_value(state, *object, segments),
    }
}

fn stack_value<'a>(state: &'a MemoryState, frame: FrameId, variable: &str) -> Option<&'a Value> {
    let target = match state.frames.iter().find(|candidate| candidate.id == frame) {
        Some(target) => target,
        None => return None,
    };
    target
        .variables
        .iter()
        .find(|candidate| candidate.name == variable)
        .map(|candidate| &candidate.value)
}

fn object_value<'a>(
    state: &'a MemoryState,
    object: ObjectId,
    segments: &[Segment],
) -> Option<&'a Value> {
    let target = match state.object(object) {
        Some(target) => target,
        None => return None,
    };
    match (segments, &target.shape) {
        ([Segment::Field(name)], ObjectShape::Structure { fields, .. }) => fields
            .iter()
            .find(|candidate| &candidate.name == name)
            .map(|candidate| &candidate.value),
        ([Segment::Index(index)], ObjectShape::Sequence { elements, .. }) => elements.get(*index),
        _ => None,
    }
}

// ----------------------------------------------------------------- connectors

fn connect(
    state: &MemoryState,
    frames: &[FrameBox],
    objects: &[ObjectBox],
    config: &LayoutConfig,
) -> Vec<Connector> {
    let mut connectors = Vec::new();
    for frame in frames {
        for slot in &frame.slots {
            push_connector(state, objects, slot, config, &mut connectors);
        }
    }
    for object in objects {
        for slot in &object.slots {
            push_connector(state, objects, slot, config, &mut connectors);
        }
    }
    connectors
}

fn push_connector(
    state: &MemoryState,
    objects: &[ObjectBox],
    slot: &SlotBox,
    config: &LayoutConfig,
    connectors: &mut Vec<Connector>,
) {
    let value = value_at(state, &slot.path);
    if let Some(Value::Reference(target)) = value {
        let target_box = objects.iter().find(|candidate| candidate.object == *target);
        if let Some(target_box) = target_box {
            connectors.push(Connector {
                source: slot.path.clone(),
                target: *target,
                from: Point {
                    x: slot.rect.right(),
                    y: slot.rect.center_y(),
                },
                to: Point {
                    x: target_box.rect.x,
                    y: target_box.rect.y + config.header_height / 2.0,
                },
            });
        }
    }
}

// --------------------------------------------------------------------- output

fn measure(frames: &[FrameBox], objects: &[ObjectBox]) -> Size {
    let mut width: f32 = 0.0;
    let mut height: f32 = 0.0;
    for frame in frames {
        width = width.max(frame.rect.right());
        height = height.max(frame.rect.bottom());
    }
    for object in objects {
        width = width.max(object.rect.right());
        height = height.max(object.rect.bottom());
    }
    Size { width, height }
}

fn row_rect(column: &Rect, y: f32, config: &LayoutConfig) -> Rect {
    Rect {
        x: column.x + config.padding,
        y,
        width: column.width - config.padding * 2.0,
        height: config.slot_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ids::FunctionId;
    use crate::model::memory::{Field, HeapObject, ObjectState, PrimitiveValue, StackVariable};

    fn int(value: i64) -> Value {
        Value::Primitive(PrimitiveValue::Integer(value))
    }

    fn variable(name: &str, value: Value) -> StackVariable {
        StackVariable {
            name: name.to_string(),
            value,
        }
    }

    fn frame(id: u32) -> StackFrame {
        StackFrame {
            id: FrameId(id),
            function: FunctionId(id),
            current_line: 1,
            variables: Vec::new(),
        }
    }

    fn point(id: u32, x: i64, y: i64) -> HeapObject {
        HeapObject {
            id: ObjectId(id),
            label: format!("Point#{}", id),
            state: ObjectState::Alive,
            shape: ObjectShape::Structure {
                structure: crate::model::ids::StructId(0),
                fields: vec![
                    Field {
                        name: "x".to_string(),
                        value: int(x),
                    },
                    Field {
                        name: "y".to_string(),
                        value: int(y),
                    },
                ],
            },
        }
    }

    fn two_frames() -> MemoryState {
        let mut state = MemoryState::empty();
        let mut first = frame(1);
        first
            .variables
            .push(variable("origin", Value::Reference(ObjectId(1))));
        first.variables.push(variable("total", int(253)));
        let mut second = frame(2);
        second.variables.push(variable("total", int(0)));
        state.frames.push(first);
        state.frames.push(second);
        state.heap.push(point(1, 3, 4));
        state
    }

    #[test]
    fn an_empty_state_draws_nothing() {
        let layout = layout_memory(&MemoryState::empty(), &LayoutConfig::default());
        assert!(layout.frames.is_empty());
        assert!(layout.objects.is_empty());
        assert!(layout.connectors.is_empty());
        assert_eq!(
            layout.content,
            Size {
                width: 0.0,
                height: 0.0
            }
        );
    }

    #[test]
    fn rows_stack_in_declaration_order() {
        let layout = layout_memory(&two_frames(), &LayoutConfig::default());
        let first = &layout.frames[0];
        assert_eq!(
            first.rect,
            Rect {
                x: 24.0,
                y: 24.0,
                width: 240.0,
                height: 84.0
            }
        );
        assert_eq!(first.slots[0].rect.y, 52.0);
        assert_eq!(first.slots[1].rect.y, 76.0);
        assert_eq!(first.slots[0].rect.right(), 256.0);
    }

    #[test]
    fn a_new_frame_goes_below_the_previous_one() {
        let layout = layout_memory(&two_frames(), &LayoutConfig::default());
        let first = layout.frames[0].rect;
        let second = layout.frames[1].rect;
        assert_eq!(second.y, first.bottom() + 16.0);
        assert_eq!(second.x, first.x);
    }

    #[test]
    fn the_same_name_in_two_frames_gets_two_addresses() {
        let layout = layout_memory(&two_frames(), &LayoutConfig::default());
        let first = &layout.frames[0].slots[1].path;
        let second = &layout.frames[1].slots[0].path;
        assert_ne!(first, second);
        assert!(layout.cell(first).is_some());
        assert!(layout.cell(second).is_some());
    }

    #[test]
    fn struct_fields_are_drawn_on_the_heap_column() {
        let layout = layout_memory(&two_frames(), &LayoutConfig::default());
        let object = &layout.objects[0];
        assert_eq!(object.rect.x, 340.0);
        assert_eq!(object.slots.len(), 2);
        assert_eq!(
            object.slots[1].path,
            Path::Object {
                object: ObjectId(1),
                segments: vec![Segment::Field("y".to_string())],
            }
        );
    }

    #[test]
    fn a_reference_draws_a_connector_between_the_cells() {
        let layout = layout_memory(&two_frames(), &LayoutConfig::default());
        assert_eq!(layout.connectors.len(), 1);
        let connector = &layout.connectors[0];
        assert_eq!(connector.target, ObjectId(1));
        assert_eq!(connector.from, Point { x: 256.0, y: 64.0 });
        assert_eq!(connector.to, Point { x: 340.0, y: 38.0 });
    }

    #[test]
    fn nested_references_are_connected_too() {
        let mut state = two_frames();
        state.heap[0].shape = ObjectShape::Structure {
            structure: crate::model::ids::StructId(0),
            fields: vec![
                Field {
                    name: "x".to_string(),
                    value: int(0),
                },
                Field {
                    name: "next".to_string(),
                    value: Value::Reference(ObjectId(2)),
                },
            ],
        };
        state.heap.push(point(2, 1, 1));
        let layout = layout_memory(&state, &LayoutConfig::default());
        assert_eq!(layout.connectors.len(), 2);
        assert_eq!(layout.connectors[1].target, ObjectId(2));
    }

    #[test]
    fn a_dangling_reference_draws_no_connector() {
        let mut state = MemoryState::empty();
        let mut only = frame(1);
        only.variables
            .push(variable("ghost", Value::Reference(ObjectId(9))));
        state.frames.push(only);
        let layout = layout_memory(&state, &LayoutConfig::default());
        assert!(layout.connectors.is_empty());
    }

    #[test]
    fn a_released_object_keeps_its_place() {
        let mut state = two_frames();
        state.heap[0].state = ObjectState::Released;
        let layout = layout_memory(&state, &LayoutConfig::default());
        assert_eq!(layout.objects.len(), 1);
        assert_eq!(layout.objects[0].slots.len(), 2);
    }

    #[test]
    fn the_content_size_covers_both_columns() {
        let layout = layout_memory(&two_frames(), &LayoutConfig::default());
        assert_eq!(layout.content.width, 580.0);
        assert_eq!(layout.content.height, 184.0);
    }

    #[test]
    fn a_sequence_cell_can_be_read_back() {
        let mut state = MemoryState::empty();
        state.heap.push(HeapObject {
            id: ObjectId(7),
            label: "scores[3]".to_string(),
            state: ObjectState::Alive,
            shape: ObjectShape::Sequence {
                element_type: crate::model::program::DataType::Integer,
                elements: vec![int(90), int(75), int(88)],
            },
        });
        let layout = layout_memory(&state, &LayoutConfig::default());
        assert_eq!(layout.objects[0].slots.len(), 3);
        let path = Path::Object {
            object: ObjectId(7),
            segments: vec![Segment::Index(2)],
        };
        assert_eq!(value_at(&state, &path), Some(&int(88)));
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::model::ids::{FunctionId, ScopeId};
    use crate::model::memory::{PrimitiveValue, StackVariable};
    use crate::model::scope::ScopeKind;

    fn state_with_one_variable() -> MemoryState {
        let mut state = MemoryState::empty();
        state.frames.push(StackFrame {
            id: FrameId(1),
            function: FunctionId(0),
            current_line: 1,
            variables: vec![StackVariable {
                name: "a".to_string(),
                value: Value::Primitive(PrimitiveValue::Integer(1)),
            }],
        });
        state
    }

    #[test]
    fn a_scope_band_covers_its_variables_inside_the_frame() {
        let mut state = state_with_one_variable();
        state
            .scopes
            .open_as(ScopeId(1), ScopeKind::Function, FrameId(1), 1)
            .expect("function scope");
        state.scopes.declare("a");

        let layout = layout_memory(&state, &LayoutConfig::default());
        assert_eq!(layout.scopes.len(), 1);
        let band = &layout.scopes[0];
        let frame = layout.frame(FrameId(1)).expect("frame");
        assert!(band.rect.x >= frame.rect.x);
        assert!(band.rect.y >= frame.header.y);
        assert!(band.rect.bottom() <= frame.rect.bottom());
        assert!(band.rect.height > 0.0);
        assert_eq!(
            layout.scope(ScopeId(1)).map(|scope| scope.kind),
            Some(ScopeKind::Function)
        );
    }

    #[test]
    fn a_nested_scope_is_inset_and_an_empty_one_stays_thin() {
        let mut state = state_with_one_variable();
        state
            .scopes
            .open_as(ScopeId(1), ScopeKind::Function, FrameId(1), 1)
            .expect("function scope");
        state.scopes.declare("a");
        let block = state
            .scopes
            .open_as(ScopeId(2), ScopeKind::Block, FrameId(1), 3)
            .expect("block scope");

        let layout = layout_memory(&state, &LayoutConfig::default());
        assert_eq!(layout.scopes.len(), 2);
        let outer = layout.scope(ScopeId(1)).expect("outer band");
        let inner = layout.scope(block).expect("inner band");
        assert_eq!(inner.depth, 1);
        assert!(inner.rect.x > outer.rect.x);
        assert!(inner.rect.width < outer.rect.width);
        let config = LayoutConfig::default();
        assert_eq!(inner.rect.height, config.slot_height / 2.0);
    }
}
