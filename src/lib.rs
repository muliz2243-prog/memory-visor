// The style guide forbids `?` in the core: every error has to be a branch the
// reader can follow. Clippy's `question_mark` lint asks for the opposite, so it
// is disabled once, here, instead of scattering `#[allow]` over the stages.
#![allow(clippy::question_mark)]

pub mod edge;
pub mod model;
pub mod shell;
pub mod stages;
