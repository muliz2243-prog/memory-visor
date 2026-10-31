# Memory Visor — core design

A time-travel debugger for Rust data lifetime: scopes, ownership, borrows and
`Drop` are recorded as events and replayed in a cursor-driven tape. This
document describes the current architecture. Code, comments and documentation
are in English.

## 1. The architectural decision

The core is an event engine. Callbacks, observers and event buses are forbidden
in the core, and the shape is fixed:

    current_state + event -> next_state + effects

Events are **data** (`enum TraceEvent`), they arrive in order with a monotonic
sequence number, and a single pure function folds them. The UI receives no
notifications: it reads an immutable snapshot and asks for the next step.
Replay is deterministic, so moving backwards needs no extra machinery.

## 2. Pipeline

| Stage | Input | Output |
|---|---|---|
| `analyze` | `&str` source | `Analysis::Analyzed(ProgramTree)` or `Analysis::Failed(SyntaxError { line, detail })` |
| `apply` | `Application { state, event, program }` | `Applied { state, effects }` or `Rejected { state, incident }` |
| `replay` | `&ProgramTree`, `&[TraceEvent]` | `Timeline { states, steps }` |
| `layout` | `&MemoryState`, `&LayoutConfig` | `MemoryLayout { frames, objects, connectors, scopes, content }` |
| `history` | a `TraceEvent` at a time | ordered tape + cursor (`History`) |
| `player` (edge) | `Tick { delta_ms }`, event count | `PlayerPlan::{Advance, Wait, Finished}` |

`effects` are animation instructions with no time in them (`Wrote`,
`CreatedObject`, `FramePushed`, `Dropped`, `Borrowed`, `Moved`, …). The clock
lives only at the edge, inside `Tick`.

## 3. Memory model

- **Stack**: `Vec<StackFrame>`; the last one is the running frame. Each frame
  keeps its function, current line and variables in declaration order.
- **Heap**: `Vec<HeapObject>` with stable indices. Two shapes:
  `Structure { structure, fields }` and `Sequence { element_type, elements }`.
- **Nesting** is `Value::Reference(ObjectId)`. It is an ownership arrow, not a
  borrow; borrows are `Loan`, described below.
- **Types** live only in `ProgramTree`. Memory never repeats them, so there is
  no second source of truth.
- **`Location`** anchors an incident: `Trace`, `Stack`, `Frame`, `Cell(Path)`,
  `Line`, `Heap`. `Path` navigates the stack
  (`Path::Stack { frame, variable }`) or an object by field and index. The
  `frame` is mandatory: `total` exists at the same time in `sum_scores` and in
  `main`, and without it both cells would share an address. A write to a frame
  that is not on top is rejected with `FrameNotCurrent`.
- **`ScopeRegistry`** tracks open and closed lexical scopes.
- **`Vec<Loan>`** tracks live borrows. Both structures are part of
  `MemoryState`, so every snapshot carries its own scope and loan state and
  scrubbing back removes loans that had not been taken yet.

## 4. Lexical scopes (`model/scope.rs`)

Rust drops locals at the end of their scope in reverse declaration order. The
registry enforces that shape instead of letting each caller re-derive it.

- `Scope` stores `id`, `kind`, owning `frame`, `parent`, `depth` (0 for a
  function body, 1 for a block inside it, …), the line of the opening brace and
  the variable names in declaration order.
- `ScopeKind` is `Function`, `Block` or `Loop`. All three drop the same way; the
  kind exists so the canvas can label the band (`fn`, `{`, `loop`).
- `ScopeRegistry::open` allocates the next id. `open_as` accepts the id the
  recorder announced and returns `None` if it was already used, so a replay
  cannot alias two scopes; `apply` turns that into `DuplicateScope`.
- `declare` appends the name to the running scope and ignores repeats: the
  engine keeps the first declaration, which is the lifetime the trace already
  announced. With no open scope it returns `None` without failing.
- `close` pops the running scope and returns its `ClosedScope`, whose `dropped`
  list is the variables **reversed**. That is the only place drop order is
  computed.
- `close_until(target)` closes from the innermost scope out to `target`; a
  function return uses it. `close_frame(frame)` closes every scope owned by the
  frame, innermost first, so a frame can never outlive its scopes.
- `resolve(name)` returns the innermost open scope that declares the name plus
  its depth, which is how shadowing is answered.
- Closed scopes stay in `scopes` for replay; `open_scopes()` exposes only the
  stack, outermost first, so the layout can draw the nesting.

`EndFunction` closes the frame's scopes and emits one `Dropped` effect per
variable before `FramePopped`.

