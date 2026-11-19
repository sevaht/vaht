# 00 — Feature inventory

Everything fluxland does today (as of commit `e47fabc`), where it lives, and a
**proposed** verdict for vaht. Verdicts are for discussion, not decisions.

- **keep**: same behavior, new home in the object model.
- **rework**: keep the capability, change the shape (config, behavior, or scope).
- **drop**: remove; reason given.
- **later**: keep in mind and allow for in the structure, but not in the first milestone (parity).
- **ask**: needs a decision.

My live setup (`~/.config/fluxland/`: `init`, `keys`, `apps`, `menu`,
`windowmenu`, `overlay`, `startup`, `styles/nacitar_fbstyle`) is the parity
target. A ★ marks features it actually uses.

## Decisions so far

| Topic | Decision |
|---|---|
| First milestone | Parity with the ★ features. Everything else marked keep or later is designed for, but built afterwards |
| Config | TOML in several files. The split is ours to choose, not Fluxbox's |
| License | MIT, matching Smithay. Dependencies may be permissive or MPL-2.0 (file-level copyleft); strong copyleft is decided case by case |
| Slit, tabs and groups, Remember | Dropped (Remember is a proposal) |
| Keyboard | Nothing requires a mouse: every taskbar operation is a bindable action. The taskbar never takes keyboard focus; menus and dialogs do |
| Writing config | Only through SaveSettings, explicitly, preserving formatting. Never implicitly |
| VRR and tearing | VRR first-class; tearing off by default |
| IPC | vaht's own protocol: one JSON message per line over a Unix socket |
| Display profiles | Built in, with hooks. Audio stays outside |

## Glossary (N6)

| Use | Not | Meaning |
|---|---|---|
| **output** | head, monitor, screen | A display vaht draws to |
| **window** | view, client, frame | A managed toplevel, X11 or Wayland |
| **frame** | decoration | What vaht draws around a window: titlebar, buttons, border, handle |
| **minimize** | iconify | Hide a window to the taskbar |
| **taskbar** | toolbar | The bar with workspaces, window list, tray and clock |
| **window list** | iconbar | The taskbar's list of windows |
| **tray** | systray, system tray | The taskbar's StatusNotifierItem host |
| **panel** | (none) | Anything vaht draws and lays out itself: a frame, the taskbar, a menu, a tooltip, a dialog |
| **profile** | layout, profile set | A named output configuration |
| **layout** | preset, profile, theme | A named set of presentation settings (which taskbar widgets, the clock, the window buttons, blur): a file in `layouts/`, chosen by the `layout` setting. It lies under `settings.toml`, which wins, and swapping it never rewrites that file |
| **style** | skin | The colors, fonts, textures and sizes of everything vaht draws: a file in `styles/`, chosen by the `style` setting. Fluxbox calls the same thing a style; `vaht import fluxbox-style` converts one |
| **setting** | option, preference, resource | One key of `settings.toml`, with a documented type, range and default |
| **pending change** | draft, dirty edit | A setting changed while vaht runs and not yet written to its file; it already runs |
| **revert** | undo, discard | Drop the pending changes, so the files run as they say |
| **problem** | error, warning | Something in the configuration files vaht could not use: it left that out, used the default or what was in effect, and reports it (log, banner, `vaht config files`) |

---

## 1. What the survey found (feeds `01-object-model.md`)

These are structural problems in fluxland that vaht's design should avoid.

1. **Every compositor-drawn surface has its own rendering, layout and input
   code.** `render.c` does provide shared drawing primitives (textures, text,
   glyphs, rounded rectangles). Above them, though, the decoration, toolbar,
   menu, slit, systray, tooltip, dialog, snap picker and snap/position overlays
   each carry their own copy of all of these:
   - building scene buffers
   - layout
   - hit-testing (`wm_decoration_button_at`, `wm_menu_handle_motion`,
     `wm_systray_handle_click`, `wm_toolbar_tooltip_at`, …)
   - hover and pressed state
   - tooltip lookup
   - rebuild and damage logic

   This is the duplication I remembered from Fluxbox. In vaht, one *panel/widget* model
   should cover all of them: a window frame is a panel, and so are the taskbar
   and a menu.
2. **The style is a flat struct, one field per element per state.** For example
   `window_label_focus_text_color` and `window_button_pressed_pic_color` are
   separate fields, and there are dozens more like them. Fluxbox wildcards
   (`*.color`) are resolved once at load time. In vaht this should be a small
   model of *element × state → appearance*, with the cascade done by the style
   loader.
