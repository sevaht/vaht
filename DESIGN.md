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

The remaining crates in 01 §2 (`vaht-dialog`, `vaht-import`, `vaht-menu-gtk`)
are added when work on them starts.

## Invariants that are easy to break

- Only `Windows` owns a `Window`; everything else holds a `WindowId` (01 §1).
- Only `window/toplevel.rs` knows whether a window is Xdg or X11 (01 §4.1).
- Only `Outputs::apply` changes output modes (01 §5.1).
- Only `Launcher` spawns processes (03 §2).
- Every event-loop registration is an `EventSource` guard (03 §2).
