use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Ui, Vec2,
};

use crate::edge::ui::message::UiMessage;
use crate::model::events::TraceEvent;
use crate::model::memory::MemoryState;
use crate::model::program::ProgramTree;
use crate::shell::theme::Theme;

const ROW_HEIGHT: f32 = 18.0;
const CODE_FONT: f32 = 12.0;
const NUMBER_RIGHT: f32 = 30.0;
const CODE_LEFT: f32 = 42.0;
const ACCENT_WIDTH: f32 = 3.0;
const ACCENT_INSET: f32 = 1.0;

/// One source line that appears in the trace, and the step that reached it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LineAnchor {
    pub line: u32,
    pub step: usize,
}

pub struct CodePanelInput<'a> {
    pub events: &'a [TraceEvent],
    pub program: &'a ProgramTree,
    pub active_line: Option<u32>,
    pub theme: &'a Theme,
    /// Ask the scroll area to follow the active row in this frame.
    pub follow: bool,
    /// The editor replaces the highlighted rows while this is set.
    pub editing: bool,
    /// Last recording error, already formatted for the header.
    pub error: Option<&'a str>,
    /// Focus the editor on this frame, right after `Edit` is pressed.
    pub focus: bool,
}

/// Line the trace is on after `step` events. `0` is before the first event.
pub fn active_line(events: &[TraceEvent], state: Option<&MemoryState>, step: usize) -> Option<u32> {
    if step == 0 {
        return None;
    }
    let event = events.get(step - 1)?;
    match event.active_line() {
        Some(line) => Some(line),
        None => state.and_then(|state| state.current_line()),
    }
}

/// Lines recorded by the trace, in chronological order.
///
/// A call contributes both the callee's first line (where execution goes) and
/// the caller's line (the instruction that made the call). A jump contributes
/// both ends for the same reason. Events without a line, such as returns and
/// releases, add nothing; `step_for_line` still maps those lines by proximity.
pub fn line_anchors(events: &[TraceEvent], program: &ProgramTree) -> Vec<LineAnchor> {
    let mut anchors = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let step = index + 1;
        match event {
            TraceEvent::StartFunction {
                function,
                call_line,
                ..
            } => {
                if let Some(definition) = program.function(*function) {
                    anchors.push(LineAnchor {
                        line: definition.start_line,
                        step,
                    });
                }
                anchors.push(LineAnchor {
                    line: *call_line,
                    step,
                });
            }
            TraceEvent::ConditionalJump {
                from_line, to_line, ..
            } => {
                anchors.push(LineAnchor {
                    line: *from_line,
                    step,
                });
                anchors.push(LineAnchor {
                    line: *to_line,
                    step,
                });
            }
            _ => {
                if let Some(line) = event.active_line() {
                    anchors.push(LineAnchor { line, step });
                }
            }
        }
    }
    anchors
}

/// Step a click on `line` should show.
///
/// An exact hit selects the last event recorded on that line, so the panel
/// shows the memory after the line finished. A line with no event keeps the
/// most recent step at or before it; lines before the first anchor start at the
/// first step.
pub fn step_for_line(anchors: &[LineAnchor], line: u32) -> Option<usize> {
    let mut exact = None;
    let mut before = None;
    for anchor in anchors {
        if anchor.line == line {
            exact = Some(anchor.step);
        }
        if anchor.line <= line {
            before = Some(anchor.step);
        }
    }
    exact
        .or(before)
        .or_else(|| anchors.first().map(|anchor| anchor.step))
}

pub fn code_panel(
    ui: &mut Ui,
    input: &CodePanelInput<'_>,
    draft: &mut String,
) -> Option<UiMessage> {
    let mut message = header(ui, input);
    ui.separator();
    match input.editing {
        true => editor(ui, draft, input.focus, &mut message),
        false => lines(ui, input, draft, &mut message),
    }
    message
}

/// Title, mode switch and the two recording actions.
fn header(ui: &mut Ui, input: &CodePanelInput<'_>) -> Option<UiMessage> {
    let texts = &input.theme.texts;
    let mut message = None;
    ui.horizontal_wrapped(|ui| {
        ui.heading(&texts.code_title);
        let toggle = match input.editing {
            true => &texts.view_label,
            false => &texts.edit_label,
        };
        if ui.button(toggle).clicked() {
            message = Some(UiMessage::ToggleEdit);
        }
        if ui.button(&texts.run_label).clicked() {
            message = Some(UiMessage::Record);
        }
        if ui.button(&texts.open_label).clicked() {
            message = Some(UiMessage::OpenFile);
        }
    });
    let hint = match input.editing {
        true => &texts.recorded_label,
        false => &texts.code_hint,
    };
    let hint = RichText::new(hint)
        .small()
        .color(input.theme.palette.muted_text);
    ui.label(hint);
    if let Some(error) = input.error {
        ui.colored_label(input.theme.palette.alert_stroke, error);
    }
    message
}

fn lines(ui: &mut Ui, input: &CodePanelInput<'_>, draft: &str, message: &mut Option<UiMessage>) {
    let anchors = line_anchors(input.events, input.program);
    let digits = draft.lines().count().to_string().len();
    ScrollArea::vertical()
        .id_salt("code_lines")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (index, text) in draft.lines().enumerate() {
                let number = index as u32 + 1;
                let step = step_for_line(&anchors, number);
                if let Some(clicked) = source_row(ui, input, number, text, digits, step) {
                    *message = Some(clicked);
                }
            }
        });
}