3. **`wm_server` holds everything.** It has about 98 protocol globals plus grab
   state, snap state, focus-cycle state, key-repeat state, window marks and
   more. Each group belongs in its own type.
4. **Window state is a pile of flags.** `wm_view` uses `maximized`,
   `maximized_vert`, `maximized_horiz`, `lhalf`, `rhalf`, `tile`,
   `snap_return`, `fullscreen` and `pending_*`. Nothing stops contradictory
   combinations. vaht should replace them with one placement-state enum (P4,
   make invalid states unrepresentable).
5. **Object lifetimes are managed by hand.** There are 362 `wl_listener`
   fields and 228 manual `wl_list_remove` calls. Bugs fixed in fluxland's own
   history (a use-after-free in the menu, type confusion in `view_at`) come
   from this. Rust ownership plus Smithay removes most of this class of bug;
   see `03-resources-logging.md`.
6. **Commands are strings.** The command language parses well, but actions are
   dispatched by name through one 1,741-line `keyboard_actions.c`. In vaht
   they should become a typed `Action` enum, parsed at the config boundary
   (M6) and matched exhaustively (E7).
7. **Some protocols are advertised but do nothing.** `tearing-control` and
   `content-type` hints are accepted and never acted on, so fluxland never
   performs a tearing page flip for games. See `04-smithay-mapping.md`.

---

## 2. Windows

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| Managed windows, one abstraction over xdg-shell and X11 | `view.c`, `view_xdg.c`, `xwayland.c` (`wm_view_impl` vtable) | keep | Becomes a `Window` that owns a `ClientSurface` enum (`Xdg` / `X11`); see M11 and R-M15 rung 2 |
| Unmanaged X11 windows (override-redirect) stacked as X does | `xwayland.c` | keep | |
| X11 Shape clipping | `xwayland_shape.c` | keep, exact | fluxland made a clipped scene node per rectangle and gave up past 32 (`WM_XSHAPE_PIECES_MAX`), drawing the whole window. vaht clips exactly, with no limit: see 04 §8 |
| Size hints (min, max, increments, aspect) | `view_geometry.c` | keep ★ | `maxIgnoreIncrement` |
| Server-side frame: titlebar, label, buttons, handle, grips, border, round corners | `decoration.c` | keep ★ | First user of the unified panel model |
| Negotiate client-side decoration fallback (xdg-decoration, KDE protocol, Motif hints) | `decoration.c`, `xwayland.c` | keep | |
| Tabs and groups, in the titlebar or external | `tabgroup.c`, `decoration.c` | **drop** (decided) | A frame holds exactly one window. My Tab, NextTab, PrevTab and StartTabbing bindings are not converted, and the import tool reports them as dropped. This removes the tab focus model, `{groups}` in patterns, and tab drag-and-drop |
| Shade | | keep ★ | |
| Sticky (on all workspaces) | | keep | |
| Stacking layers (Desktop, Below, Normal, Above, plus Dock and others) | `view.h` `wm_view_layer` | keep | Rename to plain layers; the import tool maps Fluxbox's numbers |
| Maximize, maximize horizontally or vertically, fullscreen | | keep ★ | One `Placement` enum (see §1.4) |
| LHalf and RHalf, TileTo, SnapStep, snap-zone drag with layout picker | `cursor_snap.c`, `snap_picker.c` | keep ★ | My own additions (SnapStep, picker). Fold them all into one tiling or snap concept |
| Edge snapping while moving or resizing | `cursor_snap.c` | keep ★ | `edgeSnapThreshold: 10` |
| Opaque and wireframe move and resize, position overlay | `cursor_snap.c` | keep ★ | I use opaque move and wireframe resize |
| Arrange and cascade commands | `keyboard_actions.c` | keep | Cheap once placement is a type |
| Placement strategies (RowMinOverlap and others) | `placement.c` | keep ★ | Strategy enum |
| Per-focus alpha | | keep | |
| Pseudo-transparency (a translucent window shows only the wallpaper) | `pseudo_transparency.c` | keep ★ | My kitty rule. Implemented as a render pass, not a copy of the desktop |
| Fade animations, workspace slide or fade | `animation.c`, `workspace.c` | rework | Drive from the frame clock instead of a 60 Hz timer; default off |
| Minimize (iconify), urgency, attention flashing | | keep | |
| Window marks, SetTitle, SetXProp | | keep | SetXProp only applies to X11 windows |
| Remember (writes rules back to the apps file) | `rules.c` | **drop** (proposed) | Explicit rules only. A later convenience: a "rule for this window" action that copies a ready-made TOML rule (match plus current geometry) to the clipboard for you to paste |
| Windows return to a monitor that comes back | `output_memory.c` | keep | My feature |
| Compositor-owned windows (dialogs as real windows) | `view_internal.c`, `dialog.c` | rework | Becomes panels hosted in a frame; there is no fake view backend |

