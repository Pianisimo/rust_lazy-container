# Tentative features

Ideas for what to build next in lazy-container. None of them are decided yet.
Tick items off as they land.

[lazydocker](https://github.com/jesseduffield/lazydocker) is the reference for
what's missing. Each idea was checked against the `container` CLI **1.4.1**
(the version installed on 2026-09-17), so it can be built with commands that
CLI already has. lazydocker features the CLI can't support are listed under
[Not possible with the CLI](#not-possible-with-the-cli).

Suggested starting point: the four [foundations](#foundations), then the Logs
tab together with delete and shell. See [Suggested order](#suggested-order).

## Foundations

Most of the features below depend on these, so they come first.

- [x] **Refresh the home screen on a timer.** `App::refresh_status`
  (`crates/lib/src/screens/mod.rs`) only refreshes the status screen.
  - The problem: containers started in another terminal, or ones that crash,
    only show up after you act on something.
  - The fix: refresh both lists on the existing `STATUS_REFRESH_INTERVAL`.
    Call `HomeScreen::reselect_container` afterwards so the cursor stays on
    the same container; images need the same treatment.
  - If the container system stops while the home screen is open, go back to
    the status screen.
- [x] **Run commands in the background and show their errors.** `run_command`
  blocks until the command finishes, then discards its exit status and
  stderr. `container stop` alone can freeze the UI for 5 s (its default
  `--time`), and a failed start looks the same as nothing happening.
  - Move commands, and later the log and stats streams, to a worker thread
    that reports back over `std::sync::mpsc`. The main loop reads replies
    with `try_recv` on every tick, as `crates/examples/external-events` does.
  - Show progress ("stopping postgres…") while a command runs.
  - `Notice` (`crates/lib/src/screens/selection.rs`) is currently only used
    to confirm copies. Extend it so a failed command shows its stderr.
- [x] **Scroll long lists.** The lists are drawn as plain text and can't
  scroll. At 80×24 each panel fits 8 rows, and the cursor disappears when
  it moves past them.
  - Fix: ratatui's `Table` + `TableState` scrolls to the selected row and
    keeps the header aligned.
  - `render` runs on `&App`, so either build a `TableState` each frame from
    `selected_rows` or keep the scroll offset in `HomeScreen`.
- [x] **Parse JSON instead of text tables.** `ls`, `image ls`, `stats`,
  `system df` and `system status` all accept `--format json`. Field lists
  are under [Useful CLI output](#useful-cli-output).
  - One call returns a container's image, resources, start time, hostname,
    environment and published ports, plus each image's layer history.
  - Parsing no longer depends on column positions. The table parser already
    had to handle the `ADDR` → `IP` column rename.
  - This needs `serde` and `serde_json`. Mark fields `#[serde(default)]` or
    `Option` so a CLI update that drops one doesn't break parsing.

## lazydocker comparison

| lazydocker | `container` 1.4.1 | lazy-container today |
|---|---|---|
| Container and image lists, hide stopped | `ls -a`, `image ls` | ✅ |
| Start / stop | `start`, `stop` | ✅ |
| Remove (asks before forcing a running container) | `delete [--force]` | ✅ `d` |
| Remove with volumes (`docker rm -v`) | no flag: `delete`, then `volume delete` for its anonymous volumes | ✅ `D`, "purge" |
| Restart | no command; `stop` then `start` | ❌ |
| Shell into a container | `exec -it <id> sh` | ❌ (only in the help panel text) |
| Logs tab | `logs -f -n <lines>`, plus `--boot` | ❌ |
| Stats tab with graphs | `stats --format json` | ❌ |
| Env / Config / Top tabs | list JSON, `inspect`, `exec <id> ps` | ❌ |
| Image layer history | `image ls --format json` | ❌ |
| Volumes and Networks panels | `volume …`, `network …` | ❌ |
| Bulk actions and pruning | `stop --all`, `prune`, `image/volume/network prune` | ❌ |
| Filter a list (`/`) | handled in the app | ❌ (`/` opens help) |
| Open in browser | published ports in the list JSON | ❌ |
| Custom commands, config file | handled in the app | ❌ |
| Mouse click and scroll | handled in the app | drag-to-copy only |
| Screen modes, jump to panel by number | handled in the app | ❌ |
| Pause, Compose, attach to a running container | not in the CLI | out of scope |

lazy-container already does a few things lazydocker doesn't. It can start and
stop the container system itself, create a container from an image, and copy
screen text by dragging.

## Roadmap

### Tier 1: quick wins

- [x] **`d` delete** the selected container, asking y/n first. If it's
  running, offer `container delete --force`.
  - `D` deletes its anonymous volumes as well, as `docker rm -v` does. The
    app calls this purging, so `prune` stays free for the bulk menu's
    "remove every stopped container".
    `container delete` has no option for that, so the app runs
    `volume delete` for each one once the container is gone.
  - Like Docker, `D` keeps named volumes, and anonymous ones that another
    container still mounts. The dialog lists what goes and what stays.
- [ ] **`r` restart.** The CLI has no restart command, so run `stop` and then
  `start`.
- [ ] **`K` kill** with `container kill`, optionally choosing a signal with
  `--signal`.
- [ ] **`E` shell:**
  - Suspend the TUI: leave the alternate screen and turn off raw mode and
    **mouse capture**.
  - Run `container exec -it <id> sh` in the real terminal.
  - When it exits, restore the TUI and redraw everything.
  - Use `sh`, since many images don't include bash.
- [ ] **More container columns.** The `container ls --format json` output
  already carries the image, OS, architecture, CPUs, memory and start time,
  but `parse_containers` only keeps ID, state and IP. Hide the less important
  columns on narrow terminals.
- [ ] **`/` filters the focused list by name.** Help moves to `?`. The help
  panel then lists the app's own keys as well as the CLI cheat sheet.
- [ ] **`y` copies** the selected container's IP or ID, using the existing
  `copy_to_clipboard`.

### Tier 2: detail pane

This is the core of lazydocker's layout:
- Lists on the left (about a third of the width), tabs for the selected item
  on the right.
- `[` and `]` switch tabs, `Enter` moves focus to the pane so it can scroll,
  and `Esc` goes back to the list.

At 80 columns, image rows (76 columns, see the column widths in
`home_screen/mod.rs`) won't fit in a third-width side panel. Two ways out:
- The side lists show only name and state, and everything else moves into
  the pane.
- Keep today's stacked layout on terminals narrower than about 120 columns.

Container tabs:

- [ ] **Logs.** Stream `container logs -f -n 200 <id>` from the worker thread.
- [ ] **Boot.** `container logs --boot <id>` shows the VM's boot log, which
  Docker has no equivalent for.
- [ ] **Stats.** Poll `container stats --no-stream --format json` every couple
  of seconds. Draw CPU %, memory against its limit, network and disk I/O,
  and process count with `Sparkline` or `Chart`. The CPU formula is under
  [Useful CLI output](#useful-cli-output).
- [ ] **Top.** `container exec <id> ps`, if the image includes `ps`.
- [ ] **Env.** `configuration.initProcess.environment` from the list JSON.
- [ ] **Config.** `container inspect <id>`, pretty-printed.

Image tabs:

- [ ] **Layers.** `variants[].config.history` from `image ls --format json`.
  This is lazydocker's "ancestor layers" view.
- [ ] **Config.** Entrypoint, command, environment, working directory and
  labels from the same JSON, or from `container image inspect`.

### Tier 3: more resources and cleanup

- [ ] **Volumes panel:** list, create, delete, inspect and prune
  (`container volume …`).
- [ ] **Networks panel:** list, create, delete, inspect and prune
  (`container network …`).
- [ ] **`b` bulk menu** for each panel:
  - stop all (`stop --all`)
  - remove stopped containers (`prune`)
  - `image prune`, and `image prune --all`
  - `volume prune`
  - `network prune`

  Destructive entries ask first.
- [ ] **Disk usage in the panel titles**, from `system df --format json`,
  including reclaimable space. On the development machine it reported
  9.2 GiB of 9.9 GiB of images as reclaimable on 2026-09-17.
- [ ] **`p` pull an image** by reference, showing the CLI's progress output.
- [ ] **Fuller create dialog:** published ports (`-p`), environment (`-e`),
  volumes (`-v`), CPUs (`-c`), memory (`-m`) and `--rm`. Also a choice
  between creating only and starting as well (`run -d`).

### Tier 4: polish and Apple-only extras

- [ ] **Config file**, for example `~/.config/lazy-container/config.toml`,
  holding:
  - the refresh interval
  - how many log lines to load
  - colours
  - custom commands with `{id}` / `{image}` placeholders, like lazydocker's
    `customCommands`
- [ ] **Mouse:** click to focus a panel and select a row, wheel to scroll.
  `App::copy_selection` already tells a click from a drag.
- [ ] **Screen modes** with `+` / `_` (normal, half, full screen), as in
  lazydocker.
- [ ] **Number keys jump to a panel.** Shift+↑↓ gets awkward with more than
  two panels.
- [ ] **`w` opens `http://localhost:<port>`** in the browser, using the
  container's published ports.
- [ ] **More detail on the status screen:**
  - host and version details from `system status --format json`
  - recent `system logs` when the system fails to start
  - the image builder's state from `builder status`
- [ ] **Machines panel** for `container machine`: list, stop, delete, and a
  shell through `machine run`. lazydocker has nothing like it.
- [ ] **Copy and export files.** `container cp` copies files into or out of
  the selected container. `container export` saves its filesystem as a tar
  archive.

## Proposed keybindings

Existing keys keep their meaning, except help, which moves from `/` to `?`.

| Key | Where | Action | Status |
|---|---|---|---|
| `q` | everywhere | quit | existing |
| `Enter` | status screen | open the home screen | existing |
| `s` | status screen, containers | start or stop the system or the selected container | existing |
| `Esc` | home screen | close help, leave the detail pane, then go back to the status screen | existing, extended |
| `.` | containers | show all or running only | existing |
| `c` | images | create a container | existing |
| `Shift+↑↓` | home screen | switch panel | existing |
| `↑↓` | lists | move | existing |
| `?` | home screen | help | moved from `/` |
| `/` | lists | filter by name | new |
| `d` | containers | delete, after asking | existing |
| `D` | containers | purge: delete it with its anonymous volumes, after asking | existing |
| `d` | images | delete, after asking | new |
| `r` | containers | restart | new |
| `K` | containers | kill | new |
| `E` | containers | shell | new |
| `y` | containers | copy IP or ID | new |
| `Enter` | home screen | focus the detail pane | new |
| `[` `]` | home screen | previous / next tab | new |
| `b` | lists | bulk menu | new |
| `p` | images | pull | new |
| `w` | containers | open in browser | new |
| `+` `_` | home screen | next / previous screen mode | new |
| `1`–`9` | home screen | jump to panel | new |

## Useful CLI output

These are the field names `container` 1.4.1 prints. Later CLI releases may
change them.

**`container ls -a --format json`** returns one object per container:

- `id`, `status.state`, `status.startedDate`
- `status.networks[]`: `ipv4Address`, `ipv6Address`, `hostname`,
  `macAddress`, `network`
- `configuration.image.reference`,
  `configuration.platform.{os,architecture}`
- `configuration.resources.{cpus,memoryInBytes}`
- `configuration.initProcess`: `executable`, `arguments`, `environment`,
  `workingDirectory`
- `configuration.publishedPorts` and `configuration.labels`. These were
  empty on the sample container, so their names come from
  `ContainerConfiguration.swift` in apple/container.
- `configuration.mounts[]`: `destination`, `source`, `options`, and `type`,
  which is `{"volume": {"name": …, "format": …}}` for a volume,
  `{"virtiofs": {}}` for a bind mount, or `{"tmpfs": {}}`. A volume's
  `source` is its `volume.img` file.
- `configuration.creationDate`

**`container stats --no-stream --format json`** returns one object per
running container: `id`, `cpuUsageUsec`, `memoryUsageBytes`,
`memoryLimitBytes`, `networkRxBytes`, `networkTxBytes`, `blockReadBytes`,
`blockWriteBytes`, `numProcesses`.

`cpuUsageUsec` is a running total, so CPU % needs two samples:
`(cpu₂ − cpu₁) / (t₂ − t₁ in µs) × 100`. As with `docker stats`, the result
can pass 100 % for a container with several CPUs. Divide by
`configuration.resources.cpus` to get a 0–100 scale.

**`container image ls --format json`** returns one object per image:

- `id`, `configuration.name`, `configuration.creationDate`,
  `configuration.descriptor.{digest,size}`
- `variants[]`: `platform.{os,architecture,variant}`, `size`, `digest`
- `variants[].config.history[]`: `created`, `created_by`, `empty_layer`,
  `comment`
- `variants[].config.config`: `Env`, `Cmd`, `Entrypoint`, `WorkingDir`,
  `Labels`

**`container system df --format json`** returns `containers`, `images` and
`volumes`. Each has `total`, `active`, `sizeInBytes` and `reclaimable`.

**`container system status --format json`** returns `status`,
`client.{version,build,commit,appName}`, `server.{…}` (same fields),
`host.{operatingSystem,architecture,cpus}`, `paths.{appRoot,installRoot}`
and `resources.{containersRunning,containersTotal,images}`. The table output
uses different names for some of these, such as `host.os` and
`containers.total`.

**`container volume ls --format json`** returns one object per volume:
`id` (the name) and `configuration.{name,labels,source,sizeInBytes,creationDate}`.
There is no type field. An anonymous volume has a UUID name and the label
`com.apple.container.resource.anonymous`.

**Deleting** (checked in the 1.4.1 source and on the CLI):

- `container delete <id>` refuses a running container: `container <id> is
  running and can not be deleted`. A container that is still stopping can't
  be deleted, even with `--force`.
- `--force` kills a running container with SIGKILL, then deletes it. There is
  no graceful stop first.
- Deleting removes the container's directory: its disk, config and logs.
  Named and anonymous volumes stay, and so does the image.
- `volume delete a b` deletes nothing if any of the names doesn't exist. A
  volume that a container still mounts can't be deleted. In that case, the
  reason is on a log line before the `Error:` line.

**`container logs`** only accepts `--boot`, `-f` and `-n <lines>`. It has no
timestamp or `--since` options.

## Not possible with the CLI

`container` 1.4.1 has no command for these lazydocker features:

- **Pause and unpause.**
- **Compose projects and services.**
- **Attaching to a running container.** `start --attach --interactive` only
  works when starting a stopped container.
- **Health status.** `create` has no healthcheck options.
- **Log timestamps and a `since` window.** `logs` only takes a line count.

## Suggested order

1. All four [foundations](#foundations).
2. The detail-pane layout with the Logs tab.
3. `d` delete and `E` shell.
4. The rest of Tier 1, then Tiers 2, 3 and 4.

## References

- [lazydocker README](https://github.com/jesseduffield/lazydocker)
- [lazydocker keybindings](https://github.com/jesseduffield/lazydocker/blob/master/docs/keybindings/Keybindings_en.md)
- [lazydocker configuration](https://github.com/jesseduffield/lazydocker/blob/master/docs/Config.md)
- [apple/container](https://github.com/apple/container), especially
  `Sources/ContainerResource/Container/ContainerConfiguration.swift`
- `container <command> --help` for CLI 1.4.1
