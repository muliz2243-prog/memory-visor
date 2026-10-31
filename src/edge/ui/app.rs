use std::time::Duration;

use egui::Sense;

use crate::edge::player::{Player, PlayerPlan, Tick};
use crate::edge::recorder::{RecordError, record};
use crate::edge::sample::{LIFECYCLE_SOURCE, lifecycle_trace};
use crate::edge::ui::code::{CodePanelInput, active_line, code_panel};
use crate::edge::ui::message::UiMessage;
use crate::edge::ui::trace::{TracePanelInput, trace_panel};
use crate::model::events::TraceEvent;
use crate::model::memory::MemoryState;
use crate::model::outcome::Effect;
use crate::model::program::ProgramTree;
use crate::shell::format::truncate;
use crate::shell::render::{RenderInput, content_size, draw_memory};
use crate::shell::theme::Theme;
use crate::stages::analyze::{Analysis, analyze_program};
use crate::stages::execution::{ExecutionStep, execution_step};
use crate::stages::layout::{LayoutConfig, MemoryLayout, layout_memory};
use crate::stages::replay::{Timeline, build_timeline};

/// Maximum characters of an incident shown in the toolbar, so a long message
/// never pushes the canvas down the screen.
const INCIDENT_CHARS: usize = 120;

/// While playing, ask egui for a frame this often. The player accumulates the
/// deltas, so this is a polling rate, not the step duration (`Player::step_ms`).
const FRAME_MS: u64 = 50;

pub struct VisorApp {
    source: String,
    program: ProgramTree,
    events: Vec<TraceEvent>,
    timeline: Timeline,
    theme: Theme,
    config: LayoutConfig,
    layout: MemoryLayout,
    player: Player,
    /// The source panel shows the editor instead of the highlighted rows.
    editing: bool,
    /// Last recording failure, kept while the previous trace stays on screen.
    error: Option<RecordError>,
    /// Focus the editor on the next frame after `Edit` is pressed.
    focus_editor: bool,
    /// Ask the code and trace panels to scroll to the cursor on this frame.
    /// Set when the cursor moves and taken once, before drawing the panels.
    follow_cursor: bool,
}

impl VisorApp {
    pub fn new(program: ProgramTree, events: Vec<TraceEvent>, source: &str) -> Self {
        let config = LayoutConfig::default();
        let timeline = build_timeline(&program, &events);
        let mut app = VisorApp {
            source: source.to_string(),
            program,
            events,
            timeline,
            theme: Theme::english(),
            config,
            layout: layout_memory(&MemoryState::empty(), &config),
            player: Player::new(),
            editing: false,
            error: None,
            focus_editor: false,
            follow_cursor: false,
        };
        app.rebuild_layout();
        app
    }

    /// The only place where a UI interaction becomes a state change.
    pub fn update(&mut self, message: UiMessage) {
        match message {
            UiMessage::NextStep => self.set_step(self.player.index.saturating_add(1)),
            UiMessage::PrevStep => self.set_step(self.player.index.saturating_sub(1)),
            UiMessage::SetStep(step) => self.set_step(step),
            UiMessage::TogglePlay => self.toggle_play(),
            UiMessage::Tick(tick) => self.tick(tick),
            UiMessage::ToggleEdit => {
                self.editing = !self.editing;
                self.focus_editor = self.editing;
            }
            UiMessage::Record => {
                self.apply_source();
            }
            UiMessage::OpenFile => self.open_file(),
        }
    }

