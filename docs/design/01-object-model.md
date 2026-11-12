# 01 — Object model

The types vaht is made of: who owns what, and where each feature from
`00-feature-inventory.md` lives. This is a draft for discussion. Type
names follow the glossary in 00 (N6). Rule IDs refer to the coding
standards.

---

## 1. Principles that shape everything below

1. **Ids, not references, across the object graph.** A window is owned in
   exactly one place, the `Windows` arena. Everything else (stacking order,
   focus history, the taskbar's window list, grabs, marks, IPC subscriptions)
   holds a `WindowId`, a `Copy` newtype. Looking up a closed window returns
   `None`; it can never dangle. This removes fluxland's use-after-free class
   (00 §1.5) and makes leaks checkable: holders can be asserted to hold only
   live ids (see 03).
2. **Gateways translate, coordinators decide** (M4, M12).
   - The Smithay handler traits are gateways. They turn protocol events into
     calls on domain types and contain no policy.
   - Policy (placement, focus, rules) lives in plain Rust types with no Smithay
     in their signatures where that's practical, so they can be unit tested
     without a compositor (T4).
3. **One panel system draws everything the compositor draws.** A window frame,
   the taskbar, a menu, a tooltip, a dialog and the snap picker are all
   *content* described in widgets. One `Panel` type lays them out, paints
   them, hit-tests them, and tracks hover, pressed state and tooltips.
   Content types contain **no** drawing and **no** hit-testing code
   (00 §1.1).
4. **Closed sets are enums** (M8, R-M10): placement state, actions, theme
   elements, widget kinds, pointer targets, grabs, backends. Matches are
   exhaustive, so a new variant makes the compiler list every place that must
   handle it.
5. **Configuration is parsed once at the boundary** (M3, M6). TOML becomes a
   validated `Config` value. Reloading builds a new one, and the compositor
   applies the difference.

---

## 2. Crates (Cargo workspace, R-T4)

| Crate | Kind | Contents | Depends on |
|---|---|---|---|
| `vaht-ui` | library | Panel toolkit: widgets, layout, `Canvas` (tiny-skia), `Typesetter` (cosmic-text), `Theme` model. **Knows nothing about Wayland or windows** (M14) | tiny-skia, cosmic-text, image, resvg |
| `vaht-ipc` | library | IPC message types (serde) plus a JSON Schema export (schemars), and a small blocking client | serde, serde_json |
| `vaht-compositor` | library | The compositor | smithay, vaht-ui, vaht-ipc, zbus, toml, toml_edit |
| `vaht-dialog` | library | Text dialogs as ordinary Wayland client windows: run, command, set title, rename workspace | vaht-ui, vaht-ipc, smithay-client-toolkit |
| `vaht-import` | library | Converts fluxbox and fluxland setups to vaht TOML | vaht-compositor's `config` types |
| `vaht-menu-gtk` | library, optional cargo feature | GTK menu helper (§6.4), run as `vaht menu-helper gtk`. **The only crate that depends on GTK.** After the first milestone | gtk4, gtk4-layer-shell, vaht-ipc |
| `vaht` | **the one binary** | Subcommands (R-A3): `vaht` (run the compositor), `vaht msg …` (IPC client), `vaht dialog run\|command\|title\|workspace-name`, `vaht import`, `vaht check` | all of the above, clap |

**One binary, several libraries.** There is one thing to install and one
version to match, and the compositor runs its own dialogs as
`vaht dialog …` (as niri does with `niri msg`). The code stays separated by
crate.

**Dialogs are client windows (decided).**
- They are framed and focused, appear in the taskbar, and follow rules like any
  other window.
- They act through IPC; a dialog for a window receives its `WindowId`.
- The compositor needs no text-field widget, no dialog key handling, and no
  "internal window" backend (fluxland's `view_internal.c`).

Keeping `vaht-ui` separate means panels can be rendered to PNG in ordinary
tests and checked as snapshots (`insta`, R-TEST4), with no compositor running.
The dialogs reuse it, so they look like the rest of the desktop.

---

## 3. The compositor state and its ownership tree

Smithay requires one state type that implements all its handler traits. vaht
accepts that, but the type only **holds** subsystems and routes between them.
It is not where behavior lives.

```
Compositor                      (the Smithay state; coordinator of last resort)
├── protocols: ProtocolStates   Smithay's *State globals. Mechanical; no logic
├── backend: Backend            enum { Drm(DrmBackend), Winit(WinitBackend) }
├── outputs: Outputs            Output entities, layout, primary output
│   └── Output                  identity, mode, usable area, FrameClock, VrrPolicy,
│       ├── layers: LayerMap    layer-shell surfaces (other bars, launchers, swaybg)
│       ├── taskbar: Option<Taskbar>
│       └── wallpaper: Option<Wallpaper>
├── profiles: DisplayProfiles   named layouts, automatic choice, hooks
├── windows: Windows            arena WindowId → Window, plus Stack (layers)
│   └── Window                  Toplevel, Frame, Placement, WindowState, settings
├── workspaces: Workspaces      names, current index, membership queries
├── focus: Focus                MRU history, focus model, FocusCycle, activation policy
├── input: Input                Seat, KeyBindings + KeyChain, MouseBindings, Grab
├── overlays: Overlays          menus (MenuStack), tooltip, snap picker,
│                               wireframe, position overlay
├── config: Config              the validated configuration, plus runtime overrides
├── theme: Theme                (vaht-ui) element × state → Style
├── typesetter: Typesetter      (vaht-ui) the one font system, lent to panels while painting
└── services
    ├── ipc: IpcServer          socket listener, IpcConnection per client
    ├── tray: Tray              StatusNotifierWatcher/Host on a zbus thread, bridged by channel
    ├── xwayland: Xwayland      X11Wm, Xresources, RandR primary, InputEmulation (libei)
    ├── launcher: Launcher      spawns children, environment overrides, autostart
    └── session: Session        lock, idle, activation environment export
```

Methods on `Compositor` exist only for use cases that span several
subsystems: "map a window", "apply a profile", "execute a command". Each one
reads as a short sequence of calls on the fields. Rust lets a method borrow
disjoint fields mutably at the same time
(`self.focus.activate(&mut self.windows, id)`), so subsystems never need
back-pointers to each other.

---

## 4. Windows

### 4.1 `Window` (entity)

```rust
pub struct Window {
    id: WindowId,
    toplevel: Toplevel,          // the client's side: gateway over Xdg or X11
    frame: Frame,                // what vaht draws around it
    placement: Placement,        // where it is and why (one enum, §4.2)
    state: WindowState,          // independent flags: shaded, minimized, urgent, sticky
    layer: Layer,                // Desktop, Below, Normal, Above
    workspace: WorkspaceIndex,   // ignored while sticky
    settings: WindowSettings,    // from rules: alpha, decorations, focus protection,
                                 //   pseudo-transparency, hidden from window list
    last_output: Option<OutputIdentity>, // returns here when that output comes back
}
```

**`Toplevel`** is the one place that knows about xdg-shell and X11 (rung 2 of
R-M15: an enum of the two). It wraps `smithay::desktop::Window`, which already
unifies the two surface kinds, and adds what fluxland's `wm_view_impl` had:
`request_size`, `set_activated`, `set_tiled_edges`, `close`, `size_hints`,
`title`, `app_id`, `pid`, `parent`, `x11_property`. Nothing outside
`window/toplevel.rs` matches on Xdg versus X11 (fluxland's "never touch
`xdg_toplevel` outside the backends" rule, enforced by module privacy).

### 4.2 `Placement` replaces fluxland's flag pile

```rust
pub enum Placement {
    Floating { geometry: Rectangle },
    Maximized { axes: Axes, restore: Rectangle },        // Axes: Horizontal | Vertical | Both
    Tiled { cell: TileCell, output: OutputId, restore: Rectangle },
    Fullscreen { output: OutputId, underneath: Box<Placement> },
}
```

- `LHalf`, `RHalf`, `TileTo`, the layout picker and `SnapStep` all produce a
  `Tiled` value. A `TileCell` is a fraction of an output's usable area.
- `SnapStep` is a pure function `(Placement, Direction) → Placement`, so it can
  be tested exhaustively.
- Leaving fullscreen restores `underneath` exactly.
- A window can never be "maximized and half-tiled at once".
- Rules set the initial `Placement` before the window maps. The geometry change
  always follows from the variant, which fixes fluxland's "setting the flag is
  not enough" pitfall.

### 4.3 `Frame` (content for the panel system)

The frame is **not** one big buffer around the window, which would cost about
8 MB per 1080p window. It is assembled from:

- a **titlebar** panel strip: buttons, label, buttons;
- a **handle** panel strip with its grips (optional, as in my style);
- **borders**, drawn as solid-color render elements with no buffer.

The frame decides only *what* goes in each strip, from window state and
settings. Painting, hover and pressed state, and clicks are handled by `Panel`.
A click on a frame widget yields a `FrameHit` (`Button(TitlebarButton)`,
`Label`, `Handle`, `Grip(Side)`, `Border(Edges)`), which input turns into a
binding context (§7).

### 4.4 `Windows` and `Stack`

- **`Windows`** owns the arena: a slot map keyed by `WindowId`. The id doubles
  as the IPC window id, and ids are never reused.
- **`Stack`** holds the order within each `Layer`. It is the single source of
  stacking order, so raise and lower are methods on it.
- **Rendering and hit-testing** walk the `Stack` top-down. vaht does not use
  Smithay's `Space`: its single z-order and element model duplicate what
  `Stack` and the scene already own (P5).

### 4.5 Rules and matching

- **`WindowMatcher`** is the typed form of Fluxbox client patterns. It matches
  on app id, class, instance, title, role, tag, workspace, layer, state and
  output, with exact or regex values and negation. It is parsed once.
- **`Rules`** is an ordered list of `Rule { matcher, settings, placement }`.
- **`Rules::settings_for(&Window) → WindowSettings`** runs once, at first map.

---

## 5. Outputs, profiles, workspaces

### 5.1 `Output` (entity)

- `identity: OutputIdentity`: make, model and serial, plus the connector name.
  Configuration and profiles match on this (00 decision).
- `smithay: smithay::output::Output`: the global clients see.
- `usable_area`: the output's area minus exclusive zones (taskbar, layer-shell
  panels). Placement and maximize read it.
- `frame_clock: FrameClock`: the redraw state machine
  (`Idle → Queued → AwaitingVblank`). It renders only when something is damaged
  or a client has committed, never on a fixed tick, which VRR requires.
- `vrr: VrrPolicy`: `Off`, `On` or `Fullscreen`. It is evaluated each frame
  from what the scene shows (fullscreen with content type game or video) and
  applied with `DrmCompositor::use_vrr`.
- `taskbar: Option<Taskbar>` and `wallpaper: Option<Wallpaper>`. Both are per
  output from day one; the multi-output taskbar feature is a configuration
  question, not a refactor.

**`Outputs`** owns all outputs, the global layout and `primary: Option<OutputId>`.
It has a single `apply(OutputConfiguration)` path. Startup config, display
profiles and the wlr-output-management protocol all go through it, so there is
one place that performs modesets.

### 5.2 `DisplayProfiles` (coordinator)

- It holds `Profile { name, outputs: Vec<(OutputMatch, OutputSettings)>, primary_candidates }`.
- `choose(connected) → Option<&Profile>` picks automatically from the set of
  connected outputs.
- `cycle()` steps to the next valid profile, and `cycle_primary()` to the next
  primary candidate.
- After an apply, it hands a `ProfileApplied` value to the `Launcher`, which
  runs the hook command with JSON on standard input.

### 5.3 `Workspaces`

- It holds the names, `current: WorkspaceIndex` and `previous`.
- Membership is the window's own field, so `Workspaces` answers queries through
  `windows`. It does not keep a second list (P5).
- Workspaces are **global**, shared by all outputs, as in Fluxbox (decided).
  To leave room for per-output workspaces at no cost, callers always ask
  `workspaces.current_on(output)` and never read `current` directly. Today the
  method ignores the output. Making workspaces per output later would then
  change only `Workspaces` and the taskbar's workspace widgets.

---

## 6. The panel system (`vaht-ui`), the heart of the deduplication

### 6.1 Types

| Type | Role | Responsibility |
|---|---|---|
| `Widget` | value | One node of a panel: `kind: WidgetKind`, `element: ThemeElement`, `size: SizePolicy`, `hit: Option<HitId>`, `tooltip: Option<String>` |
| `WidgetKind` | enum | `Text(String)`, `Glyph(Glyph)`, `Icon(ImageRef)`, `Fill`, `TextField(TextFieldState)` (used only by `vaht-dialog`), `Row(Vec<Widget>)`, `Column(Vec<Widget>)`, `Grid { rows, children }` |
| `SizePolicy` | enum | `Fixed(px)`, `Fit` (measured), `Fill { weight }` |
| `PanelContent` | trait | Implemented by Frame strips, Taskbar, Menu, Tooltip and SnapPicker (and by `vaht-dialog`'s windows): `fn build(&self) -> Widget`, plus an associated `Hit` type that `HitId` maps back to |
| `Panel<C: PanelContent>` | entity | Owns its content, size, laid-out tree, `Canvas`, hover and pressed `HitId`, and a dirty flag. `layout`, `paint_if_dirty`, `hit_at(point) → Option<C::Hit>`, `tooltip_at`, `pointer_motion`, `press`, `release` |
| `Canvas` | entity | A tiny-skia pixmap. Paints textures, text, glyphs and icons for a laid-out tree |
| `Typesetter` | entity | The one cosmic-text `FontSystem` plus glyph cache. Measures and shapes text; creating one takes 55 ms, so it is shared |
| `Theme` | value | `style(ThemeElement, WidgetState) → &Style`, with the cascade resolved at load |
| `Style` | value | `texture: Texture`, `text: TextStyle`, `border: Border`, `padding: Insets` |
| `Texture` | enum | `Solid(Color)`, `Gradient { direction, from, to, bevel, interlaced }`, `Image { path, fit }`, `ParentRelative` |
| `TextStyle` | value | `font: FontSpec`, `color`, `effect: TextEffect` (`None`, `Shadow`, `Halo`), `justify` |

`ThemeElement` is one enum covering every themable thing: `TitlebarLabel`,
`TitlebarButton`, `Handle`, `Grip`, `TaskbarBackground`, `WindowListEntry`,
`Clock`, `MenuTitle`, `MenuItem`, `Tooltip`, `DialogField` and so on.
`WidgetState` is `Focused`, `Unfocused`, `Hovered`, `Pressed`, `Active` or
`Disabled`. Together they replace fluxland's roughly 150-field `wm_style` with
one lookup.

### 6.2 Data flow

1. Domain state changes, for example a window title. The owner marks its panel
   content stale: `panel.invalidate()`.
2. Before the next frame of each output it appears on, the panel rebuilds the
   tree (`content.build()`), lays it out (measuring text through the
   `Typesetter`), paints it into the `Canvas`, and uploads it to its
   `MemoryRenderBuffer` with damage limited to the widgets that changed.
3. Each frame, the scene asks the panel for a render element. If nothing
   changed, its commit counter is unchanged and the damage tracker skips it.
   The spike confirmed this.
4. Hover and pressed changes are panel-internal. They repaint without
   rebuilding.

The bridge from `Canvas` to `MemoryRenderBuffer` sits in `vaht`
(`scene/panel_element.rs`), which keeps `vaht-ui` free of Smithay.

### 6.3 Where the new taskbar features fit

| Feature | Mechanism |
|---|---|
| Several rows | The taskbar builds the window list and tray as `Grid { rows: n }`. Layout wraps entries row by row; nothing else changes |
| Taller clock | `Clock` picks a `ClockFormat` by the height it was given (one line, or time over date). It is a `build()` decision |
| Taskbar on all outputs | `taskbar.outputs = "primary" \| "all" \| [matches]` decides which `Output`s get a `Taskbar` |
| Window list per output | `WindowListScope::{AllOutputs, ThisOutput}` plus a `WindowMatcher` filter |
| Slit later | Taskbar placement is `EdgeDock { edge, alignment, size, auto_hide, layer, reserves_space }`. A slit would be another `PanelContent` in an `EdgeDock` |

### 6.4 Menus: one model, interchangeable presenters

A menu's *content* is separate from *how it is shown*, so a toolkit menu can
be swapped in without touching the root menu, the window menu or the tray.

- **`MenuModel`** (value) is the backend-neutral tree. Each item has an id,
  label, icon, enabled flag, check or radio state, and a submenu that can be
  loaded lazily (dbusmenu's `AboutToShow`). It is built by the root menu
  config, the window menu and `Tray` (from dbusmenu) alike.
- **`MenuPresenter`** (enum, rung 2) has two variants:
  - **`BuiltIn`**: a `Panel<MenuContent>` stack in `Overlays`, using vaht's
    theme, with keyboard navigation and type-to-search. This is the default and
    is in the first milestone.
  - **`External(MenuHelper)`**: a helper process (`vaht menu-helper gtk`,
    later perhaps `qt`) that shows the model with a real toolkit.
- Both report a `MenuOutcome` (`Chosen(ItemId)`, `Dismissed`, or
  `SubmenuOpened(ItemId)` for lazy loading), which the menu's owner turns into
  a `Command` or a dbusmenu event.

**The helper protocol is JSON lines over the helper's standard input and
output**: `show { model, position, output }`, `update`, `close`, and replies
`chosen` / `dismissed` / `opened`. Anything that speaks it can be a presenter,
so Qt later means a new helper binary and **no** compositor change.

**How the GTK helper works.** It is a GTK 4 client using `gtk4-layer-shell`.
For each menu it opens a transparent overlay-layer surface at the pointer and
pops a `GtkPopoverMenu` from it. This is the same approach waybar uses for
tray menus. It stays running between menus, because GTK startup takes 100 ms
or more.

**Cost and risk.**
- It is moderate work: a few hundred lines in its own crate, `vaht-menu-gtk`.
  It is the only crate that depends on GTK, so a build without it has no GTK
  dependency at all.
- **Where bugs are likely:**
  - keyboard focus moving to the helper's surface and back;
  - dismissing the menu on outside clicks;
  - placement near output edges;
  - round trips for lazily loaded submenus.

  vaht handles each of these generically, but they need testing on real tray
  applications.
- **Plan:** the `MenuModel` / `MenuPresenter` split goes in from the start. The
  GTK helper comes after the first milestone. `menus.presenter = "built-in" |
  "gtk"`, and if the helper fails to start or crashes, vaht logs it and falls
  back to `built-in`, so a menu always appears.

---

## 7. Input

- **`Input`** owns the Smithay `Seat` (keyboard, pointer, touch, tablet),
  `KeyBindings`, `MouseBindings`, `KeyChain` (chain and keymode state), key
  repeat for `Repeat` bindings, and the active `Grab`.
- **`PointerTarget`** is computed once per motion event by walking the scene
  top-down:
  `Window { id, part: WindowPart }` (`Content` or `Frame(FrameHit)`),
  `Panel { kind, hit }`, `LayerSurface`, `Popup`, or `Desktop`.
  Focus, bindings, cursor shape and tooltips all read this value; none of them
  hit-tests on its own.
- **`KeyboardTarget`** is an enum: `Window(WindowId)`, `LayerSurface`,
  `LockSurface`, `Popup` or `Menu`. It is the single record of where keys go.
  Smithay's `KeyboardTarget` trait is implemented on it, so focus can never
  point at something that is not one of these.
- **`BindingContext`** (`Desktop`, `Taskbar`, `Titlebar`, `Window`, `Grip`,
  `Border`, `WindowListEntry`…) derives from `PointerTarget`. This is how the
  Fluxbox mouse contexts are expressed.
- **`Grab`** is an enum: `Move { id, start, snap: SnapTracking }`,
  `Resize { id, edges, start }` and `Menu`. It implements Smithay's
  `PointerGrab` in one place. Opaque versus wireframe is a field, not a
  separate code path.
- **`KeyBindings`** maps `(KeyMode, KeyChord)` to a `Command`. Chains are a
  tree, and `KeyChain` holds the current node.
- The taskbar never takes keyboard focus (00 decision). An open menu receives
  keys through `Overlays`. Dialogs are ordinary client windows, so they get
  keys like any other window.

---

## 8. Commands and actions

```rust
pub enum Command {
    Run(Action),
    Sequence(Vec<Command>),                       // MacroCmd
    Toggle { id: ToggleId, commands: Vec<Command> },   // ToggleCmd; position kept in Input
    If { condition: Condition, then: Box<Command>, otherwise: Option<Box<Command>> },
    ForEach { matcher: WindowMatcher, command: Box<Command> },
    Delay { command: Box<Command>, duration: Duration },
    Repeat { command: Box<Command>, rate: Option<RepeatRate> },
}
```

- **`Command` is an immutable value parsed from config.** Runtime state lives
  outside it: a toggle's position is kept by `Input` in
  `toggle_positions: HashMap<ToggleId, usize>`, so reloading config resets it
  cleanly.
- **`Action`** is one enum of typed actions, about 100 after removing aliases
  and tabs: `Close`, `Maximize(Axes)`, `SnapStep(Direction)`,
  `SendToWorkspace(WorkspaceTarget)`, `ActivateWindowListEntry(usize)`,
  `ActivateTrayItem(usize)`, `SetSetting { key, value }`, `SaveSettings`,
  `SetEnvironment { name, value }`, `CycleProfile`, `CyclePrimaryOutput` and
  so on.
- **`Compositor::execute(command, ExecutionContext)`** is the only dispatcher.
  The context carries the target window, resolved per binding kind: the
  clicked window, the focused one, or each one under `ForEach`.
- Keys, mouse bindings, menu items, panel widgets and IPC all build a
  `Command`; nothing dispatches on strings.
- Each `Action` variant's behavior lives with the subsystem it changes.
  `execute` is a flat match that delegates, for example
  `Action::Maximize(axes) => self.windows.maximize(target, axes, &self.outputs)`,
  so it never becomes fluxland's 1,741-line dispatcher.

---

## 9. Scene composition (per output, per frame)

`scene::compose(&Compositor, OutputId) → Vec<SceneElement>` returns the
elements from top to bottom:

0. **while the session is locked: the lock surface of this output, or a solid
   color, and nothing else** (no window content can leak through)
1. cursor (or the cursor plane)
2. overlays: menus, tooltip, dialogs, snap picker, wireframe, position overlay
3. layer-shell overlay layer
4. **a fullscreen window on this output**, which covers items 5–8 so direct
   scanout is possible
5. layer-shell top layer, then the taskbar
6. windows by `Stack`, top-down; each contributes popups, surface, frame
   strips and borders. A pseudo-transparent window also contributes a
   wallpaper crop beneath its surface that hides the windows below it.
7. layer-shell bottom layer
8. wallpaper, then layer-shell background

`SceneElement` is a `render_elements!` enum: `Surface`, `Panel`,
`SolidColor`, `Wallpaper`, `Cursor`. The same walk (without rendering) answers
`PointerTarget` queries, so what is drawn and what is clicked can never
disagree.

---

## 10. Services (gateways to the outside world)

| Type | What it wraps | Notes |
|---|---|---|
| `IpcServer` / `IpcConnection` | Unix socket (calloop source per connection) | Requests become `Command`s or queries; events are sent to subscribed connections. Connections are dropped on error, so nothing is held on a client's behalf |
| `Tray` | zbus on its own thread; `TrayItem` model; dbusmenu → `Menu` content | Communicates only through channels, so there is no shared mutable state (R-O4). Activation tokens for clicks |
| `Xwayland` | Smithay `X11Wm`, `XWayland` process | `.Xresources`, RandR primary from `Outputs::primary`, `InputEmulation` with allow/ask/deny |
| `Launcher` | `std::process` with pidfd reaping in calloop | Environment overrides (`SetEnvironment`), denylist, autostart, profile hooks. Replaces fluxland's five double-fork copies with one |
| `Session` | session lock, idle notify and inhibit, systemd/D-Bus activation environment | |

---

## 11. How a few use cases flow

**A window opens.**
1. `XdgShellHandler::new_toplevel` creates `Windows::insert(Toplevel)` (not yet
   mapped).
2. At the first commit, `Compositor::map_window(id)` runs:
   1. `Rules::settings_for`
   2. `Placement` from rules, or `PlacementStrategy::place`
   3. `Workspaces` assignment
   4. `Frame::new`
   5. `Focus::on_mapped` (focus new windows, activation policy)
   6. `Stack::push`
   7. affected outputs marked damaged
   8. IPC event `WindowOpened`

**Click the close button.**
1. Pointer motion computes
   `PointerTarget::Window { id, part: Frame(Button(Close)) }`.
2. The titlebar panel shows the button hovered.
3. A press shows it pressed.
4. A release on the same `HitId` runs `execute(Run(Close), target = id)`.
5. `Toplevel::close`.

**Alt+Tab.**
1. The binding `NextWindow { matcher }` makes `Focus::cycle` freeze the MRU
   order into `FocusCycle`.
2. Each press steps through it.
3. Releasing the modifier makes `Input` call `Focus::commit_cycle`.

**Hotplug a monitor.**
1. The backend reports a new connector, and `Outputs::add`.
2. `DisplayProfiles::choose` picks a profile, then `Outputs::apply`.
3. The taskbar and wallpaper for that output are created per config.
4. Windows whose `last_output` matches move back.
5. The profile hook runs.

---

## 12. Where each inventory section lives

| 00 section | Home |
|---|---|
| §2 Windows | `window/` (`Window`, `Toplevel`, `Placement`, `Frame`), `windows.rs` (`Windows`, `Stack`), `rules.rs`, `matcher.rs`, `placement_strategy.rs` |
| §3 Focus | `focus.rs` (`Focus`, `FocusCycle`, `ActivationPolicy`) |
| §4 Workspaces and outputs | `workspaces.rs`, `output/` (`Output`, `Outputs`, `FrameClock`, `VrrPolicy`), `profiles.rs`, `wallpaper.rs` |
| §5 Taskbar | `taskbar/` (`Taskbar`, `Clock`, `WindowList`), `tray/` |
| §7 Menus and dialogs | `overlays/` (`MenuStack`, `Menu`, `Tooltip`, `SnapPicker`); dialogs in the `vaht-dialog` crate |
| §8 Input | `input/` (`Input`, `KeyBindings`, `MouseBindings`, `Grab`, `PointerTarget`) |
| §9 Commands | `command.rs` (`Command`, `Action`, `Condition`) |
| §10 Config and theme | `config/` (`Config`, `ConfigFiles`, `SettingOverrides`), `vaht-ui::theme` |
| §11 Session and IPC | `services/` (`IpcServer`, `Launcher`, `Session`), `xwayland/` |
| §12 Media and games | `output/frame_clock.rs`, `output/vrr.rs`, `protocols/` (fifo, commit-timing, syncobj, content-type: Smithay states plus thin handlers) |

---

## 13. Resolved questions

1. **Workspaces are global**, and callers always ask per output (§5.3).
2. **Borders are solid colors.**
3. **Menus go through a `MenuPresenter`, with the built-in one as the default
   and GTK as an option (decided).** See §6.4. The reasoning for drawing menus
   ourselves by default:
   - The StatusNotifierItem specification makes the *host* responsible for
     showing an item's dbusmenu menu and tooltip. waybar draws them with GTK
     and Plasma with Qt, each in its own style. The alternative,
     `ContextMenu(x, y)`, asks the application to show its own menu, which
     cannot be positioned on Wayland, so few applications implement it.
   - Root and window menus drawn by the compositor are standard for stacking
     window managers (Openbox, labwc, KWin's window menu).
   - Being a good citizen here means:
     - menus appear in screenshots and screen sharing (they are part of the
       output);
     - they scale with the output;
     - they close when focus goes elsewhere;
     - every menu has an action, so users can bind a key to an external
       launcher (fuzzel, rofi) instead.
4. **Dialogs are client windows run from the same binary** (§2).
