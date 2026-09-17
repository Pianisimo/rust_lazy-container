# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Apple `container` CLI — use the `apple-container` skill

lazy-container is a ratatui TUI, modeled on lazydocker, for Apple's [`container`](https://github.com/apple/container) CLI. It runs the CLI through `std::process::Command` in `crates/lib/src/screens/cli.rs`, only ever from the background threads in `screens/worker.rs`, and parses its `--format json` output in `screens/status_screen.rs` and `screens/home_screen/`.

**Load the `apple-container` skill before any work that touches the `container` CLI** (most tasks here do): adding or changing a command, parsing its output, or planning a feature from `tentative-features.md`. `container` is not Docker. Never guess a command, flag, or output field from Docker; check the skill, or run `container <command> --help`.

- It is a personal skill in `~/.claude/skills/apple-container/`, not part of this repo. If it isn't in the session's skill list, say so instead of answering CLI questions from memory.
- Its notes for this project are in `references/inspection-and-scripting.md`, under "Our consumer: `rust/lazy-container`": parse `--format json` output instead of tables, and run blocking commands such as `container stop` off the UI thread.
- The skill and `tentative-features.md` were both checked against `container` 1.4.1. Run `container --version` first; on any other version, trust `container <command> --help` over both.

## Commands

```bash
# Build the whole workspace
cargo build

# Build a specific crate
cargo build -p lazy-container
cargo build -p simple-frame
cargo build -p panel-selection
cargo build -p external-events
cargo build -p lib

# Run a specific binary
cargo run -p lazy-container
cargo run -p simple-frame
cargo run -p panel-selection
cargo run -p external-events

# Run tests
cargo test                   # all workspace tests
cargo test -p lib            # tests for the lib crate only

# Lint
cargo clippy --workspace

# Check (faster than build, no codegen)
cargo check --workspace
```

## Architecture

This is a Cargo workspace (`resolver = "2"`) under `crates/`:

```
crates/
  lib/                  # shared library crate (App struct + Widget impl)
  lazy-container/       # main binary — thin wrapper that calls lib::App
  examples/
    simple-frame/       # standalone example: minimal ratatui counter app
    panel-selection/    # standalone example: two-panel keyboard navigation
    external-events/    # standalone example: mpsc + notify file-system watcher
```

### Shared lib crate (`crates/lib`)

`lib` exports a reusable `App` struct (counter with `u8` value, `exit` flag) and its `Widget` implementation. `lazy-container` depends on `lib` and delegates directly to it. The examples are fully self-contained — they duplicate the same patterns rather than depending on `lib`.

### Ratatui app pattern

All apps follow the same structure:

1. **`ratatui::run`** — entry point that sets up raw mode / alternate screen and restores the terminal on exit (including on panic).
2. **Draw loop** — `while !exit { terminal.draw(|frame| self.draw(frame)); self.handle_events(); }`. Ratatui re-renders the full UI every frame with no retained widget tree.
3. **`Widget` for `&App`** — rendering is implemented on a shared reference so it cannot mutate state. The borrow checker enforces the state-mutation / render separation.
4. **`Buffer`** — a 2-D grid of styled characters. Widgets write into it; ratatui diffs against the previous frame and flushes only changed cells.
5. **`Layout`** — splits a `Rect` into sub-`Rect`s using `Constraint`s (`Percentage`, `Length`, `Fill`, `Min`, `Max`). No heap allocation.

### External event integration (`external-events`)

When mixing keyboard input with another event source, replace `event::read()` (blocks forever) with `event::poll(timeout)` (returns after at most `timeout` ms). This gives the loop a heartbeat. Events from the background source (here: `notify` file-system watcher) cross the thread boundary via a stdlib `mpsc` channel, drained with `try_recv` in a loop each iteration.

`lib` uses the same pattern for the `container` CLI: `screens/worker.rs` runs commands and refreshes on two threads, and `App::run` drains their replies every tick.

### Workspace-wide lint rules

`unsafe_code = "forbid"` is set in `[workspace.lints.rust]`.

Release profile uses `opt-level = 3`, `strip = true`, `lto = true`, `codegen-units = 1`.
