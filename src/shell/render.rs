use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect as EguiRect, Stroke, StrokeKind,
    Vec2,
};

use crate::model::events::BorrowKind;
use crate::model::ids::{FrameId, ObjectId, ScopeId};
use crate::model::memory::{Location, MemoryState, ObjectState, Path};
use crate::model::outcome::{Effect, Incident};
use crate::model::program::ProgramTree;
use crate::model::scope::ScopeKind;
use crate::shell::format::{cell_label, format_value, function_name, truncate};
use crate::shell::theme::{Palette, Theme};
use crate::stages::execution::FlowEdge;
use crate::stages::layout::{self, Connector, FrameBox, MemoryLayout, ObjectBox, SlotBox};

const BOX_CORNER: CornerRadius = CornerRadius::same(4);
const CELL_CORNER: CornerRadius = CornerRadius::same(2);
const TITLE_FONT: f32 = 12.5;
const CELL_FONT: f32 = 12.0;
const TITLE_CHARS: usize = 28;
const LABEL_CHARS: usize = 12;
const VALUE_CHARS: usize = 16;
const TEXT_PAD: f32 = 6.0;
const ARROW_WIDTH: f32 = 1.6;

/// Everything the renderer reads. Borrowed, never owned: a snapshot painted
/// twice must look exactly the same.
pub struct RenderInput<'a> {
    pub state: &'a MemoryState,
    pub layout: &'a MemoryLayout,
    pub program: &'a ProgramTree,
    pub theme: &'a Theme,
    /// Effects of the step that produced `state`. Empty at step 0 and after a
    /// rejected event; the renderer marks their targets in amber.
    pub effects: &'a [Effect],
    /// Frame the cursor is running. Its box gets the active outline.
    pub active_frame: Option<FrameId>,
    /// Innermost scope of the running frame, if the flow is inside a block.
    pub active_scope: Option<ScopeId>,
    /// Branch a conditional jump took, drawn next to the active frame.
    pub flow: Option<FlowEdge>,
}

/// Paints the whole memory with `origin` as the top-left corner of the layout
/// space. Returns the size the content occupies, which is what a scroll area
/// needs to know.
pub fn draw_memory(painter: &Painter, origin: Pos2, input: &RenderInput<'_>) -> Vec2 {
    let canvas = Canvas {
        painter,
        origin,
        input,
    };
    canvas.draw_connectors();
    canvas.draw_scopes();
    canvas.draw_frames();
    canvas.draw_loans();
    canvas.draw_objects();
    canvas.draw_flow();
    content_size(input.layout)
}

pub fn content_size(layout: &MemoryLayout) -> Vec2 {
    Vec2::new(layout.content.width, layout.content.height)
}

/// Incident pinned on this exact cell, if any. The renderer asks this once per
/// row instead of carrying an "is error" flag through the model.
pub fn cell_incident<'a>(state: &'a MemoryState, path: &Path) -> Option<&'a Incident> {
    state.incident_at(&Location::Cell(path.clone()))
}

/// Incident pinned on the frame as a whole: a write from a frame that is gone,
/// or any rejection that can only blame the box, not one of its rows.
pub fn frame_incident(state: &MemoryState, frame: FrameId) -> Option<&Incident> {
    state.incident_at(&Location::Frame(frame))
}

/// Incident pinned on an object as a whole. A runtime error that says "this
/// object is wrong" carries an empty segment list, which no cell address has.
pub fn object_incident(state: &MemoryState, object: ObjectId) -> Option<&Incident> {
    let path = Path::Object {
        object,
        segments: Vec::new(),
    };
    state.incident_at(&Location::Cell(path))
}

/// The step wrote this exact cell.
pub fn wrote_here(effects: &[Effect], path: &Path) -> bool {
    effects.iter().any(|effect| match effect {
        Effect::Wrote { path: written } => written == path,
        _ => false,
    })
}

/// The step dropped this exact cell.
pub fn dropped_here(effects: &[Effect], path: &Path) -> bool {
    effects.iter().any(|effect| match effect {
        Effect::Dropped { path: dropped } => dropped == path,
        _ => false,
    })
}

