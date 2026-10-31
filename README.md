# Memory Visor

![Tests](https://github.com/muliz2243-prog/memory-visor/actions/workflows/rust.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)

A time-travel debugger for Rust memory: lexical scopes, ownership, borrows and
`Drop` are recorded as events and replayed on an `egui` canvas with a movable
cursor.

![Memory Visor showing two live borrows inside a block](docs/screenshot.png)

Run the desktop window with `cargo run --bin visor`; `cargo run` starts the CLI.
The Source panel opens in read-only mode and switches to a live editor with
**Edit**: every keystroke is recorded, and **Open** loads a `.rs` file from disk.
See `DESIGN.md` for the architecture and `AGENTS.md` for the verification
commands.

## Build and run

```bash
cargo build --release
cargo test --all-targets
cargo run              # CLI walkthrough
cargo run --bin visor  # desktop window
```

## 🏗️ Architecture & Multi-Language Extensibility

The current version implements Rust's memory model strictly (LIFO scopes, `Drop`
destructors and a borrow-checker simulation), but the project is built on a
decoupled, layered architecture:

- **Presentation and History layer (`egui` / `history.rs`):** language-agnostic.
  It only computes the geometry of scopes and references and drives time-travel
  playback over a tape of events.
- **Semantic engine (`apply.rs`):** owns the validation rules for memory and
  lifetime semantics.

### Future extensions

The layering allows extending the visualizer to other languages (such as
Python, C++ or JavaScript) by abstracting the semantic engine behind an adapter
trait (`LanguageSemantics`). That adapter would interpret language-specific
memory events (for example garbage collection or reference counting) while
reusing the same visual and playback engine.

## License

Released under the MIT License. See [LICENSE](LICENSE).
