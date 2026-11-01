<div align="center">

<img src=".idea/icon.svg" alt="lazy-container icon" width="96" height="96">

# lazy-container

**A terminal UI for [Apple's `container` CLI](https://github.com/apple/container), inspired by [lazydocker](https://github.com/jesseduffield/lazydocker).**

See your Linux containers and images on a Mac, and start, stop, create and delete containers, all from one keyboard-driven screen.

![Platform: macOS on Apple silicon](https://img.shields.io/badge/platform-macOS%20%C2%B7%20Apple%20silicon-lightgrey)
![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-B7410E)
![Built with ratatui](https://img.shields.io/badge/built%20with-ratatui-blue)
![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-green)

</div>

```text
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ lazy-container ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃┌ Containers (all) ──────────────────────────────────────────────────────────────────────────┐┃
┃│   ID              STATE       IP                                                           │┃
┃│ > postgres        running     192.168.64.3/24                                              │┃
┃│   web             running     192.168.64.4/24                                              │┃
┃│   redis           stopped                                                                  │┃
┃│                                                                                            │┃
┃└────────────────────────────────────────────────────────────────────────────────────────────┘┃
┃┌ Images ────────────────────────────────────────────────────────────────────────────────────┐┃
┃│   NAME                                  TAG     DIGEST       PLATFORM                      │┃
┃│   alpine                                latest  4bcff63911fc linux/arm64 +7                │┃
┃│   nginx                                 alpine  b03ccb7431a2 linux/arm64 +7                │┃
┃│   postgres                              17      6cf6142afacf linux/arm64 +7                │┃
┃│   redis                                 8       e5b3a9a2b5d4 linux/arm64 +7                │┃
┃│                                                                                            │┃
┃└────────────────────────────────────────────────────────────────────────────────────────────┘┃
┗━━━ Shift+↑↓: panel .: filter /: help s: stop d/D: delete/purge E: shell Esc: back q: quit ━━━┛
```

<sub>Rendered by the app's own widgets, using sample data.</sub>

## Features

- **Live lists:** containers and images refresh every 2 seconds, so changes made in another terminal show up on their own. The cursor stays on the item it was on.
- **Container lifecycle:** start or stop a container with one key. Commands run in the background, so the UI never freezes, and the row shows `starting…` or `stopping…` until they finish.
- **Create from an image:** pick an image, type a name, and the new container is selected in the list.
- **Safe deletes:** a dialog shows what will go before anything is removed, including whether a running container will be killed and which volumes go with it or stay.
- **System control:** a status screen shows whether the `container` system is running and can start or stop it. Once the system is up, the app moves to the home screen by itself. If the system goes down, the app returns to the status screen.
- **Shell shortcut:** copy a ready-to-paste command that opens a shell in the selected container, using `bash` if it exists and `sh` if not.
- **Drag to copy:** drag across any text on screen to copy it to the clipboard.
- **Readable errors:** when a command fails, the message from `container` stays on screen until you dismiss it.
- **Built-in cheat sheet:** press `/` for a panel of common `container` commands.

## Requirements

- A Mac with Apple silicon
- macOS 26 or later (`container` also runs on macOS 15, with major networking limitations)
- [`container`](https://github.com/apple/container/releases) 1.4.1 or later, on your `PATH`
- [Rust](https://rustup.rs) 1.88 or later, to build from source

## Installation

```bash
cargo install --git https://github.com/Pianisimo/rust_lazy-container lazy-container
```

Or build it from a clone:

```bash
git clone https://github.com/Pianisimo/rust_lazy-container.git
cd rust_lazy-container
cargo build --release -p lazy-container
./target/release/lazy-container
```

### First run

If you've just installed `container`, start it once from a normal terminal:

```bash
container system start
```

The first start asks whether to download a Linux kernel. lazy-container can't answer that prompt, so starting the system from inside the app fails until the kernel is installed. After that, `s` on the status screen works as expected.

## Usage

```bash
lazy-container
```

The app opens on the status screen and moves to the home screen as soon as the system is running.

### Home screen

| Key | Action | Works in |
|---|---|---|
| `↑` / `↓` | Move the cursor | Both panels |
| `Shift+↑` / `Shift+↓` | Switch panel | Both panels |
| `s` | Start or stop the selected container | Containers |
| `d` | Delete the selected container, after asking | Containers |
| `D` | Purge: delete the container and its anonymous volumes, after asking | Containers |
| `E` | Copy a command that opens a shell in the container (it must be running) | Containers |
| `.` | Show all containers or only running ones | Both panels |
| `c` | Create a container from the selected image, without starting it | Images |
| `/` | Show or hide the command cheat sheet | Both panels |
| `Esc` | Close the cheat sheet, or go back to the status screen | Both panels |
| `q` | Quit | Both panels |

In the delete dialog, `Enter` or `y` confirms and `Esc` or `n` cancels. Purge keeps named volumes and any anonymous volume that another container also mounts.

### Status screen

| Key | Action |
|---|---|
| `s` | Start or stop the `container` system |
| `Enter` | Open the home screen (while the system is running) |
| `q` | Quit |

> [!WARNING]
> `s` on the status screen stops the whole system straight away, without asking first. That stops every running container and deletes any that were started with `--rm`.

## How it works

lazy-container has no daemon and doesn't use a private API. It runs the same `container` commands you would type and parses their `--format json` output, so what you see always matches the CLI.

The commands run on background threads and report back to the UI over a channel. A slow `container stop` therefore can't block the interface, and a system stop that's under way still finishes if you quit the app.

## Roadmap

The next features under consideration are logs and stats tabs, restart, volume and network panels, list filtering and bulk actions. [`tentative-features.md`](tentative-features.md) has the full comparison with lazydocker and the features the CLI can't support.

## Development

```bash
cargo run -p lazy-container   # run the app
cargo test --workspace        # run the tests
cargo clippy --workspace      # lint
```

The repository is a Cargo workspace:

| Path | Contents |
|---|---|
| `crates/lazy-container` | The binary, a thin wrapper around `lib` |
| `crates/lib` | The app: screens, background worker and CLI integration |
| `crates/examples` | Small standalone ratatui examples: a counter, panel navigation and external events |

`container` isn't Docker: its commands, flags and output often differ. Check `container <command> --help` rather than assuming the Docker equivalent exists.

## Acknowledgements

- [lazydocker](https://github.com/jesseduffield/lazydocker), the inspiration for this project
- [ratatui](https://ratatui.rs), which draws the interface
- [Apple `container`](https://github.com/apple/container), which does the real work

lazy-container is an independent project and isn't affiliated with or endorsed by Apple.

## License

Copyright 2026 sergio

Licensed under the [Apache License, Version 2.0](LICENSE).