/// Some variable of this frame was dropped in this step. Used to fade the
/// header, because the row itself is already gone from the state.
pub fn dropped_in_frame(effects: &[Effect], frame: FrameId) -> bool {
    effects.iter().any(|effect| match effect {
        Effect::Dropped {
            path: Path::Stack { frame: dropped, .. },
        } => *dropped == frame,
        _ => false,
    })
}

/// The step pushed or popped this frame.
pub fn frame_touched(effects: &[Effect], frame: FrameId) -> bool {
    effects.iter().any(|effect| match effect {
        Effect::FramePushed { frame: touched } | Effect::FramePopped { frame: touched } => {
            *touched == frame
        }
        _ => false,
    })
}

/// The step created or released this object.
pub fn object_touched(effects: &[Effect], object: ObjectId) -> bool {
    effects.iter().any(|effect| match effect {
        Effect::CreatedObject { object: touched } | Effect::ReleasedObject { object: touched } => {
            *touched == object
        }
        _ => false,
    })
}

/// Layout-space point to screen point.
pub fn screen_pos(origin: Pos2, point: layout::Point) -> Pos2 {
    Pos2::new(origin.x + point.x, origin.y + point.y)
}

/// Layout-space rectangle to screen rectangle.
pub fn screen_rect(origin: Pos2, rect: layout::Rect) -> EguiRect {
    EguiRect::from_min_size(
        screen_pos(
            origin,
            layout::Point {
                x: rect.x,
                y: rect.y,
            },
        ),
        Vec2::new(rect.width, rect.height),
    )
}

struct Canvas<'a, 'b> {
    painter: &'a Painter,
    origin: Pos2,
    input: &'a RenderInput<'b>,
}

impl Canvas<'_, '_> {
    fn draw_connectors(&self) {
        let stroke = Stroke::new(ARROW_WIDTH, self.input.theme.palette.arrow);
        for connector in &self.input.layout.connectors {
            self.draw_connector(connector, stroke);
        }
    }

    fn draw_connector(&self, connector: &Connector, stroke: Stroke) {
        let from = screen_pos(self.origin, connector.from);
        let to = screen_pos(self.origin, connector.to);
        let vector = to - from;
        if vector.length() < 1.0 {
            return;
        }
        self.painter.arrow(from, vector, stroke);
    }

    /// Bands go under the frames: they group rows without hiding them, and the
    /// inner scope reads as an inset of the outer one. The active block gets a
    /// brighter outline, so the canvas shows where the flow is.
    fn draw_scopes(&self) {
        let palette = &self.input.theme.palette;
        for scope in &self.input.layout.scopes {
            let active = self.input.active_scope == Some(scope.scope);
            let fill = match active {
                true => Color32::from_rgba_unmultiplied(
                    palette.code_active_fill.r(),
                    palette.code_active_fill.g(),
                    palette.code_active_fill.b(),
                    90,
                ),
                false => Color32::from_rgba_unmultiplied(
                    palette.frame_header.r(),
                    palette.frame_header.g(),
                    palette.frame_header.b(),
                    70,
                ),
            };
            let stroke = match active {
                true => Stroke::new(2.0, palette.code_active_stroke),
                false => Stroke::new(1.0, palette.muted_text.gamma_multiply(0.7)),
            };
            let rect = screen_rect(self.origin, scope.rect);
            self.painter.rect_filled(rect, BOX_CORNER, fill);
            self.painter
                .rect_stroke(rect, BOX_CORNER, stroke, StrokeKind::Inside);
            self.draw_scope_label(scope.kind, rect);
        }
    }

    fn draw_scope_label(&self, kind: ScopeKind, rect: EguiRect) {
        let color = self.input.theme.palette.muted_text;
        let position = Pos2::new(rect.left() + TEXT_PAD, rect.top() + TEXT_PAD);
        let font = FontId::monospace(CELL_FONT);
        let _ = self.painter.text(
            position,
            Align2::LEFT_CENTER,
            scope_marker(kind),
            font,
            color,
        );
    }

