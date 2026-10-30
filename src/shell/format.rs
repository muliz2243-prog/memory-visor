use crate::model::ids::FunctionId;
use crate::model::memory::{MemoryState, ObjectState, Path, PrimitiveValue, Segment, Value};
use crate::model::program::ProgramTree;
use crate::shell::theme::Texts;

/// Value as the panel prints it. References show the object label when the
/// object still exists, and the reason when it does not.
pub fn format_value(state: &MemoryState, value: &Value, texts: &Texts) -> String {
    match value {
        Value::Primitive(primitive) => format_primitive(primitive),
        Value::Reference(object) => match state.object(*object) {
            Some(target) => match target.state {
                ObjectState::Alive => format!("&{} {}", object.0, target.label),
                ObjectState::Released => {
                    format!("&{} ({})", object.0, texts.released_object)
                }
            },
            None => format!("&{} ({})", object.0, texts.missing_object),
        },
    }
}

pub fn format_primitive(primitive: &PrimitiveValue) -> String {
    match primitive {
        PrimitiveValue::Integer(number) => format!("{}", number),
        PrimitiveValue::Float(number) => format!("{:.2}", number),
        PrimitiveValue::Boolean(flag) => format!("{}", flag),
        PrimitiveValue::Character(character) => format!("'{}'", character),
        PrimitiveValue::Text(text) => format!("\"{}\"", text),
    }
}

/// Label inside a cell: the variable name, or the field / index of an object.
pub fn cell_label(path: &Path) -> String {
    match path {
        Path::Stack { variable, .. } => variable.clone(),
        Path::Object { segments, .. } => match segments.as_slice() {
            [Segment::Field(name)] => format!(".{}", name),
            [Segment::Index(index)] => format!("[{}]", index),
            _ => "?".to_string(),
        },
    }
}

/// Full address, for the trace panel and the CLI.
pub fn format_path(path: &Path) -> String {
    match path {
        Path::Stack { frame, variable } => format!("frame {}.{}", frame.0, variable),
        Path::Object { object, segments } => {
            let mut text = format!("object {}", object.0);
            for segment in segments {
                match segment {
                    Segment::Field(name) => {
                        text.push('.');
                        text.push_str(name);
                    }
                    Segment::Index(index) => {
                        text.push('[');
                        text.push_str(&index.to_string());
                        text.push(']');
                    }
                }
            }
            text
        }
    }
}

pub fn function_name(program: &ProgramTree, function: FunctionId) -> String {
    match program.function(function) {
        Some(definition) => definition.name.clone(),
        None => format!("#{}", function.0),
    }
}

/// Cuts text to `max_chars` without splitting a character, with an ellipsis.
pub fn truncate(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    match total <= max_chars {
        true => text.to_string(),
        false => {
            let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
            cut.push('…');
            cut
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ids::{ObjectId, StructId};
    use crate::model::memory::{Field, HeapObject, ObjectShape};
    use crate::shell::theme::Texts;

    fn texts() -> Texts {
        Texts::english()
    }

    fn state_with_point() -> MemoryState {
        let mut state = MemoryState::empty();
        state.heap.push(HeapObject {
            id: ObjectId(1),
            label: "Point#1".to_string(),
            state: ObjectState::Alive,
            shape: ObjectShape::Structure {
                structure: StructId(0),
                fields: vec![Field {
                    name: "x".to_string(),
                    value: Value::Primitive(PrimitiveValue::Integer(5)),
                }],
            },
        });
        state
    }

    #[test]
    fn primitives_read_like_source_code() {
        assert_eq!(format_primitive(&PrimitiveValue::Integer(253)), "253");
        assert_eq!(format_primitive(&PrimitiveValue::Float(1.5)), "1.50");
        assert_eq!(format_primitive(&PrimitiveValue::Boolean(true)), "true");
        assert_eq!(format_primitive(&PrimitiveValue::Character('a')), "'a'");
        assert_eq!(
            format_primitive(&PrimitiveValue::Text("hola".to_string())),
            "\"hola\""
        );
    }

    #[test]
    fn a_reference_shows_the_object_label() {
        let state = state_with_point();
        assert_eq!(
            format_value(&state, &Value::Reference(ObjectId(1)), &texts()),
            "&1 Point#1"
        );
    }

    #[test]
    fn a_released_object_says_so() {
        let mut state = state_with_point();
        state.heap[0].state = ObjectState::Released;
        assert_eq!(
            format_value(&state, &Value::Reference(ObjectId(1)), &texts()),
            "&1 (released)"
        );
    }

    #[test]
    fn a_missing_object_says_so() {
        assert_eq!(
            format_value(
                &MemoryState::empty(),
                &Value::Reference(ObjectId(9)),
                &texts()
            ),
            "&9 (no object)"
        );
    }

    #[test]
    fn a_cell_label_drops_the_address() {
        let stack = Path::Stack {
            frame: crate::model::ids::FrameId(1),
            variable: "total".to_string(),
        };
        assert_eq!(cell_label(&stack), "total");
        let field = Path::Object {
            object: ObjectId(1),
            segments: vec![Segment::Field("x".to_string())],
        };
        assert_eq!(cell_label(&field), ".x");
        let element = Path::Object {
            object: ObjectId(2),
            segments: vec![Segment::Index(0)],
        };
        assert_eq!(cell_label(&element), "[0]");
    }

    #[test]
    fn a_path_names_its_column() {
        let element = Path::Object {
            object: ObjectId(2),
            segments: vec![Segment::Index(3)],
        };
        assert_eq!(format_path(&element), "object 2[3]");
    }

    #[test]
    fn truncation_keeps_whole_characters() {
        assert_eq!(truncate("short", 8), "short");
        assert_eq!(truncate("abcdefgh", 5), "abcd…");
        assert_eq!(truncate("naïve", 2), "n…");
    }
}
