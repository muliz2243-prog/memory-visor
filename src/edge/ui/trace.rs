use egui::{Align, Button, Color32, RichText, ScrollArea, TextWrapMode, Ui};

use crate::edge::ui::message::UiMessage;
use crate::model::events::TraceEvent;
use crate::model::outcome::Incident;
use crate::model::program::ProgramTree;
use crate::shell::narrative::{describe_event, describe_step};
use crate::shell::theme::Theme;
use crate::stages::replay::{Timeline, TimelineStep};

const ROW_FONT: f32 = 12.0;
const TAIL_INDENT: &str = "   ";

pub struct TracePanelInput<'a> {
    pub events: &'a [TraceEvent],
    pub timeline: &'a Timeline,
    pub program: &'a ProgramTree,
    pub theme: &'a Theme,
    /// Number of events already applied. `0` is the empty memory.
    pub current: usize,
    /// Ask the scroll area to follow the current row in this frame.
    pub follow: bool,
}

pub fn trace_panel(ui: &mut Ui, input: &TracePanelInput<'_>) -> Option<UiMessage> {
    let mut message = None;
    ui.heading(&input.theme.texts.trace_title);
    let hint = RichText::new(&input.theme.texts.trace_hint)
        .small()
        .color(input.theme.palette.muted_text);
    ui.label(hint);
    ui.separator();
    ScrollArea::vertical()
        .id_salt("trace_rows")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(TextWrapMode::Wrap);
            message = start_row(ui, input);
            for (index, event) in input.events.iter().enumerate() {
                let clicked = event_row(ui, input, index, event);
                if clicked.is_some() {
                    message = clicked;
                }
            }
        });
    message
}

fn start_row(ui: &mut Ui, input: &TracePanelInput<'_>) -> Option<UiMessage> {
    let selected = input.current == 0;
    let text = RichText::new(&input.theme.texts.start_label)
        .monospace()
        .size(ROW_FONT)
        .color(row_color(None, selected, input.theme));
    let response = ui.add(Button::selectable(selected, text).wrap());
    follow(&response, selected, input.follow);
    match response.clicked() {
        true => Some(UiMessage::SetStep(0)),
        false => None,
    }
}

fn event_row(
    ui: &mut Ui,
    input: &TracePanelInput<'_>,
    index: usize,
    event: &TraceEvent,
) -> Option<UiMessage> {
    let step = input.timeline.steps.get(index);
    let selected = input.current == index + 1;
    let incident = step.and_then(|step| step.incident.as_ref());
    let text = RichText::new(row_text(event, step, input.program))
        .monospace()
        .size(ROW_FONT)
        .color(row_color(incident, selected, input.theme));
    let response = ui.add(Button::selectable(selected, text).wrap());
    follow(&response, selected, input.follow);
    match response.clicked() {
        true => Some(UiMessage::SetStep(index + 1)),
        false => None,
    }
}

fn follow(response: &egui::Response, selected: bool, follow: bool) {
    if selected && follow {
        response.scroll_to_me(Some(Align::Center));
    }
}

fn row_text(event: &TraceEvent, step: Option<&TimelineStep>, program: &ProgramTree) -> String {
    let head = format!(
        "[{}] {}",
        event.sequence().0,
        describe_event(event, program)
    );
    let tail = match step {
        Some(step) => describe_step(&step.effects, step.incident.as_ref()),
        None => String::new(),
    };
    match tail.is_empty() {
        true => head,
        false => format!("{}\n{}{}", head, TAIL_INDENT, tail),
    }
}

fn row_color(incident: Option<&Incident>, selected: bool, theme: &Theme) -> Color32 {
    match (incident, selected) {
        (Some(_), _) => theme.palette.alert_stroke,
        (None, true) => theme.palette.effect_text,
        (None, false) => theme.palette.text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ids::{FrameId, Sequence};
    use crate::model::memory::{Path, PrimitiveValue, Value};
    use crate::model::outcome::Effect;
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
    fn a_row_shows_sequence_event_and_effect() {
        let event = TraceEvent::DeclareVariable {
            sequence: Sequence(1),
            line: 2,
            name: "total".to_string(),
            value: Value::Primitive(PrimitiveValue::Integer(1)),
        };
        let step = TimelineStep {
            effects: vec![Effect::Wrote { path: cell() }],
            incident: None,
        };
        let text = row_text(&event, Some(&step), &program());
        assert!(text.starts_with("[1] declare total"));
        assert!(text.contains("wrote frame 1.total"));
    }

    #[test]
    fn a_row_without_a_recorded_step_keeps_the_event() {
        let event = TraceEvent::ReleaseObject {
            sequence: Sequence(9),
            object: crate::model::ids::ObjectId(1),
        };
        let text = row_text(&event, None, &program());
        assert_eq!(text, "[9] release object 1");
    }
}