    /// One arrow per live loan. Immutable loans are thin and blue; a mutable
    /// loan is thicker and amber, and says `&mut` on the wire.
    fn draw_loans(&self) {
        for loan in &self.input.state.loans {
            let (from, to) = match (
                self.input.layout.cell(&loan.borrower),
                self.input.layout.cell(&loan.target),
            ) {
                (Some(from), Some(to)) => (from, to),
                _ => continue,
            };
            let start = Pos2::new(
                screen_rect(self.origin, *from).right() + 2.0,
                screen_rect(self.origin, *from).center().y,
            );
            let end = Pos2::new(
                screen_rect(self.origin, *to).right() + 2.0,
                screen_rect(self.origin, *to).center().y,
            );
            let (stroke, marker) = match loan.kind {
                BorrowKind::Immutable => (Stroke::new(1.8, self.input.theme.palette.arrow), "&"),
                BorrowKind::Mutable => (
                    Stroke::new(2.6, self.input.theme.palette.effect_stroke),
                    "&mut",
                ),
            };
            self.painter.arrow(start, end - start, stroke);
            let middle = Pos2::new(start.x + 4.0, (start.y + end.y) / 2.0);
            let font = FontId::monospace(CELL_FONT);
            let _ = self
                .painter
                .text(middle, Align2::LEFT_CENTER, marker, font, stroke.color);
        }
    }

    fn draw_frames(&self) {
        for frame in &self.input.layout.frames {
            self.draw_frame(frame);
        }
    }

    fn draw_frame(&self, frame: &FrameBox) {
        let palette = &self.input.theme.palette;
        let incident = frame_incident(self.input.state, frame.frame).is_some();
        let active = !incident && self.input.active_frame == Some(frame.frame);
        let highlighted = !incident
            && (frame_touched(self.input.effects, frame.frame)
                || dropped_in_frame(self.input.effects, frame.frame));
        let fill = match incident {
            true => palette.alert_fill,
            false => palette.frame_fill,
        };
        let stroke = frame_outline(incident, active, highlighted, palette);
        let rect = screen_rect(self.origin, frame.rect);
        let header = screen_rect(self.origin, frame.header);
        self.painter.rect_filled(rect, BOX_CORNER, fill);
        self.painter
            .rect_stroke(rect, BOX_CORNER, stroke, StrokeKind::Inside);
        let header_fill = match (incident, active, highlighted) {
            (true, _, _) => palette.alert_fill,
            (false, true, _) => palette.code_active_fill,
            (false, false, true) => palette.effect_fill,
            (false, false, false) => palette.frame_header,
        };
        self.painter.rect_filled(header, BOX_CORNER, header_fill);
        let title_color = match (incident, active) {
            (true, _) => palette.alert_text,
            (false, true) => palette.title_text,
            (false, false) => accent(
                incident,
                highlighted,
                palette.title_text,
                palette.alert_text,
                palette.effect_text,
            ),
        };
        self.draw_title(&self.frame_title(frame), header, title_color);
        for slot in &frame.slots {
            self.draw_slot(slot, palette.cell_fill, palette.text);
        }
    }

    fn draw_objects(&self) {
        for object in &self.input.layout.objects {
            self.draw_object(object);
        }
    }

    /// Branch taken by a conditional jump, pinned next to the frame that ran it.
    /// The event carries line numbers, not coordinates, so the chip states the
    /// destination the same way the source panel does.
    fn draw_flow(&self) {
        let flow = match self.input.flow.as_ref() {
            Some(flow) => flow,
            None => return,
        };
        let frame = match self
            .input
            .active_frame
            .and_then(|frame| self.input.layout.frame(frame))
        {
            Some(frame) => frame,
            None => return,
        };
        let texts = &self.input.theme.texts;
        let palette = &self.input.theme.palette;
        let outcome = match flow.outcome {
            crate::model::events::ConditionOutcome::True => "true",
            crate::model::events::ConditionOutcome::False => "false",
        };
        let label = format!("{} {} ({})", texts.line_label, flow.to_line, outcome);
        let frame_rect = screen_rect(self.origin, frame.rect);
        let start = Pos2::new(frame_rect.right(), frame_rect.top() + 12.0);
        let chip_origin = Pos2::new(start.x + 10.0, start.y - 10.0);
        let width = label.chars().count() as f32 * 7.0 + 14.0;
        let chip = EguiRect::from_min_size(chip_origin, Vec2::new(width, 20.0));
        self.painter.arrow(
            start,
            Vec2::new(8.0, 0.0),
            Stroke::new(ARROW_WIDTH, palette.code_active_stroke),
        );
        self.painter
            .rect_filled(chip, CELL_CORNER, palette.effect_fill);
        self.painter.rect_stroke(
            chip,
            CELL_CORNER,
            Stroke::new(1.5, palette.effect_stroke),
            StrokeKind::Inside,
        );
        let position = Pos2::new(chip.left() + TEXT_PAD, chip.center().y);
        let _ = self.painter.text(
            position,
            Align2::LEFT_CENTER,
            label,
            FontId::monospace(CELL_FONT),
            palette.effect_text,
        );
    }