## 3. Focus and stacking

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| Click, sloppy and strict-mouse focus models | `view_focus.c` | keep ★ | ClickFocus |
| Auto-raise with delay, click raises | | keep ★ | |
| Focus new windows | | keep ★ | |
| Most-recently-used cycling, order frozen while modifiers are held | `view_focus.c` (`focus_seq`) | keep ★ | Alt-Tab over the windows of the current workspace |
| Directional focus (FocusLeft and the others) | | keep | |
| Focus-stealing prevention with xdg-activation tokens, FocusProtection, tray grants | `protocols.c`, `tray_grant.c` | keep ★ | My steam rule. Design `FocusPolicy` as one type |
| noFocusWhileTypingDelay | | keep | |
| Keyboard-only navigation between interface zones | `focus_nav.c` | **rework** (decided) | Nothing requires a mouse, but the taskbar **never takes keyboard focus**. Clicking it focuses only the window you picked. Every taskbar operation is a bindable action instead: activate the Nth window in the window list, activate a tray item, open its menu. Only menus and dialogs take keys, because you opened them and they close when done |
| AT-SPI bridge | `atspi_bridge.c` | later | Optional |

## 4. Workspaces and outputs

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| Named workspaces, add and remove | `workspace.c` | keep ★ | 4 workspaces |
| Workspace warping at screen edges | | keep | Off in my config |
| Per-workspace wallpaper (SetWallpaper) | `wallpaper.c` | rework (decided: native) | Per output, optionally per workspace; fill, fit, center and tile modes. Decode once, scale once per output size, draw as the lowest element. Native wallpaper also makes pseudo-transparency simpler, because the compositor owns the image it shows through. swaybg remains the fallback if this turns into an ordeal |
| ext-workspace protocol | `ext_workspace.c` | keep | Smithay does not implement it, so vaht implements it |
| ShowDesktop | | keep | |
| Per-output mode, position, enabled and adaptive sync | `output_config.c` | keep ★ | HDMI-A-1, DP-2, eDP-1 off |
| Hot-plug and resume | `output.c` | keep | My latest commit fixes disabled outputs coming back on resume |
| Output management protocol (wlr-randr, kanshi, av-output-switcher) | `output_management.c` | keep ★ | Still exposed for outside tools. Runtime changes are not written back to config |
| **New: display profiles** (av-output-switcher's video half, built in) | none (external tool) | keep (decided) | Named layouts; automatic choice by which monitors are connected; `CycleProfile` and `ApplyProfile` actions. Monitors matched by make, model and serial, falling back to connector |
| **New: primary output** | none (external tool's own notion) | keep (decided) | Real inside vaht: where the taskbar goes, where new windows open, and RandR primary for Xwayland games. Cycled with `CyclePrimaryOutput` |
| **New: profile hooks** | none | keep (decided) | On profile apply or primary change, run a user command with a JSON description (profile name, outputs, primary) on stdin, plus `VAHT_PROFILE` in the environment. Audio switching stays outside vaht, in a script the hook calls |
| **New: profile editor GUI** | none | later | A separate program speaking the IPC (list monitors, preview, "save the current layout as a profile"). The IPC needs those operations from the start |
| Output power, gamma control | | keep | |
| Refer to heads by position | | keep | |

## 5. Taskbar (Fluxbox's toolbar)

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| Tools: prev/next workspace, workspace name, iconbar, system tray, clock | `toolbar.c` | keep ★ | Exactly my `tools:` line |
| Placement, head, width percent, height, layer, alpha, auto-hide, maxOver | | keep ★ | |
| Iconbar mode as a client pattern, alignment, icon width, padding, app icons | `toolbar.c`, `icon.c` | keep ★ | `{static groups} (workspace)` |
| Clock with strftime format | | keep ★ | `%k:%M` |
| System tray: StatusNotifierWatcher, dbusmenu menus, tooltips, activation tokens, pinLeft and pinRight | `systray.c`, `systray_menu.c`, `tray_grant.c` | keep ★ | Pure-Rust D-Bus with `zbus` |
| Tooltips (truncated titles, tray items) | `tooltip.c` | keep ★ | One `Tooltip` panel for everything |
| **New:** several rows (tray and iconbar each wrap onto N rows; a taller clock gets its own layout) | none | **built**: `rows` (a number, or `"auto"` up to `max_rows`, by `row_limit`) | Allow for it from the start: widgets lay out in a *grid area*, not a fixed strip |
| **New:** a taskbar on every output (optional) | none | later | The taskbar is per output from day one; config says which outputs |
| **New:** iconbar shows all windows, or only those on its output | none | later | An iconbar filter is a client pattern plus an output scope |

## 6. Slit

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| Container for dockapps: placement, autohide, layer, slitlist ordering | `slit.c` | **drop** (decided) | Never used. Not implemented, but the design allows for it: the taskbar's edge-docked panel hosting (placement, autohide, layer, strut) is general enough that a slit could be another panel of that kind later |

## 7. Menus and dialogs

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| Root, window, workspace, client (window list) and custom menus | `menu*.c` | keep ★ | |
| Submenus, separators, pipe menus, icons, key navigation, type-to-search, right-to-left text | `menu.c` | keep ★ | `menuSearch: itemstart`, `menuDelay: 200` |
| Built-in entries: [stylesdir], [workspaces], [config], [reconfig], [restart], [exit] | `menu_parse.c` | rework | [config] edits fluxbox `init` live. Under the new format, show toggles for real settings only |
| Window menu (my `windowmenu`) | | keep ★ | |
| CommandDialog, SetTitleDialog, SetWorkspaceNameDialog | `dialog.c` | rework (decided) | Ordinary client windows (egui, the `vaht-dialog` program): `command`, `title`, acting through IPC |
| RunDialog (separate `fluxland-run` program) | `tools/fluxland-run.c`, `run_dialog.c` | keep ★ (decided) | `vaht-dialog run`: an egui window that appears in the taskbar |

## 8. Input

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| Key bindings: chains, keymodes, Repeat with rate | `keybind.c`, `keyboard.c` | keep ★ | My Repeat on volume keys |
| Mouse bindings by context (OnDesktop, OnToolbar, OnTitlebar, OnWindow, OnLeftGrip…), double click, MoveN drags | `mousebind.c`, `cursor.c` | keep ★ | Contexts become the *region* a panel widget reports |
| Interactive move and resize (NearestCorner) grabs | `cursor.c` | keep ★ | One `Grab` enum, owned by the seat |
| Per-device libinput settings, keyboard layouts, key repeat rate | `input_config.c` | keep | |
| Touchpad gestures, touch, tablet | `input.c`, `tablet.c` | keep | Smithay has them |
| Input methods and text-input, virtual keyboard and pointer | `text_input.c` | keep | |
| Emulated input from X11 programs via libei (Steam Input) | `eis.c` | keep ★ | `inputEmulation.allow: steam`. Smithay has a libei backend |
| Keyboard-shortcuts inhibit, pointer constraints, relative pointer | | keep | Games |

## 9. Command language

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| 132 Fluxbox commands | `keybind.c`, `keyboard_actions.c` | rework | One typed `Action` enum. Prune the alias duplicates (Iconify, Minimize, MinimizeWindow…) |
| MacroCmd, ToggleCmd, If, Cond, ForEach, Map, Delay | `command.c` | rework | Keep the capabilities in a cleaner syntax, as part of `02-config-format.md` |
| Client patterns `(name=…) (title!=…) {N}` | `client_pattern.c` | rework | Becomes a typed `WindowMatcher` |
| SetResourceValue (and its dialog) | `keyboard_actions.c` | keep, renamed **SetSetting** (decided) | Changes a setting while running, by vaht's own setting key (`taskbar.auto_hide true`). Lasts until the settings file says otherwise at that key, or SaveSettings writes it. Also available from `vaht config set`, and by hand in the settings window (`vaht-settings`, the `settings` command), which is a client of vaht's IPC: it holds no configuration logic, shows each setting by the schema vaht describes, runs every change at once and offers Save and Revert |
| SaveRC | | keep, renamed **SaveSettings** (decided) | Writes the settings changed at runtime into the settings file. Uses `toml_edit`, so the file's comments, ordering and formatting survive and only the changed keys are touched. This is the only time vaht writes to config, and only when you ask |
| Export, SetEnv | | keep, merged into **SetEnvironment** (decided) | Sets a variable in vaht's environment, inherited by programs it starts afterwards |

## 10. Configuration and style

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| `init` (90 Fluxbox resources) | `config.c`, `rcparser.c` | rework | New format (see 02). My `init` gets converted |
| `keys`, `apps`, `menu`, `windowmenu` files | | rework | New format, possibly one file or several. Converted |
| Fluxbox style format: textures, gradients, bevels, interlacing, pixmaps, fonts with halo and shadow, button pixmaps, style overlay | `style.c`, `render.c` | rework | New style format built on cascading *element × state*. My style plus overlay gets converted. A Fluxbox style importer lives in the import tool |
| Live reload (Mod4+r, SIGHUP) | | keep | |
| `--check-config` validation | `validate.c` | keep | `vaht config check` |
| libinput settings (`session.input.*`) and switch commands (`session.switch.*`: lid, tablet mode) | `input_config.c`, `input.c` | keep | `[input]` in `settings.toml` and `[switches]` in `bindings.toml`, both converted by `vaht import` |
| `--import-fluxbox` | `import.c`, `fbcompat.c` | rework | `vaht import`, which converts both fluxbox and fluxland configs |
| Settings with no effect on Wayland (colorsPerChannel, cacheLife…) | | drop | |
| DPI setting | | keep ★ | `dpi: 96` |
| `.Xresources` loaded into Xwayland | | keep ★ | |

## 11. Session and integration

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| Startup script and autostart (double fork, environment denylist) | `autostart.c` | keep ★ | My `startup` script |
| How vaht is started | run from a TTY | keep (decided) | Running `vaht` from a TTY is the primary, fully supported path. It also ships a `wayland-sessions/vaht.desktop` for display-manager users, and optionally binds to systemd's `graphical-session.target` (off by default) |
| D-Bus and systemd activation environment for portals | `session_env.c` | keep ★ | My script also does part of this, so pick one owner |
| Session lock, idle notify, idle inhibit | | keep ★ | |
| IPC: JSON over a Unix socket, `-ctl` tool, subscriptions | `ipc*.c`, `tools/fluxland-ctl.c` | rework (decided: own protocol) | One JSON message per line over a Unix socket at `$VAHT_SOCKET`. Usable from any language's standard library, or from `socat`. Requests, replies and events are Rust types in a shared protocol crate, published as JSON Schema so other languages can generate clients. `vaht`'s own commands that ask the running vaht use the same crate. Not i3/sway compatible; bars use ext-workspace and foreign-toplevel instead |
| Screen capture (screencopy, ext-image-copy-capture, export-dmabuf) | `screencopy.c` | rework (decided) | ext-image-copy-capture only: my xdg-desktop-portal-wlr 0.8.4 and grim 1.5 both speak it. wlr-screencopy and export-dmabuf are dropped |
| WLCS conformance shim | `wlcs_shim.c` | later | Good for testing; Smithay compositors can use WLCS too |
| Compile-time performance probes | `perf.c` | rework | Use `tracing` spans with optional Tracy (see 03) |

## 12. Media and games (requested focus)

| Feature | fluxland | Verdict | Notes |
|---|---|---|---|
| presentation-time | `presentation.c` | keep | |
| fifo-v1, commit-timing-v1 | `frame_pacing.c` (736 lines, hand-written) | keep | Smithay has both |
| Explicit sync (linux-drm-syncobj) | `drm_syncobj.c` | keep | Important for NVIDIA and Wine/Proton |
| tearing-control | hint accepted, **never applied** | rework (decided: no tearing by default) | VRR is the answer to latency. Async page flips only if a setting allows them *and* a fullscreen game asks; the setting is off by default |
| content-type | accepted, **unused** | rework | Use it for a policy: games may tear and use VRR, video gets VRR at its own rate |
| Adaptive sync (VRR) | static per output only | rework (decided: first-class) | Per-output modes: off, always, or only while a fullscreen client is shown (the default). Frame scheduling must not fight VRR: render when a client commits, not on a fixed tick |
| Direct scanout of fullscreen windows | implicit (wlr_scene) | keep | Explicit in Smithay's `DrmCompositor`; design so a fullscreen window is not covered |
| DRM lease (VR headsets) | `drm_lease.c` | keep | |
| Idle inhibit (video players) | | keep | |
| Pointer constraints, relative pointer, pointer warp | | keep | |
| Xwayland keyboard grab | `xwayland_grab.c` | keep | Smithay has it |