    /// Records the buffer. On success the window replays the new trace from the
    /// start; on failure the previous trace and cursor stay untouched.
    pub fn apply_source(&mut self) {
        match record(&self.source) {
            Ok(recording) => {
                self.program = recording.program;
                self.events = recording.events;
                self.timeline = build_timeline(&self.program, &self.events);
                self.player = Player::new();
                self.error = None;
                self.follow_cursor = false;
                self.rebuild_layout();
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn open_file(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Rust", &["rs"])
            .pick_file();
        let path = match picked {
            Some(path) => path,
            None => return,
        };
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                self.source = source;
                self.editing = true;
                self.apply_source();
            }
            Err(error) => {
                self.error = Some(RecordError::new(
                    0,
                    format!("cannot read {}: {}", path.display(), error),
                ));
            }
        }
    }

    /// One line for the source header: the line only when the error has one.
    fn error_text(&self) -> Option<String> {
        self.error.as_ref().map(|error| match error.line {
            0 => error.detail.clone(),
            line => format!("line {}: {}", line, error.detail),
        })
    }

    /// Moves the cursor and recomputes the geometry for the new snapshot.
    pub fn set_step(&mut self, step: usize) {
        let clamped = step.min(self.timeline.len());
        if clamped == self.player.index {
            return;
        }
        self.player = self.player.at(clamped);
        self.follow_cursor = true;
        self.rebuild_layout();
    }

    pub fn step(&self) -> usize {
        self.player.index
    }

    /// Line the trace is on at the current cursor, if any.
    pub fn active_source_line(&self) -> Option<u32> {
        let state = self.timeline.state_at(self.player.index);
        active_line(&self.events, state, self.player.index)
    }

    /// Effects of the event that produced the current snapshot. Empty at step 0
    /// and after a rejected event, because a rejection changes nothing.
    pub fn current_effects(&self) -> &[Effect] {
        match self.player.index {
            0 => &[],
            index => match self.timeline.steps.get(index - 1) {
                Some(step) => step.effects.as_slice(),
                None => &[],
            },
        }
    }

    pub fn playing(&self) -> bool {
        !self.player.paused
    }

    pub fn layout(&self) -> &MemoryLayout {
        &self.layout
    }

    /// Read-only view of the step the cursor is on. `None` past the trace.
    pub fn active_step(&self) -> Option<ExecutionStep<'_>> {
        execution_step(&self.events, &self.timeline, self.player.index)
    }

    fn toggle_play(&mut self) {
        if self.playing() {
            self.player = self.player.paused();
            return;
        }
        if self.player.index >= self.timeline.len() {
            self.set_step(0);
        }
        self.player = self.player.running();
    }

    fn tick(&mut self, tick: Tick) {
        let plan = self.player.plan(tick, self.timeline.len());
        match plan {
            PlayerPlan::Finished => {
                self.player = self.player.paused();
            }
            PlayerPlan::Wait { .. } => {
                self.player = self.player.apply(plan);
            }
            PlayerPlan::Advance { .. } => {
                self.player = self.player.apply(plan);
                self.rebuild_layout();
            }
        }
    }

    fn rebuild_layout(&mut self) {
        let layout = match self.timeline.state_at(self.player.index) {
            Some(state) => layout_memory(state, &self.config),
            None => return,
        };
        self.layout = layout;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let length = self.timeline.len();
        let playing = self.playing();
        let play_label = match playing {
            true => self.theme.texts.pause_label.clone(),
            false => self.theme.texts.play_label.clone(),
        };
        let step_label = self.theme.texts.step_label.clone();
        let incident_label = self.theme.texts.incident_label.clone();
        let mut message = None;
        ui.horizontal(|ui| {
            if ui.button("|<").clicked() {
                message = Some(UiMessage::SetStep(0));
            }
            if ui.button("<").clicked() {
                message = Some(UiMessage::PrevStep);
            }
            if ui.button(play_label).clicked() {
                message = Some(UiMessage::TogglePlay);
            }
            if ui.button(">").clicked() {
                message = Some(UiMessage::NextStep);
            }
            if ui.button(">|").clicked() {
                message = Some(UiMessage::SetStep(length));
            }
            let mut requested = self.player.index;
            let slider = egui::Slider::new(&mut requested, 0..=length).show_value(false);
            if ui.add(slider).changed() {
                message = Some(UiMessage::SetStep(requested));
            }
            ui.label(format!("{} {} / {}", step_label, self.player.index, length));
        });
        if let Some(message) = message {
            self.update(message);
            ui.ctx().request_repaint();
        }
        self.incident_line(ui, &incident_label);
    }

    fn incident_line(&self, ui: &mut egui::Ui, label: &str) {
        let state = match self.timeline.state_at(self.player.index) {
            Some(state) => state,
            None => return,
        };
        let incident = match state.incidents.last() {
            Some(incident) => incident,
            None => return,
        };
        let message = truncate(
            &format!("{}: {}", label, incident.message()),
            INCIDENT_CHARS,
        );
        ui.colored_label(self.theme.palette.alert_stroke, message);
    }