    fn draw_object(&self, object: &ObjectBox) {
        let palette = &self.input.theme.palette;
        let released = is_released(self.input.state, object.object);
        let incident = object_incident(self.input.state, object.object).is_some();
        let highlighted = !incident && object_touched(self.input.effects, object.object);
        let fill = released_fill(released, palette);
        let stroke = outline(incident, highlighted, palette);
        let rect = screen_rect(self.origin, object.rect);
        let header = screen_rect(self.origin, object.header);
        self.painter.rect_filled(rect, BOX_CORNER, fill);
        self.painter
            .rect_stroke(rect, BOX_CORNER, stroke, StrokeKind::Inside);
        let header_fill = object_header_fill(released, incident, highlighted, palette);
        self.painter.rect_filled(header, BOX_CORNER, header_fill);
        let text_color = match released {
            true => palette.muted_text,
            false => palette.title_text,
        };
        let title_color = accent(
            incident,
            highlighted,
            text_color,
            palette.alert_text,
            palette.effect_text,
        );
        self.draw_title(&self.object_title(object, released), header, title_color);
        for slot in &object.slots {
            let cell_fill = released_fill(released, palette);
            let text = match released {
                true => palette.muted_text,
                false => palette.text,
            };
            self.draw_slot(slot, cell_fill, text);
        }
    }

    fn draw_slot(&self, slot: &SlotBox, fill: Color32, text: Color32) {
        let palette = &self.input.theme.palette;
        let incident = cell_incident(self.input.state, &slot.path).is_some();
        let dropped = !incident && dropped_here(self.input.effects, &slot.path);
        let highlighted = !incident && !dropped && wrote_here(self.input.effects, &slot.path);
        let cell_fill = match (incident, dropped, highlighted) {
            (true, _, _) => palette.alert_fill,
            (false, true, _) => palette.released_fill,
            (false, false, true) => palette.effect_fill,
            (false, false, false) => fill,
        };
        let stroke = outline(incident, highlighted, palette);
        let text_color = match dropped {
            true => palette.muted_text,
            false => accent(
                incident,
                highlighted,
                text,
                palette.alert_text,
                palette.effect_text,
            ),
        };
        let rect = screen_rect(self.origin, slot.rect);
        self.painter.rect_filled(rect, CELL_CORNER, cell_fill);
        self.painter
            .rect_stroke(rect, CELL_CORNER, stroke, StrokeKind::Inside);
        self.draw_cell_label(slot, rect, text_color);
        self.draw_cell_value(slot, rect, text_color);
        if dropped {
            let y = rect.center().y;
            self.painter.line_segment(
                [
                    Pos2::new(rect.left() + TEXT_PAD, y),
                    Pos2::new(rect.right() - TEXT_PAD, y),
                ],
                Stroke::new(1.0, palette.muted_text),
            );
        }
    }

    fn draw_cell_label(&self, slot: &SlotBox, rect: EguiRect, color: Color32) {
        let label = truncate(&cell_label(&slot.path), LABEL_CHARS);
        let position = Pos2::new(rect.left() + TEXT_PAD, rect.center().y);
        let font = FontId::monospace(CELL_FONT);
        let _ = self
            .painter
            .text(position, Align2::LEFT_CENTER, label, font, color);
    }

    fn draw_cell_value(&self, slot: &SlotBox, rect: EguiRect, color: Color32) {
        let texts = &self.input.theme.texts;
        let value = match layout::value_at(self.input.state, &slot.path) {
            Some(value) => format_value(self.input.state, value, texts),
            None => texts.unknown_value.clone(),
        };
        let position = Pos2::new(rect.right() - TEXT_PAD, rect.center().y);
        let font = FontId::monospace(CELL_FONT);
        let _ = self.painter.text(
            position,
            Align2::RIGHT_CENTER,
            truncate(&value, VALUE_CHARS),
            font,
            color,
        );
    }