## 5. Chronological tape (`model/history.rs`)

`History` owns the events in the order the recorder produced them and a cursor
that is a position **between** steps: `0` is before the first event and `len()`
is after the last one. Moving the cursor never mutates the tape; a snapshot is
rebuilt by replaying the prefix.

- `push(event)` requires the sequence to start at 1 and strictly increase.
  Anything else returns `HistoryError::OutOfOrder { last, received }` and leaves
  the tape untouched — a refused event is never half-recorded. A gap is
  accepted: lost telemetry should not stop the replay.
- On success the cursor jumps to the live edge (`len()`), so a recorder that
  appends sees the newest step.
- `next()` and `prev()` move one event and return the event the cursor now sits
  after; `set_cursor(step)` clamps to `0..=len` and returns where it landed.
  `next` keeps that name because it drives the transport controls; it is not an
  iterator.
- `at(step)` reads any step without moving the cursor (`step - 1` indexes the
  tape, and step 0 has no event). `current()`, `events()`, `last_sequence()`,
  `at_start()` and `at_end()` complete the read-only surface.

`Timeline` still precomputes one snapshot per event, which is what the UI
scrubs. `History` is the model-level tape the recorder writes and the tests
exercise; the two share the same step numbering.

## 6. Borrow and lifecycle semantics (`stages/apply.rs`)

Each event is applied atomically: validate, mutate, return effects, or reject
without touching memory.

- **`EnterScope`** needs a running frame (`EmptyStack`) and an unused id
  (`DuplicateScope`). It records the current line.
- **`ExitScope`** must name the running scope, otherwise `ScopeNotCurrent`. It
  closes the scope, removes its variables from the frame and emits one
  `Dropped` per variable, last declared first. That cascade is the visual proof
  of LIFO.
- **`Drop`** on a stack cell removes the variable (`FrameNotCurrent` or
  `UnknownVariable` when it cannot). On a whole heap object it releases the
  object; a partial object path is `InvalidPath`.
- **`Borrow`** requires the target cell to exist. The exclusion rule is Rust's:
  a mutable loan excludes **every** other loan on the same target, and an
  immutable loan excludes any mutable one. Two immutable loans may coexist. A
  conflict is pinned on the target cell as `ConflictingBorrow`.
- **`ReleaseBorrow`** removes the loan by borrower; without one it is
  `UnknownBorrow`, pinned on the borrower cell.
- **`Move`** currently copies a stack slot into another stack slot, empties the
  source and emits both `Moved` and `Dropped`. Heap moves would need a shape
  check the recorder cannot express yet, so they are rejected with
  `UnsupportedMove` rather than guessed.

Loans are kept as `Loan { borrower, target, kind }` with
`BorrowKind::{Immutable, Mutable}`. They are data, not references into the
heap: a loan never changes a `Value`, it only forbids other loans.

## 7. Pinned errors

An invalid event does **not** erase what is drawn: the state is unchanged, an
`Incident::InvalidEvent { sequence, location, reason }` is created and stored in
`MemoryState::incidents`. Incidents are part of the snapshot, so stepping back
removes those that had not happened yet.

The UI paints the incident at its `Location`:

- write to a released object → `Cell(Object { object: 1, segments: [Field("x")] })`
  (the exact field);
- index out of range → `Cell(Object { object: 2, segments: [] })` (the whole
  box; an object path with no segments means "the object");
- unknown function or bad sequence → `Trace` (the trace panel);
- empty stack or depth limit → `Stack`;
- duplicate scope or non-current scope → `Line` / `Stack`;
- borrow conflict or unknown borrower → the cell of the target or borrower.

## 8. Covered edge cases