    fn canvas(&self, ui: &mut egui::Ui) {
        let state = match self.timeline.state_at(self.player.index) {
            Some(state) => state,
            None => return,
        };
        let step = self.active_step();
        let input = RenderInput {
            state,
            layout: &self.layout,
            program: &self.program,
            theme: &self.theme,
            effects: self.current_effects(),
            active_frame: step.as_ref().and_then(ExecutionStep::active_frame),
            active_scope: step.as_ref().and_then(ExecutionStep::active_scope),
            flow: step.as_ref().and_then(|step| step.flow.clone()),
        };
        let size = content_size(&self.layout);
        egui::ScrollArea::both()
            .id_salt("memory_canvas")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (response, painter) = ui.allocate_painter(size, Sense::hover());
                let background = self.theme.palette.background;
                painter.rect_filled(response.rect, 0.0, background);
                let _ = draw_memory(&painter, response.rect.min, &input);
            });
    }

    fn code(&mut self, ui: &mut egui::Ui, follow: bool) {
        let error = self.error_text();
        let message = {
            let input = CodePanelInput {
                events: &self.events,
                program: &self.program,
                active_line: self.active_source_line(),
                theme: &self.theme,
                follow,
                editing: self.editing,
                error: error.as_deref(),
                focus: self.focus_editor,
            };
            code_panel(ui, &input, &mut self.source)
        };
        self.focus_editor = false;
        if let Some(message) = message {
            self.update(message);
            ui.ctx().request_repaint();
        }
    }

    fn trace(&mut self, ui: &mut egui::Ui, follow: bool) {
        let message = {
            let input = TracePanelInput {
                events: &self.events,
                timeline: &self.timeline,
                program: &self.program,
                theme: &self.theme,
                current: self.player.index,
                follow,
            };
            trace_panel(ui, &input)
        };
        if let Some(message) = message {
            self.update(message);
            ui.ctx().request_repaint();
        }
    }
}

impl eframe::App for VisorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.playing() {
            let delta = ui.ctx().input(|input| input.stable_dt);
            self.update(UiMessage::Tick(Tick::from_seconds(delta)));
        }
        let top = egui::Frame::side_top_panel(ui.style())
            .fill(self.theme.palette.background)
            .inner_margin(8.0);
        egui::Panel::top("toolbar").frame(top).show(ui, |ui| {
            self.toolbar(ui);
        });
        let follow = std::mem::take(&mut self.follow_cursor);
        let side = egui::Frame::side_top_panel(ui.style())
            .fill(self.theme.palette.panel_fill)
            .inner_margin(8.0);
        egui::Panel::left("code_panel")
            .resizable(true)
            .default_size(400.0)
            .size_range(260.0..=720.0)
            .frame(side)
            .show(ui, |ui| {
                self.code(ui, follow);
            });
        egui::Panel::right("trace_panel")
            .resizable(true)
            .default_size(300.0)
            .size_range(200.0..=520.0)
            .frame(side)
            .show(ui, |ui| {
                self.trace(ui, follow);
            });
        let central = egui::Frame::central_panel(ui.style())
            .fill(self.theme.palette.background)
            .inner_margin(8.0);
        egui::CentralPanel::default().frame(central).show(ui, |ui| {
            self.canvas(ui);
        });
        if self.playing() {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(FRAME_MS));
        }
    }
}