    fn draw_title(&self, title: &str, header: EguiRect, color: Color32) {
        let position = Pos2::new(header.left() + TEXT_PAD, header.center().y);
        let font = FontId::monospace(TITLE_FONT);
        let _ = self
            .painter
            .text(position, Align2::LEFT_CENTER, title, font, color);
    }

    fn frame_title(&self, frame: &FrameBox) -> String {
        let state_frame = self
            .input
            .state
            .frames
            .iter()
            .find(|candidate| candidate.id == frame.frame);
        let (name, line) = match state_frame {
            Some(state_frame) => (
                function_name(self.input.program, state_frame.function),
                state_frame.current_line,
            ),
            None => (format!("frame {}", frame.frame.0), 0),
        };
        let line_label = &self.input.theme.texts.line_label;
        truncate(
            &format!("fn {} · {} {}", name, line_label, line),
            TITLE_CHARS,
        )
    }

    fn object_title(&self, object: &ObjectBox, released: bool) -> String {
        let texts = &self.input.theme.texts;
        let label = match self.input.state.object(object.object) {
            Some(target) => target.label.clone(),
            None => texts.missing_object.clone(),
        };
        let base = format!("object {} · {}", object.object.0, label);
        let title = match released {
            true => format!("{} ({})", base, texts.released_object),
            false => base,
        };
        truncate(&title, TITLE_CHARS)
    }
}

/// Rust syntax tokens, not words: the band says which kind of scope it is
/// without needing a translation.
fn scope_marker(kind: ScopeKind) -> &'static str {
    match kind {
        ScopeKind::Function => "fn",
        ScopeKind::Block => "{",
        ScopeKind::Loop => "loop",
    }
}

fn is_released(state: &MemoryState, object: ObjectId) -> bool {
    match state.object(object) {
        Some(target) => target.state == ObjectState::Released,
        None => false,
    }
}

fn released_fill(released: bool, palette: &Palette) -> Color32 {
    match released {
        true => palette.released_fill,
        false => palette.object_fill,
    }
}

fn object_header_fill(
    released: bool,
    incident: bool,
    highlighted: bool,
    palette: &Palette,
) -> Color32 {
    match (incident, highlighted, released) {
        (true, _, _) => palette.alert_fill,
        (false, true, _) => palette.effect_fill,
        (false, false, true) => palette.released_header,
        (false, false, false) => palette.object_header,
    }
}

/// Incident wins over the active frame, which wins over the last effect: a red
/// box is never turned amber or blue by a highlight, and the cursor always wins
/// over a write that happened in the same step.
fn frame_outline(incident: bool, active: bool, highlighted: bool, palette: &Palette) -> Stroke {
    match (incident, active, highlighted) {
        (true, _, _) => Stroke::new(2.0, palette.alert_stroke),
        (false, true, _) => Stroke::new(2.5, palette.code_active_stroke),
        (false, false, true) => Stroke::new(2.0, palette.effect_stroke),
        (false, false, false) => Stroke::new(1.0, palette.stroke),
    }
}

/// Incident wins over the last effect: a red box is never turned amber by a
/// highlight, and an amber cell never hides a failure.
fn outline(incident: bool, highlighted: bool, palette: &Palette) -> Stroke {
    match (incident, highlighted) {
        (true, _) => Stroke::new(2.0, palette.alert_stroke),
        (false, true) => Stroke::new(2.0, palette.effect_stroke),
        (false, false) => Stroke::new(1.0, palette.stroke),
    }
}