| Case | Behaviour | Test |
|---|---|---|
| Empty source / no `main` | `SyntaxError` on line 1 | `rejects_source_without_main` |
| Two `main` functions | `SyntaxError` on the second | `rejects_duplicate_main` |
| Unclosed block | `SyntaxError` on the block line | `rejects_unclosed_function` |
| Unknown type | `SyntaxError` with line | `rejects_unknown_type` |
| Duplicate or backwards sequence | rejected; memory untouched | `a_duplicate_sequence_does_not_touch_memory` |
| Gap in the sequence | accepted (lost telemetry); cursor advances | `History::push` |
| Stack depth | `DepthExceeded` at 256 frames | `the_depth_limit_is_reported_not_hit` |
| Repeated variable | `DuplicateVariable` | `a_duplicate_variable_is_rejected` |
| Repeated object | `DuplicateObject` | `a_duplicate_object_is_rejected` |
| Released object | `ReleasedObject` on the field | `writing_to_a_released_object_is_pinned_on_the_field` |
| Empty stack | `EmptyStack` | `ending_without_a_frame_is_rejected` |
| Type mismatch | `TypeMismatch` on the cell | `a_type_mismatch_is_pinned_on_the_variable` |
| Struct missing or extra field | `MissingField` / `UnknownField` | `a_struct_object_must_carry_its_declared_fields` |
| Sequence element with wrong type | `TypeMismatch` on `[i]` | `sequence_elements_must_match_the_declared_type` |
| Unknown function | `UnknownFunction` on `Trace` | `an_unknown_function_is_pinned_on_the_trace` |
| Write to a frame that is not running | `FrameNotCurrent` on `Frame(id)` | `a_write_to_a_frame_that_is_not_running_is_rejected` |
| Borrow while another loan is live | `ConflictingBorrow` on the target cell | `a_mutable_loan_conflicts_with_a_live_immutable_one` |
| Releasing a loan that was never taken | `UnknownBorrow` on the borrower cell | `releasing_a_missing_loan_is_rejected` |
| Moving a stack slot | value copied to the target, source emptied | `moving_a_stack_slot_empties_the_source` |
| Block exit | variables dropped last declared first | `exit_scope_drops_variables_last_declared_first` |
| Nested scope | the block opens inside the function at `depth + 1` | `a_block_nests_inside_the_function` |
| Scope close order | variables returned in reverse declaration order | `closing_returns_variables_in_reverse_declaration_order` |
| Name resolution | the innermost scope that declares the name wins | `resolving_a_name_prefers_the_innermost_scope` |
| Opening an id twice | `DuplicateScope` on the scope line | `opening_a_scope_that_already_exists_is_rejected` |
| Closing an outer scope first | `ScopeNotCurrent`; the stack is untouched | `closing_a_scope_that_is_not_on_top_is_rejected` |
| Moving into a heap cell | `UnsupportedMove` on the source cell | `moving_into_a_heap_cell_is_not_supported` |
| Huge or invalid tick | clamped to 250 ms / 0 | `a_clamped_tick_never_jumps_more_than_one_step` |

## 9. Conscious deviations

1. **`#![allow(clippy::question_mark)]`** in `lib.rs`. The guidelines forbid `?`
   in the core and clippy pushes the opposite. It is disabled once, with the
   reason written down, instead of scattering `#[allow]` through the stages.
2. **Guards with `return Err(...)`**. They are the visible error path, not a
   jump in the middle of the flow. Functions that write memory have no failure
   branch: indices were resolved and validated before.
3. **`if let` in the `store_*` helpers**. Only to satisfy the borrow checker;
   they decide nothing. Decisions live in `check_*` and `resolve_*`.
4. **Sequence gaps accepted**. If the producer loses an event, the visor keeps
   going with what it has; a repeated number is still rejected, because going
   back would reapply writes already done.
5. **Subset parser, not a compiler**: `struct`, `fn`, `let name: Type`, calls
   `name(`. `Vec<T>` and `[T; n]` are recognised; other lines are opaque
   statements and ignored on purpose. What it cannot understand structurally it
   reports with a line number; it does not guess.
6. **Ownership arrows and borrow arrows are different data**. `Value::Reference`
   means "this cell owns that object" and is drawn as a connector.
   `Loan { borrower, target, kind }` means "this cell borrows that cell" and is
   drawn as a separate arrow. Merging them would lose the difference between
   moving and lending that this project exists to show.
7. **`Move` is stack-to-stack for now**. Partial heap moves would need a shape
   rule that the recorder does not emit yet; the reducer rejects them instead of
   inventing one.

## 10. Verification

    cargo fmt
    cargo clippy --all-targets -- -D warnings
    cargo test --all-targets

134 tests: 115 unit tests (analyzer, recorder, scope registry, history, reducer
lifecycle, layout, format, narrative, theme, render, window, code, trace,
replay, player, sample scenes), 13 reducer-tape integration tests and 6
full-trace tests of the demo program.

## 11. Geometry (`stages/layout.rs`)

`layout_memory` projects the state into rectangles, with no egui and no text
measurement:

- stack on the left, frames in call order (a new frame appears below, nothing
  moves); heap on the right, in creation order;
- one row per variable, field or index, carrying the same `Path` used by effects
  and incidents, so marking red is a direct lookup (`layout.cell(&path)`);
- one connector per `Value::Reference`, from the right edge of the cell to the
  title bar of the target object; missing targets draw nothing;
