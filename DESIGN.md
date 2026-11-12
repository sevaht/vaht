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
| `crates/vaht-test-support` | Test-only: a headless vaht, a client that clicks and takes screenshots |

The remaining crates in 01 §2 (`vaht-dialog`, `vaht-import`, `vaht-menu-gtk`)
are added when work on them starts.

## Testing

`crates/vaht-test-support` runs the real binary with `--backend headless`
(Smithay's software renderer, frames in memory) and private runtime and log
directories. Its `TestClient` is an ordinary Wayland client: it opens
solid-colored windows, moves and clicks a virtual pointer
(`zwlr_virtual_pointer_v1`), and takes screenshots through
`ext-image-copy-capture`, the protocol `grim` uses. Nothing reaches the
developer's session, so tests can drag and click freely.

- `crates/vaht/tests/headless.rs`: leak tests. After clients come and go, vaht
  is stopped with SIGTERM and its exit report must show no windows, frames,
  clients or capture sessions, and as many open descriptors as at startup.
  They found two real leaks: clipboard focus keeping a gone client alive, and
  Smithay's capture session list, which grows unless pruned.
- `crates/vaht/tests/ui.rs`: UI tests. Positions come from the screenshot
  (the window's fill color), clicks go through the virtual pointer, and
  results are checked in pixels.

`wtype` and `grim` work against a headless vaht too, for looking by hand:
run `vaht --backend headless` with `XDG_RUNTIME_DIR` set to a short private
directory.

## Invariants that are easy to break

- Only `Windows` owns a `Window`; everything else holds a `WindowId` (01 §1).
- Only `window/toplevel.rs` knows whether a window is Xdg or X11 (01 §4.1).
- Only `Outputs::apply` changes output modes (01 §5.1).
- Only `Launcher` spawns processes (03 §2).
- Every event-loop registration is an `EventSource` guard (03 §2); clippy
  forbids calling `insert_source` anywhere else.
- `TerminationSignals::block()` runs at the top of `main`, before any thread
  starts. A thread started earlier would leave SIGTERM unblocked and die with
  it, skipping the clean stop.
- The Wayland `Display` lives beside the `Compositor`, not inside it, so
  clients are dispatched without `unsafe` (R-X3).
- After clients release surfaces or capture sessions,
  `Compositor::release_dead_client_resources` prunes Smithay's capture session
  list and the renderers' caches, and schedules a frame: a gone client's last
  buffers are let go when that frame is drawn.
- Clipboard focus is set together with keyboard focus in
  `Compositor::focus_window`: Smithay clears the focus of a destroyed
  surface silently, and the clipboard would otherwise keep a disconnected
  client's state alive.
