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
RUST.md §4). The compositor crate has two halves:

- **The window manager** (`window_manager/`, `WindowManager`) decides
  everything about windows, workspaces and focus. It holds no protocol
  object and never calls Smithay's seat: clients' commits arrive as
  `CommittedState` values and their requests as `ClientRequest`s, bindings
  run `WindowManagerAction`s, and what it decides goes out as
  `ClientUpdates` (each window's `RequestedState`, and `Ending`s) and as
  its choice of focused window. Its rules are unit- and property-tested on
  their own (`window_manager.rs`, `tests`): a random sequence of opens,
  closes, actions, client requests, drags and panels must keep every
  invariant (focus only on a shown window, every window stacked once, on
  a workspace that exists, maximized and tiled windows within the usable
  area).
- **The compositor** (`Compositor`) is Smithay's state type. It owns the
  window manager and the gateways: `Toplevels` (each window's xdg-shell
  object, the only code that speaks the protocol to a window),
  `FrameTextures` (frame pixels as the renderer takes them), `Layers`,
  `Input` (key filtering, button presses, double clicks, repeat and
  auto-raise timers), the `Taskbar` (a panel whose content it reads from
  the window manager and the clock as frames are prepared),
  the backend and the protocol states. It routes events and carries out
  the window manager's decisions; it decides nothing about windows.

Within the window manager, behavior lives with the data it governs:
`Window` moves and resizes itself (`WindowDrag`) and keeps what vaht asks
apart from what its client committed, `Frame` presses its buttons,
`Placement` is a value (maximizing, fullscreen and the snap ladder are pure
functions from one placement to the next, and `Window::place` applies the
result), `Workspaces` and `Focus` (focus history and Alt+Tab cycling) are
entities, `PlacementStrategy` and `EdgeSnap` are pure. Elsewhere, `Input`
decides whose a key is and what a button press does (a `PressOutcome`:
the panel's click, or focus and raise, then a titlebar button, a binding,
a border resize or the client's press), and `Bindings` what a wheel turn
runs, both tested without a seat; `MouseContext` knows where a press looks
for bindings, `Scene` composes
what an output shows and hit-tests it, and `ConfigSource` reads and watches
the configuration.

**Why `Compositor` is large, in one `impl` block (R-M2).** Smithay's seat
calls (`KeyboardHandle::set_focus`, pointer motion and buttons, grabs,
keymap changes) take `&mut Compositor` and call back into its handler
traits, so code that makes them cannot run inside a subsystem's method
while that subsystem is borrowed. Those calls, and the routing around them,
are `Compositor` methods; everything else is a method of the type that owns
the data. The handler traits (`protocols/`, `capture.rs`) are separate
trait impls, which R-M2 allows.

**Settling.** The window manager's decisions are carried out in one place,
`Compositor::settle`: after every event-loop round and every client
dispatch, before an input event reaches a client (so a key typed right
after a binding moved focus goes where focus went), and after a window's
commit (so its first configure carries what was asked). Nothing else sends
configures for windows or sets keyboard focus.

## Invariants that are easy to break

- Only `Windows` owns a `Window`; everything else holds a `WindowId` (01 §1).
- Only `toplevels/toplevel.rs` knows whether a window is Xdg or X11, and
  only it speaks the protocol to a window (01 §4.1).
- Nothing under `window_manager/` uses Wayland protocol objects, the seat,
  or a renderer; a `Window` and its `Toplevel` are added and removed
  together, by `Compositor::add_window` and `Compositor::remove_window`.
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
  `Compositor::execute`, which hands every `WindowManagerAction` to
  `WindowManager::run`. `Action::parse` reads both the string and the table
  form, so they cannot disagree.
- `start-moving` and `start-resizing` from a drag binding run inside a
  pointer grab, where a new grab cannot be set: they turn the held grab into
  a move or resize through `ExecutionContext::grab` instead.
- The Wayland `Display` lives beside the `Compositor`, not inside it, so
  clients are dispatched without `unsafe` (R-X3).
- After clients release surfaces or capture sessions, or leave,
  `Compositor::release_dead_client_resources` prunes Smithay's capture session
  list and the renderers' caches, and schedules a frame: a gone client's last
  buffers are let go when that frame is drawn.
- When the topmost window shown is fullscreen, the `Scene` draws and
  hit-tests neither the top layer nor the taskbar: the window covers them,
  and only the overlay layer is above it (01 §9).
- Drawing and hit-testing go through `Scene`, which shows only the windows
  of the current workspace (not minimized), without frames when
  fullscreen and without content when shaded, between the layer surfaces:
  overlay and top above, bottom and background below. Nothing else walks
  the stack to find what is under the pointer.
- The usable area (where windows maximize and tile) is what layer
  surfaces' exclusive zones leave free, from Smithay's `LayerMap`
  (`OutputLayers::usable_area`). A click on a layer below the windows (a
  wallpaper) counts as the desktop for bindings; a click on one above (a
  panel) is its client's alone.
- A new window is placed on its client's first commit with a size
  (`WindowManager::commit`), not when it is created: the placement
  strategies need its size. Until then it counts for nothing.
- Areas follow what the client committed, never what vaht asked: a
  client may take a while to apply a size, or not apply it exactly.
- Maximized, tiled and fullscreen windows follow the output's areas:
  `WindowManager::set_output_area` places them again when a panel comes or
  goes.
- A window's area follows from its `Placement`: code that maximizes, tiles
  or fullscreens a window builds a new `Placement` and calls
  `Window::place`, never moves or sizes it directly.
- The dmabuf global exists only when the backend's renderer imports GPU
  buffers (`Backend::dmabuf_formats`). Advertising it without that would
  make GL clients fail instead of falling back to shared memory.
- Clipboard and primary selection focus are set together with keyboard
  focus in `Compositor::sync_keyboard_focus`, which compares with the focus
  it last set rather than the seat's: Smithay clears the focus of a
  destroyed surface silently, and the clipboard would otherwise keep a
  disconnected client's state alive.