/// Inputs of the demo window: the lifecycle scene and its recorded trace.
pub fn sample_window() -> Option<VisorApp> {
    let program = match analyze_program(LIFECYCLE_SOURCE) {
        Analysis::Analyzed(program) => program,
        Analysis::Failed(error) => {
            eprintln!("syntax error on line {}: {}", error.line, error.detail);
            return None;
        }
    };
    let events = lifecycle_trace(&program);
    Some(VisorApp::new(program, events, LIFECYCLE_SOURCE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::player::DEFAULT_STEP_MS;
    use crate::edge::sample::{SAMPLE_SOURCE, sample_trace};
    use crate::stages::analyze::{Analysis, analyze_program};

    /// The original struct/array sample, kept for the trace-heavy assertions.
    fn app() -> VisorApp {
        let program = match analyze_program(SAMPLE_SOURCE) {
            Analysis::Analyzed(program) => program,
            Analysis::Failed(error) => panic!("the sample must analyse: {:?}", error),
        };
        let events = sample_trace(&program);
        VisorApp::new(program, events, SAMPLE_SOURCE)
    }

    #[test]
    fn the_window_opens_the_lifecycle_scene() {
        let mut app = sample_window().expect("the lifecycle scene must record");
        assert_eq!(app.timeline.len(), 17);
        assert_eq!(app.step(), 0);
        app.set_step(2);
        assert_eq!(app.layout().scopes.len(), 1, "the function scope is open");
        app.set_step(6);
        assert_eq!(app.layout().scopes.len(), 2, "the block adds a second band");
    }

    #[test]
    fn the_window_opens_before_the_first_event() {
        let app = app();
        assert_eq!(app.step(), 0);
        assert!(!app.playing());
        assert!(app.layout().frames.is_empty());
        assert!(app.layout().objects.is_empty());
    }

    #[test]
    fn stepping_to_the_end_rebuilds_the_layout() {
        let mut app = app();
        let length = app.timeline.len();
        app.set_step(length);
        assert_eq!(app.step(), length);
        assert_eq!(app.layout().frames.len(), 1);
        assert_eq!(app.layout().objects.len(), 2);
        assert_eq!(app.layout().connectors.len(), 2);
    }

    #[test]
    fn steps_are_clamped_to_the_timeline() {
        let mut app = app();
        let length = app.timeline.len();
        app.set_step(usize::MAX);
        assert_eq!(app.step(), length);
        app.set_step(0);
        assert_eq!(app.step(), 0);
        assert!(app.layout().objects.is_empty());
    }

    #[test]
    fn a_rejected_step_keeps_the_memory_and_records_the_incident() {
        let mut app = app();
        app.set_step(16);
        let objects_before = app.layout().objects.len();
        app.set_step(17);
        assert_eq!(app.layout().objects.len(), objects_before);
        let state = match app.timeline.state_at(17) {
            Some(state) => state,
            None => panic!("step 17 must exist"),
        };
        assert_eq!(state.incidents.len(), 1);
    }

    #[test]
    fn the_first_step_reports_the_frame_it_pushed() {
        let mut app = app();
        assert!(app.current_effects().is_empty());
        app.set_step(1);
        assert!(matches!(
            app.current_effects().first(),
            Some(Effect::FramePushed { .. })
        ));
    }

    #[test]
    fn the_active_line_follows_the_cursor() {
        let mut app = app();
        assert_eq!(app.active_source_line(), None);
        app.set_step(2);
        assert_eq!(app.active_source_line(), Some(6));
        app.set_step(3);
        assert_eq!(app.active_source_line(), Some(7));
    }

    #[test]
    fn a_rejected_step_has_no_effects_to_highlight() {
        let mut app = app();
        app.set_step(17);
        assert!(app.current_effects().is_empty());
    }

    #[test]
    fn moving_the_cursor_asks_the_trace_to_follow() {
        let mut app = app();
        assert!(!app.follow_cursor);
        app.update(UiMessage::NextStep);
        assert!(app.follow_cursor);
        app.follow_cursor = false;
        app.update(UiMessage::NextStep);
        assert!(app.follow_cursor);
    }

    #[test]
    fn next_and_previous_move_the_cursor() {
        let mut app = app();
        app.update(UiMessage::NextStep);
        assert_eq!(app.step(), 1);
        app.update(UiMessage::NextStep);
        assert_eq!(app.step(), 2);
        app.update(UiMessage::PrevStep);
        assert_eq!(app.step(), 1);
        app.update(UiMessage::PrevStep);
        app.update(UiMessage::PrevStep);
        assert_eq!(app.step(), 0);
    }

    #[test]
    fn toggling_play_changes_the_mode() {
        let mut app = app();
        assert!(!app.playing());
        app.update(UiMessage::TogglePlay);
        assert!(app.playing());
        app.update(UiMessage::TogglePlay);
        assert!(!app.playing());
    }

    #[test]
    fn a_full_tick_advances_one_step() {
        let mut app = app();
        app.update(UiMessage::TogglePlay);
        app.update(UiMessage::Tick(Tick {
            delta_ms: DEFAULT_STEP_MS,
        }));
        assert_eq!(app.step(), 1);
        assert!(app.playing());
    }

    #[test]
    fn short_ticks_accumulate_before_advancing() {
        let mut app = app();
        app.update(UiMessage::TogglePlay);
        app.update(UiMessage::Tick(Tick { delta_ms: 250 }));
        app.update(UiMessage::Tick(Tick { delta_ms: 250 }));
        assert_eq!(app.step(), 0);
        app.update(UiMessage::Tick(Tick { delta_ms: 250 }));
        assert_eq!(app.step(), 1);
    }

    #[test]
    fn a_paused_player_ignores_ticks() {
        let mut app = app();
        app.update(UiMessage::Tick(Tick { delta_ms: 700 }));
        assert_eq!(app.step(), 0);
    }

    #[test]
    fn the_end_of_the_trace_stops_the_player() {
        let mut app = app();
        let length = app.timeline.len();
        app.update(UiMessage::SetStep(length - 1));
        app.update(UiMessage::TogglePlay);
        app.update(UiMessage::Tick(Tick { delta_ms: 700 }));
        assert_eq!(app.step(), length);
        assert_eq!(app.layout().objects.len(), 2);
        assert!(app.playing());
        app.update(UiMessage::Tick(Tick { delta_ms: 700 }));
        assert_eq!(app.step(), length);
        assert!(!app.playing());
    }

    #[test]
    fn pressing_play_at_the_end_restarts_the_trace() {
        let mut app = app();
        let length = app.timeline.len();
        app.update(UiMessage::SetStep(length));
        app.update(UiMessage::TogglePlay);
        assert_eq!(app.step(), 0);
        assert!(app.playing());
    }

    #[test]
    fn editing_is_a_switch() {
        let mut app = app();
        assert!(!app.editing);
        app.update(UiMessage::ToggleEdit);
        assert!(app.editing);
        app.update(UiMessage::ToggleEdit);
        assert!(!app.editing);
    }

    #[test]
    fn editing_requests_focus_once() {
        let mut app = app();
        assert!(!app.focus_editor);
        app.update(UiMessage::ToggleEdit);
        assert!(app.focus_editor, "entering the editor asks for focus");
        app.focus_editor = false;
        app.update(UiMessage::ToggleEdit);
        assert!(!app.editing);
        assert!(!app.focus_editor, "going back to the view does not focus");
    }

    #[test]
    fn recording_new_source_replaces_the_trace() {
        let mut app = app();
        app.update(UiMessage::SetStep(4));
        app.source = "fn main() {\n    let a: i32 = 1;\n}".to_string();
        app.update(UiMessage::Record);
        assert_eq!(app.step(), 0, "a new recording starts at the beginning");
        assert!(app.error.is_none());
        assert_eq!(app.active_source_line(), None);
        assert_eq!(app.timeline.len(), 5, "start, scope, declare, exit, end");
        app.set_step(3);
        assert_eq!(app.active_source_line(), Some(2));
        assert_eq!(app.layout().frames.len(), 1, "the scalar lives in main");
    }

    #[test]
    fn a_broken_source_keeps_the_previous_trace_and_reports_the_line() {
        let mut app = app();
        let length = app.timeline.len();
        app.set_step(2);
        app.source = "fn main() {\n    let a: i32 = 1;\n".to_string();
        app.update(UiMessage::Record);
        assert_eq!(app.timeline.len(), length, "the old trace stays");
        assert_eq!(app.step(), 2, "the cursor stays");
        let error = app.error.as_ref().expect("the failure must be kept");
        assert_eq!(error.line, 1);
        assert!(app.error_text().expect("text").starts_with("line 1: "));
    }

    #[test]
    fn a_successful_recording_clears_the_error() {
        let mut app = app();
        app.source = "fn main() {\n    let a: i32 = 1;\n".to_string();
        app.update(UiMessage::Record);
        assert!(app.error.is_some());
        app.source = "fn main() {\n    let a: i32 = -2 * 3;\n}".to_string();
        app.update(UiMessage::Record);
        assert!(app.error.is_none());
    }

    #[test]
    fn the_active_box_follows_the_cursor() {
        let mut app = app();
        app.source = "fn main() {\n    let a: i32 = 1;\n}".to_string();
        app.update(UiMessage::Record);
        app.set_step(3);
        let step = app.active_step().expect("step 3 exists");
        assert_eq!(step.active_frame(), Some(crate::model::ids::FrameId(1)));
        assert_eq!(step.line, Some(2));
    }

    #[test]
    fn a_branch_exposes_its_flow_edge() {
        let mut app = app();
        app.source =
            "fn main() {\n    let n: i32 = 3;\n    if n > 2 {\n        let big: i32 = 1;\n    }\n}"
                .to_string();
        app.update(UiMessage::Record);
        let position = app
            .events
            .iter()
            .position(|event| matches!(event, TraceEvent::ConditionalJump { .. }))
            .expect("the trace records the jump")
            + 1;
        app.set_step(position);
        let step = app.active_step().expect("the jump step exists");
        let flow = step.flow.expect("the jump carries an edge");
        assert_eq!(flow.outcome, crate::model::events::ConditionOutcome::True);
        assert_eq!(flow.from_line, 3);
        assert_eq!(flow.to_line, 4);
    }
}
