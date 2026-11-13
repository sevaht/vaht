# vaht design

For maintainers and AI agents (General D10). The design was settled in the
documents under `docs/design/`, and this file points into them rather than
repeating them.

| Document | Covers |
|---|---|
| [00 Feature inventory](docs/design/00-feature-inventory.md) | Every feature fluxland has, with its verdict for vaht; **decisions so far**; the **glossary** (N6) |
| [01 Object model](docs/design/01-object-model.md) | Crates, the ownership tree, windows, outputs, the panel system, input, commands, scene composition |
| [02 Configuration format](docs/design/02-config-format.md) | The TOML files, commands, matchers, bindings, rules, menus, outputs, themes, `vaht import` |
| [03 Resources and logging](docs/design/03-resources-logging.md) | Ownership guarantees, leak detection, logging and rotation |
| [04 Smithay mapping](docs/design/04-smithay-mapping.md) | What Smithay provides, what vaht implements, games and media, spike results |

## Workspace

| Crate | Role |
|---|---|
| `crates/vaht` | The one binary and its subcommands |
| `crates/vaht-compositor` | The compositor |
| `crates/vaht-ui` | The panel toolkit; no Wayland |
| `crates/vaht-ipc` | IPC message types |
| `crates/vaht-import` | `vaht import fluxbox`: Fluxbox (and fluxland) setups to vaht's TOML |
| `crates/vaht-test-support` | Test-only: a headless vaht, a client that clicks and takes screenshots |

The remaining crates in 01 §2 (`vaht-dialog`, `vaht-menu-gtk`) are added
when work on them starts.

## Testing

`crates/vaht-test-support` runs the real binary with `--backend headless`
(Smithay's software renderer, frames in memory) and private runtime,
configuration and log directories. Its `TestClient` is an ordinary Wayland
client: it opens solid-colored windows, moves and clicks a virtual pointer
(`zwlr_virtual_pointer_v1`), types on a virtual keyboard
(`zwp_virtual_keyboard_v1`), and takes screenshots through
`ext-image-copy-capture`, the protocol `grim` uses. Nothing reaches the
developer's session, so tests can drag and click freely.

- `crates/vaht/tests/headless.rs`: leak tests. After clients come and go, vaht
  is stopped with SIGTERM and its exit report must show no windows, frames,
  clients or capture sessions, and as many open descriptors as at startup.
  They found two real leaks: clipboard focus keeping a gone client alive, and
  Smithay's capture session list, which grows unless pruned.
- `crates/vaht/tests/bindings.rs`: key and mouse bindings, with their own
  `bindings.toml`; the bound programs write files the tests read. They found
  that programs inherited vaht's blocked signals.
- `crates/vaht/tests/ui.rs`: UI tests. Positions come from the screenshot
  (the window's fill color), clicks go through the virtual pointer, and
  results are checked in pixels.

`wtype` and `grim` work against a headless vaht too, for looking by hand:
run `vaht --backend headless` with `XDG_RUNTIME_DIR` set to a short private
directory.

## Where behavior lives

The coding standards' modeling rules apply throughout (General M1–M16,
RUST.md §4). Behavior lives with the data it governs: `Window` moves and
resizes itself (`WindowDrag`), `Frame` presses its buttons, `InputState`
decides whose a key is, `MouseContext` knows where a press looks for
bindings, `Scene` composes what an output shows and hit-tests it,
`ConfigFile` reads itself. `Placement` is a value: maximizing, fullscreen
and the snap ladder are pure functions from one placement to the next, and
`Window::place` applies the result. `Workspaces` and `Focus` (focus history
and Alt+Tab cycling) are entities that decide; the compositor carries out
their decisions with the seat.

**One deliberate exception to R-M2 (one `impl` block per type):**
`Compositor` is Smithay's state type, and Smithay's seat, grab and protocol
calls all need `&mut Compositor`, so the use cases that make those calls
cannot be methods of the subsystems. They are `Compositor` methods, in one
`impl Compositor` block per concern: `compositor.rs` (lifecycle, focus,
windows coming and going), `input.rs`, `grab.rs`, `execute.rs`,
`capture.rs`, `protocols/xdg_shell.rs` and `backend/winit.rs`. Those blocks
only coordinate: each decision they need is asked of the type that owns it.
A method that needs no Smithay call does not belong in them.

## Invariants that are easy to break

- Only `Windows` owns a `Window`; everything else holds a `WindowId` (01 §1).
- Only `window/toplevel.rs` knows whether a window is Xdg or X11 (01 §4.1).
- Only `Outputs::apply` changes output modes (01 §5.1).
- Only `Launcher` spawns processes (03 §2).
- Every event-loop registration is an `EventSource` guard (03 §2); clippy
  forbids calling `insert_source` anywhere else.
- `ProcessSignals::block()` runs at the top of `main`, before any thread
  starts. A thread started earlier would leave SIGTERM unblocked and die with
  it, skipping the clean stop, or swallow a SIGCHLD and leave a zombie.
- Programs are started only by `Launcher`, with `posix_spawn`, never with
  `std::process::Command`: the child must get an empty signal mask, and
  `Command` passes on vaht's blocked one (03 §2).
- Commands are parsed once, into `Command` values (01 §8), and run only by
  `Compositor::execute`. `Action::parse` reads both the string and the table
  form, so they cannot disagree.
- `start-moving` and `start-resizing` from a drag binding run inside a
  pointer grab, where a new grab cannot be set: they turn the held grab into
  a move or resize through `ExecutionContext::grab` instead.
- The Wayland `Display` lives beside the `Compositor`, not inside it, so
  clients are dispatched without `unsafe` (R-X3).
- After clients release surfaces or capture sessions,
  `Compositor::release_dead_client_resources` prunes Smithay's capture session
  list and the renderers' caches, and schedules a frame: a gone client's last
  buffers are let go when that frame is drawn.
- Drawing and hit-testing go through `Scene`, which shows only the windows
  of the current workspace (not minimized), without frames when
  fullscreen and without content when shaded. Nothing else walks the
  stack to find what is under the pointer.
- A new window is placed on its client's first commit with a size
  (`Compositor::place_new_window`), not when it is created: the
  placement strategies need its size. Until then it counts for nothing.
- A window's area follows from its `Placement`: code that maximizes, tiles
  or fullscreens a window builds a new `Placement` and calls
  `Window::place`, never moves or sizes it directly.
- Clipboard focus is set together with keyboard focus in
  `Compositor::focus_window`: Smithay clears the focus of a destroyed
  surface silently, and the clipboard would otherwise keep a disconnected
  client's state alive.
