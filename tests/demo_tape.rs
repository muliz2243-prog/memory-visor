use memory_visor::edge::sample::{SAMPLE_SOURCE, sample_trace};
use memory_visor::model::ids::ObjectId;
use memory_visor::model::memory::{Location, ObjectState, Path, PrimitiveValue, Segment, Value};
use memory_visor::model::outcome::Incident;
use memory_visor::model::program::ProgramTree;
use memory_visor::stages::analyze::{Analysis, analyze_program};
use memory_visor::stages::replay::{Timeline, build_timeline};

fn program() -> ProgramTree {
    match analyze_program(SAMPLE_SOURCE) {
        Analysis::Analyzed(tree) => tree,
        Analysis::Failed(error) => panic!("sample source does not parse: {:?}", error),
    }
}

fn timeline() -> Timeline {
    let program = program();
    build_timeline(&program, &sample_trace(&program))
}

fn field_path(object: u32, name: &str) -> Location {
    Location::Cell(Path::Object {
        object: ObjectId(object),
        segments: vec![Segment::Field(name.to_string())],
    })
}

fn whole_object(object: u32) -> Location {
    Location::Cell(Path::Object {
        object: ObjectId(object),
        segments: Vec::new(),
    })
}

#[test]
fn the_demo_applies_every_event_but_the_write_on_released_memory() {
    let timeline = timeline();
    assert_eq!(timeline.len(), 18);
    assert_eq!(timeline.rejection_count(), 1);
    let rejected: Vec<usize> = timeline
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.incident.is_some())
        .map(|(index, _)| index)
        .collect();
    assert_eq!(rejected, vec![16]);
}

#[test]
fn the_rejected_write_is_pinned_on_the_exact_field() {
    let timeline = timeline();
    let state = timeline
        .state_at(17)
        .expect("state after the rejected write");
    let incident = state.incidents.first().expect("one incident");
    assert!(matches!(incident, Incident::InvalidEvent { .. }));
    assert_eq!(incident.location(), &field_path(1, "x"));
}

#[test]
fn the_runtime_error_is_pinned_on_the_failing_zone() {
    let timeline = timeline();
    let state = timeline.state_at(timeline.len()).expect("final state");
    let last = state.incidents.last().expect("runtime incident");
    assert!(matches!(last, Incident::RuntimeError { .. }));
    assert_eq!(last.location(), &whole_object(2));
    assert!(last.message().contains("out of bounds"));
}

#[test]
fn the_final_memory_holds_the_demo_values() {
    let timeline = timeline();
    let state = timeline.state_at(timeline.len()).expect("final state");
    assert_eq!(state.frames.len(), 1);
    let frame = &state.frames[0];
    let origin = frame
        .variables
        .iter()
        .find(|variable| variable.name == "origin")
        .expect("origin");
    assert_eq!(origin.value, Value::Reference(ObjectId(1)));
    let total = frame
        .variables
        .iter()
        .find(|variable| variable.name == "total")
        .expect("total");
    assert_eq!(total.value, Value::Primitive(PrimitiveValue::Integer(253)));
    assert_eq!(state.heap.len(), 2);
    assert_eq!(state.heap[0].state, ObjectState::Released);
    assert_eq!(state.heap[1].state, ObjectState::Alive);
}

#[test]
fn scrubbing_back_restores_the_older_memory() {
    let timeline = timeline();
    let nested = timeline.state_at(2).expect("state inside build_point");
    assert_eq!(nested.frames.len(), 2);
    let early = timeline.state_at(8).expect("state after eight events");
    assert_eq!(early.frames.len(), 1);
    assert_eq!(early.heap.len(), 2);
    assert!(early.incidents.is_empty());
    assert_eq!(early.heap[0].state, ObjectState::Alive);
    let before_release = timeline.state_at(15).expect("state before the release");
    let after_release = timeline.state_at(16).expect("state after the release");
    assert_eq!(before_release.heap[0].state, ObjectState::Alive);
    assert_eq!(after_release.heap[0].state, ObjectState::Released);
}

#[test]
fn rebuilding_the_timeline_is_deterministic() {
    assert_eq!(timeline(), timeline());
}