fn editor(ui: &mut Ui, draft: &mut String, focus: bool, message: &mut Option<UiMessage>) {
    let rows = draft.lines().count().max(16);
    ScrollArea::vertical()
        .id_salt("code_editor")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let id = ui.make_persistent_id("source_editor");
            let edit = egui::TextEdit::multiline(draft)
                .id(id)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(rows);
            let response = ui.add(edit);
            if focus {
                response.request_focus();
            }
            if response.changed() {
                *message = Some(UiMessage::Record);
            }
        });
}

fn source_row(
    ui: &mut Ui,
    input: &CodePanelInput<'_>,
    number: u32,
    text: &str,
    digits: usize,
    step: Option<usize>,
) -> Option<UiMessage> {
    let palette = &input.theme.palette;
    let active = input.active_line == Some(number);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let hovered = response.hovered() && !active;
    let fill = match (active, hovered) {
        (true, _) => Some(palette.code_active_fill),
        (false, true) => Some(palette.frame_header),
        (false, false) => None,
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 0.0, fill);
    }
    if active {
        paint_accent(ui, rect, palette.code_active_stroke);
    }
    paint_number(ui, rect, number, digits, active, palette);
    paint_code(ui, rect, text, active, palette);
    if active && input.follow {
        response.scroll_to_me(Some(Align::Center));
    }
    match response.clicked() {
        true => step.map(UiMessage::SetStep),
        false => None,
    }
}

fn paint_accent(ui: &Ui, rect: Rect, color: Color32) {
    let height = rect.height() - ACCENT_INSET * 2.0;
    let min = Pos2::new(rect.left(), rect.top() + ACCENT_INSET);
    let bar = Rect::from_min_size(min, Vec2::new(ACCENT_WIDTH, height));
    ui.painter().rect_filled(bar, 0.0, color);
}

fn paint_number(
    ui: &Ui,
    rect: Rect,
    number: u32,
    digits: usize,
    active: bool,
    palette: &crate::shell::theme::Palette,
) {
    let text = format!("{:>width$}", number, width = digits);
    let position = Pos2::new(rect.left() + NUMBER_RIGHT, rect.center().y);
    let color = match active {
        true => palette.code_active_stroke,
        false => palette.muted_text,
    };
    let font = FontId::monospace(CODE_FONT);
    let _ = ui
        .painter()
        .text(position, Align2::RIGHT_CENTER, text, font, color);
}

fn paint_code(
    ui: &Ui,
    rect: Rect,
    text: &str,
    active: bool,
    palette: &crate::shell::theme::Palette,
) {
    let position = Pos2::new(rect.left() + CODE_LEFT, rect.center().y);
    let color = match active {
        true => palette.title_text,
        false => palette.text,
    };
    let font = FontId::monospace(CODE_FONT);
    let _ = ui
        .painter()
        .text(position, Align2::LEFT_CENTER, text, font, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::sample::{SAMPLE_SOURCE, sample_trace};
    use crate::stages::analyze::{Analysis, analyze_program};
    use crate::stages::replay::{Timeline, build_timeline};

    fn demo() -> (ProgramTree, Timeline) {
        let program = match analyze_program(SAMPLE_SOURCE) {
            Analysis::Analyzed(tree) => tree,
            Analysis::Failed(error) => panic!("the sample must analyse: {:?}", error),
        };
        let events = sample_trace(&program);
        let timeline = build_timeline(&program, &events);
        (program, timeline)
    }

    fn events() -> Vec<TraceEvent> {
        let program = demo().0;
        sample_trace(&program)
    }

    #[test]
    fn before_the_first_event_there_is_no_active_line() {
        let (_, timeline) = demo();
        let events = events();
        let state = timeline.state_at(0);
        assert_eq!(active_line(&events, state, 0), None);
    }

    #[test]
    fn a_call_shows_the_callee_first_line() {
        let (_, timeline) = demo();
        let events = events();
        let state = timeline.state_at(2);
        assert_eq!(active_line(&events, state, 2), Some(6));
    }

    #[test]
    fn a_return_falls_back_to_the_caller_line() {
        let (_, timeline) = demo();
        let events = events();
        let state = timeline.state_at(5);
        assert_eq!(active_line(&events, state, 5), Some(18));
    }

    #[test]
    fn a_write_and_a_jump_report_their_own_line() {
        let (_, timeline) = demo();
        let events = events();
        assert_eq!(active_line(&events, timeline.state_at(3), 3), Some(7));
        assert_eq!(active_line(&events, timeline.state_at(12), 12), Some(14));
    }

    #[test]
    fn clicking_a_line_chooses_the_last_event_on_it() {
        let (program, _) = demo();
        let anchors = line_anchors(&events(), &program);
        assert_eq!(step_for_line(&anchors, 6), Some(2));
        assert_eq!(step_for_line(&anchors, 18), Some(6));
        assert_eq!(step_for_line(&anchors, 21), Some(18));
    }

    #[test]
    fn a_line_without_events_keeps_the_previous_step() {
        let (program, _) = demo();
        let anchors = line_anchors(&events(), &program);
        assert_eq!(step_for_line(&anchors, 8), Some(4));
        assert_eq!(step_for_line(&anchors, 22), Some(18));
    }

    #[test]
    fn every_source_line_maps_to_a_step() {
        let (program, _) = demo();
        let anchors = line_anchors(&events(), &program);
        let count = SAMPLE_SOURCE.lines().count() as u32;
        for line in 1..=count {
            assert!(step_for_line(&anchors, line).is_some(), "line {line}");
        }
        assert_eq!(step_for_line(&anchors, 1), Some(1));
    }
}