- **one `ScopeBox` per open scope**, outermost first. Its frame is the scope's
  frame, its horizontal inset is `depth * padding` (and the width shrinks by
  twice that), and its vertical extent spans the rows of the variables the scope
  declares. A scope with no variables yet keeps a thin band under the frame
  header, so it is visible the moment it opens;
- `content` is the total size; clipping and scrolling are view concerns;
- frame height = `header + rows * slot + padding`, with no magic numbers spread
  around.

Declared limitation: `value_at` resolves only one-field or one-index paths,
which are exactly the ones the layout draws. Nested structs are separate
objects, not rows inside a row.

## 12. Drawing (`shell/`)

Four files, none of them decides anything about the domain:

- `format.rs`: values and paths to text (`&1 Point#1`, `frame 2.total`) using the
  dictionary in `theme.rs`. No egui: it is tested without a window and the CLI
  uses it too.
- `narrative.rs`: one sentence per event and per effect (`wrote frame 2.total`,
  `created object 1`, `borrowed owner`, `dropped second`) plus the summary of a
  rejected step. The CLI and the trace panel call the same functions, so there
  are no two wordings to drift apart.
- `theme.rs`: `Palette` (dark surface, alert red `(255, 70, 70)`, effect amber
  `(255, 196, 87)`, active line blue `(110, 170, 255)`), `Texts` (English
  labels) and `Theme`. No panel hard-codes a colour or a word.
- `render.rs`: `draw_memory(&Painter, origin, &RenderInput)` paints connectors,
  scope bands, frames, loan arrows and objects, in that order, and returns the
  content size for the scroll area. Red is a query, not a parallel state:
  `cell_incident`, `frame_incident` and `object_incident` translate
  `Path`/`FrameId`/`ObjectId` into `state.incident_at`. `RenderInput.effects`
  carries the effects of the last step and the predicates `wrote_here`,
  `frame_touched`, `object_touched`, `dropped_here` and `dropped_in_frame`
  decide what is highlighted.

Drawing rules:

- scope bands go **under** the frames: a translucent rounded rectangle with a
  muted stroke, inset by depth, labelled with its `ScopeKind` token (`fn`, `{`,
  `loop`). Inner scopes read as insets of the outer one;
- one connector per `Connector`, from the cell's right edge to the object title;
- one arrow per live `Loan`, from the borrower cell to the target cell, drawn
  after the frames so it is visible. Immutable loans are thin and blue and carry
  `&`; mutable loans are thicker and amber and carry `&mut`;
- a cell incident paints that row: red fill, 2 px border and bright text;
- a frame or object incident paints the whole box, because there is no single
  row to blame;
- the last effect paints in amber the written cell, the pushed/popped frame or
  the created/released object, with a 2 px border. Amber is a hint about the
  current step, not a state change;
- a cell whose `Effect::Dropped` names it is dimmed and struck through. When the
  drop already removed the row (the usual case at `ExitScope`), the frame header
  turns amber instead, because the cell itself is gone;
- red always wins: `outline` and `accent` check the incident first, so a failure
  is never hidden by an effect highlight;
- a released object is drawn in grey with its last values: it keeps its place so
  the arrow pointing at it stays traceable;
- `render` does not measure text: it truncates by character count. Real
  measurement is another stage and another source of failures; truncation only
  prevents overflow.

## 13. Window, code, trace and playback (`edge/ui/`)

`VisorApp` implements `eframe::App` and takes no memory decisions:

- it holds the source (`String`, editable in the window), the `ProgramTree`,
  the `TraceEvent`s,
  the `Timeline` built from them, the `Theme`, the `LayoutConfig`, the current
  `MemoryLayout` and a `Player`. The cursor is `player.index`: there is no
  parallel `step` that could drift;
- `update(UiMessage)` is the only place that changes state. `NextStep`,
  `PrevStep` and `SetStep(n)` move the cursor; `TogglePlay` starts or stops;
  `Tick(delta)` feeds the player;
- widgets mutate nothing: they emit a message that is applied after the frame.
  `UiMessage` lives in `edge/ui/message.rs`, is `Copy` and carries the delta in
  `Tick(Tick)`, so playback is tested without a clock;
- `set_step` clamps to `0..=timeline.len()`, resets the accumulation
  (`Player::at`) and only rebuilds the geometry when the index changes;
- at the end, the next tick returns `PlayerPlan::Finished` and playback pauses;
  pressing play at the end rewinds to 0;
- while `playing()` is true, `App::ui` requests a frame every 50 ms
  (`request_repaint_after`) and turns `stable_dt` into `Tick`. There is no
  thread and no callback: `Player` accumulates deltas and advances at most one
  step per tick;
