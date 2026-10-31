use crate::edge::player::Tick;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiMessage {
    NextStep,
    PrevStep,
    SetStep(usize),
    TogglePlay,
    Tick(Tick),
    /// Switch the source panel between the highlighted view and the editor.
    ToggleEdit,
    /// Record the current source text into a fresh trace.
    Record,
    /// Ask the system for a file and record it.
    OpenFile,
}
