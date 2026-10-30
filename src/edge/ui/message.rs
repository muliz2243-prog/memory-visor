use crate::edge::player::Tick;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiMessage {
    NextStep,
    PrevStep,
    SetStep(usize),
    TogglePlay,
    Tick(Tick),
}