fn accent(
    incident: bool,
    highlighted: bool,
    normal: Color32,
    alert: Color32,
    effect: Color32,
) -> Color32 {
    match (incident, highlighted) {
        (true, _) => alert,
        (false, true) => effect,
        (false, false) => normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ids::Sequence;
    use crate::model::memory::Segment;
    use crate::model::outcome::RejectionReason;

    fn invalid(location: Location) -> Incident {
        Incident::InvalidEvent {
            sequence: Sequence(2),
            location,
            reason: RejectionReason::EmptyStack,
        }
    }

    fn field_path(object: u32) -> Path {
        Path::Object {
            object: ObjectId(object),
            segments: vec![Segment::Field("x".to_string())],
        }
    }

    #[test]
    fn a_cell_incident_marks_that_cell_only() {
        let mut state = MemoryState::empty();
        state.incidents.push(invalid(Location::Cell(field_path(1))));
        let other = Path::Object {
            object: ObjectId(1),
            segments: vec![Segment::Field("y".to_string())],
        };
        assert!(cell_incident(&state, &field_path(1)).is_some());
        assert!(cell_incident(&state, &other).is_none());
    }

    #[test]
    fn a_whole_object_incident_does_not_redden_every_field() {
        let mut state = MemoryState::empty();
        let whole = Path::Object {
            object: ObjectId(2),
            segments: Vec::new(),
        };
        state.incidents.push(invalid(Location::Cell(whole)));
        assert!(object_incident(&state, ObjectId(2)).is_some());
        assert!(cell_incident(&state, &field_path(2)).is_none());
        assert!(object_incident(&state, ObjectId(3)).is_none());
    }

    #[test]
    fn a_frame_incident_is_not_a_cell_incident() {
        let mut state = MemoryState::empty();
        state.incidents.push(invalid(Location::Frame(FrameId(7))));
        assert!(frame_incident(&state, FrameId(7)).is_some());
        assert!(frame_incident(&state, FrameId(8)).is_none());
        assert!(cell_incident(&state, &field_path(7)).is_none());
    }

    #[test]
    fn a_write_marks_exactly_the_written_cell() {
        let effects = vec![Effect::Wrote {
            path: field_path(1),
        }];
        assert!(wrote_here(&effects, &field_path(1)));
        assert!(!wrote_here(&effects, &field_path(2)));
    }

    #[test]
    fn pushing_or_popping_marks_the_frame() {
        let pushed = vec![Effect::FramePushed { frame: FrameId(3) }];
        let popped = vec![Effect::FramePopped { frame: FrameId(3) }];
        assert!(frame_touched(&pushed, FrameId(3)));
        assert!(frame_touched(&popped, FrameId(3)));
        assert!(!frame_touched(&pushed, FrameId(4)));
    }

    #[test]
    fn creating_or_releasing_marks_the_object() {
        let created = vec![Effect::CreatedObject {
            object: ObjectId(2),
        }];
        let released = vec![Effect::ReleasedObject {
            object: ObjectId(2),
        }];
        assert!(object_touched(&created, ObjectId(2)));
        assert!(object_touched(&released, ObjectId(2)));
        assert!(!object_touched(&created, ObjectId(1)));
    }

    #[test]
    fn an_incident_always_wins_over_the_last_effect() {
        let palette = Palette::dark();
        let stroke = outline(true, true, &palette);
        assert_eq!(stroke.color, palette.alert_stroke);
        let color = accent(
            true,
            true,
            palette.text,
            palette.alert_text,
            palette.effect_text,
        );
        assert_eq!(color, palette.alert_text);
        assert_eq!(outline(false, true, &palette).color, palette.effect_stroke);
    }

    #[test]
    fn points_and_rectangles_move_with_the_origin() {
        let origin = Pos2::new(100.0, 40.0);
        let point = screen_pos(origin, layout::Point { x: 24.0, y: 8.0 });
        assert_eq!(point, Pos2::new(124.0, 48.0));
        let rect = screen_rect(
            origin,
            layout::Rect {
                x: 24.0,
                y: 8.0,
                width: 240.0,
                height: 28.0,
            },
        );
        assert_eq!(rect.min, Pos2::new(124.0, 48.0));
        assert_eq!(rect.max, Pos2::new(364.0, 76.0));
    }

    #[test]
    fn the_content_size_is_the_layout_size() {
        let layout = MemoryLayout {
            frames: Vec::new(),
            objects: Vec::new(),
            connectors: Vec::new(),
            scopes: Vec::new(),
            content: layout::Size {
                width: 580.0,
                height: 232.0,
            },
        };
        assert_eq!(content_size(&layout), Vec2::new(580.0, 232.0));
    }
}