- the window is divided into four zones: `Panel::top("toolbar")`,
  `Panel::left("code_panel")` (260–720 px), `Panel::right("trace_panel")`
  (200–520 px) and the `CentralPanel` with a `ScrollArea::both` sized by
  `content_size(&layout)`;
- toolbar: `|<`, `<`, Play/Pause, `>`, `>|`, slider and `step n / m`. The last
  incident of the snapshot is shown in red, truncated to 120 characters;
- the code panel (`code.rs`) paints one row per line with monospaced numbering.
  The active line comes from `TraceEvent::active_line()`; when the event carries
  no line (return, release) it uses `MemoryState::current_line()`, the line the
  program continues on. The active row gets a blue background and a left bar;
- `line_anchors` maps events to lines: a call anchors both the function's first
  line and the call line; a jump anchors origin and destination.
  `step_for_line` picks the last event of that exact line; if the line never
  appears it keeps the previous step, and before the first anchor it uses the
  first step. Any line is clickable;
- a click on the code emits `UiMessage::SetStep`, same as the trace. No panel
  knows the other: both speak the same message bus;
- the source header carries `Edit`/`View`, `Run` and `Open`. In edit mode the
  rows are replaced by a `TextEdit::multiline` bound to the same buffer; every
  change emits `UiMessage::Record`, so the trace follows the typing. The editor
  keeps a stable widget id (`ui.make_persistent_id("source_editor")`), so the
  focus survives the error label appearing and clipboard shortcuts reach the
  widget; `ToggleEdit` sets `focus_editor`, which the next frame consumes with
  `request_focus` and clears, so `Edit` puts the caret in the buffer. `Open`
  asks `rfd` for a `.rs` file, loads it with `fs::read_to_string` and records it;
- `apply_source` calls `edge::recorder::record` on the buffer. On success it
  swaps the program, the events and the `Timeline`, resets the player to 0 and
  rebuilds the geometry; on failure it keeps the previous trace and cursor and
  stores the `RecordError`, which the header shows as `line n: detail` in red;
- the recorder is deliberately permissive. It runs `main` wherever it appears,
  skips the bodies of every other `fn` and every `struct`, ignores braces it
  cannot model by opening and closing anonymous scopes, drops statements that
  are not memory events (`return`, calls, assignments), and skips a `let` whose
  initializer cannot be reduced instead of rejecting the whole buffer. A buffer
  without `fn main` is validated and recorded as the body of an implicit `main`,
  so a bare sequence of `let` bindings still produces a trace. Typed `let`
  bindings
  with literal or arithmetic initializers and nested `{}` blocks are what it
  records; it never invents a borrow, so the demo tape keeps the hand-written
  loans;
- the trace panel (`trace.rs`) lists one row per event: sequence, narration and
  effects (or the rejection reason). The row of the applied step is selected and
  rejected rows are painted red. A click emits `UiMessage::SetStep(i + 1)`; the
  initial row emits `SetStep(0)`;
- `follow_cursor` turns on when the cursor changes and is taken once before the
  panels are drawn, so `scroll_to_me` follows playback in code and trace without
  stealing the scroll while the user is reading elsewhere;
- the binary is `src/bin/visor.rs` (`cargo run --bin visor`); `cargo run` still
  starts the CLI through `default-run`. eframe enters with
  `default-features = false` and only `glow` + `default_fonts`, so it does not
  pull in the wgpu backend.

The demo window is `sample_window()`: it analyses `LIFECYCLE_SOURCE` and records
`lifecycle_trace()`, a hand-written 17-event tape with a function scope, a
nested block, two immutable loans, one mutable loan after they are released and
two `ExitScope` cascades. `SAMPLE_SOURCE` and `sample_trace` remain for the CLI
and for the tests that depend on a fixed struct/array trace.

Tests without a window: they send `UiMessage` to `update` and check the cursor,
short-tick accumulation, pause and rewind; they also check the effects of the
current step, the active line, line → step mapping and that highlighting never
overrides an incident. No window and no real time are needed.

## 14. Pending

- playback speed (`Player::step_ms`) and transport keys;
- a recorder that emits `EnterScope`, `Borrow`, `ReleaseBorrow`, `Move` and
  `Drop` from real source (today it records `main` in any position — or the
  whole buffer as an implicit `main` — with typed `let` bindings and blocks,
  but still derives loans from the hand-written tape);
- highlighting the dropped path by name in the trace panel, not only on the
  canvas.
