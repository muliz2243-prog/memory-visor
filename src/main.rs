use memory_visor::edge::player::{DEFAULT_STEP_MS, Player, PlayerPlan, Tick};
use memory_visor::edge::sample::{SAMPLE_SOURCE, sample_trace};
use memory_visor::model::events::TraceEvent;
use memory_visor::model::ids::FrameId;
use memory_visor::model::memory::{Location, MemoryState, ObjectShape, Path};
use memory_visor::model::program::{FunctionDef, ProgramTree};
use memory_visor::shell::format::{format_path, format_value};
use memory_visor::shell::narrative::{describe_event, describe_location, describe_step};
use memory_visor::shell::theme::Texts;
use memory_visor::stages::analyze::{Analysis, analyze_program};
use memory_visor::stages::layout::{LayoutConfig, layout_memory};
use memory_visor::stages::replay::{Timeline, build_timeline};

fn main() {
    match analyze_program(SAMPLE_SOURCE) {
        Analysis::Failed(error) => {
            println!("syntax error on line {}: {}", error.line, error.detail);
        }
        Analysis::Analyzed(program) => run(&program),
    }
}

fn run(program: &ProgramTree) {
    let events = sample_trace(program);
    let timeline = build_timeline(program, &events);
    print_tree(program);
    print_steps(&events, &timeline, program);
    print_memory(&timeline);
    print_layout(&timeline);
    print_summary(&timeline);
    print_playback(&timeline);
}

// ------------------------------------------------------------------ static tree

fn print_tree(program: &ProgramTree) {
    println!("== static tree ==");
    let entry = program.function(program.entry);
    match entry {
        Some(definition) => println!("entry: {}", definition.name),
        None => println!("entry: missing"),
    }
    for structure in &program.structs {
        println!(
            "struct {}: {} fields",
            structure.name,
            structure.fields.len()
        );
    }
    for function in &program.functions {
        print_function(function, program);
    }
}

fn print_function(function: &FunctionDef, program: &ProgramTree) {
    println!(
        "fn {} at line {}: {} parameters, {} locals, {} calls",
        function.name,
        function.start_line,
        function.parameters.len(),
        function.declarations.len(),
        function.calls.len()
    );
    for call in &function.calls {
        let target = program
            .function(call.target)
            .map(|definition| definition.name.as_str())
            .unwrap_or("unknown");
        println!("  line {} -> {}", call.line, target);
    }
}

// ----------------------------------------------------------------------- steps

fn print_steps(events: &[TraceEvent], timeline: &Timeline, program: &ProgramTree) {
    println!();
    println!("== trace ==");
    for (index, event) in events.iter().enumerate() {
        let step = &timeline.steps[index];
        println!(
            "[{:>2}] {:<28} {}",
            event.sequence().0,
            describe_event(event, program),
            describe_step(step.effects.as_slice(), step.incident.as_ref())
        );
    }
}

// ---------------------------------------------------------------------- memory

fn print_memory(timeline: &Timeline) {
    let state = match timeline.state_at(timeline.len()) {
        Some(state) => state,
        None => return,
    };
    let texts = Texts::english();
    println!();
    println!(
        "== memory after {} events ==",
        state.last_sequence.map_or(0, |s| s.0)
    );
    for frame in &state.frames {
        println!("frame {} (function #{})", frame.id.0, frame.function.0);
        for variable in &frame.variables {
            let marker = match incident_marker(state, frame.id, &variable.name) {
                Some(symbol) => symbol,
                None => " ".to_string(),
            };
            println!(
                "  {} {} = {}",
                marker,
                variable.name,
                format_value(state, &variable.value, &texts)
            );
        }
    }
    for object in &state.heap {
        println!(
            "heap object {} '{}' {:?}",
            object.id.0, object.label, object.state
        );
        print_shape(state, &object.shape, &texts);
    }
}

fn incident_marker(state: &MemoryState, frame: FrameId, variable: &str) -> Option<String> {
    let location = Location::Cell(Path::Stack {
        frame,
        variable: variable.to_string(),
    });
    state.incident_at(&location).map(|_| "!".to_string())
}

fn print_shape(state: &MemoryState, shape: &ObjectShape, texts: &Texts) {
    match shape {
        ObjectShape::Structure { fields, .. } => {
            for field in fields {
                println!(
                    "  .{} = {}",
                    field.name,
                    format_value(state, &field.value, texts)
                );
            }
        }
        ObjectShape::Sequence { elements, .. } => {
            for (index, value) in elements.iter().enumerate() {
                println!("  [{}] = {}", index, format_value(state, value, texts));
            }
        }
    }
}

// --------------------------------------------------------------------- layout

fn print_layout(timeline: &Timeline) {
    let state = match timeline.state_at(timeline.len()) {
        Some(state) => state,
        None => return,
    };
    let layout = layout_memory(state, &LayoutConfig::default());
    println!();
    println!("== layout ==");
    println!(
        "content: {:.0} x {:.0} points",
        layout.content.width, layout.content.height
    );
    for frame in &layout.frames {
        println!(
            "frame {} at ({:.0}, {:.0}) {:.0}x{:.0}",
            frame.frame.0, frame.rect.x, frame.rect.y, frame.rect.width, frame.rect.height
        );
        for slot in &frame.slots {
            println!(
                "  {:<18} at ({:.0}, {:.0})",
                format_path(&slot.path),
                slot.rect.x,
                slot.rect.y
            );
        }
    }
    for object in &layout.objects {
        println!(
            "object {} at ({:.0}, {:.0}) {:.0}x{:.0}",
            object.object.0, object.rect.x, object.rect.y, object.rect.width, object.rect.height
        );
    }
    for connector in &layout.connectors {
        println!(
            "arrow {} -> object {}: ({:.0}, {:.0}) -> ({:.0}, {:.0})",
            format_path(&connector.source),
            connector.target.0,
            connector.from.x,
            connector.from.y,
            connector.to.x,
            connector.to.y
        );
    }
}

// --------------------------------------------------------------------- summary

fn print_summary(timeline: &Timeline) {
    println!();
    println!("== summary ==");
    println!(
        "events: {}, rejected: {}",
        timeline.len(),
        timeline.rejection_count()
    );
    let last = match timeline.state_at(timeline.len()) {
        Some(state) => state,
        None => return,
    };
    for incident in &last.incidents {
        println!(
            "sequence {} pinned on {}: {}",
            incident.sequence().0,
            describe_location(incident.location()),
            incident.message()
        );
    }
}

fn print_playback(timeline: &Timeline) {
    let mut player = Player::new().running();
    let mut ticks = 0;
    let mut reached = 0;
    loop {
        if ticks >= 512 {
            break;
        }
        let plan = player.plan(
            Tick {
                delta_ms: DEFAULT_STEP_MS,
            },
            timeline.len(),
        );
        match plan {
            PlayerPlan::Finished => break,
            _ => {
                player = player.apply(plan);
                reached = player.index;
                ticks += 1;
            }
        }
    }
    println!(
        "playback: reached step {} of {} in {} ticks",
        reached,
        timeline.len(),
        ticks
    );
}
