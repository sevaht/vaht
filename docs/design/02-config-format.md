# 02 — Configuration format

How vaht is configured, shown with my current fluxland setup converted. This
is a draft for discussion. Names follow the glossary in 00. Types are those of
01.

---

## 1. Principles

1. **TOML, parsed once into typed values** (M6). Every file deserializes with
   serde into `Config`, and unknown keys are errors (`deny_unknown_fields`).
   An error names the file, line and column (from `toml`'s spans) and says
   what was expected (E3).
2. **One concept per file**, split by what changes together, not by Fluxbox's
   historical split.
3. **Defaults live in code, documented in one place.** Every file and every key
   is optional; an empty config directory gives a working desktop.
   `vaht config check --print-defaults` prints the complete effective config, so the
   defaults can be read without opening the source (K3: they are written down
   only once).
4. **No paths to other config files.** File names are fixed (K3). Fluxbox's
   `session.keyFile`, `session.menuFile` and the rest are gone.
5. **A bad config is used as far as it can be, and loudly.** What cannot be
   used is left out, never the rest: see *Partly valid* under §2. `vaht config check`
   validates without running and lists every problem.
6. **vaht writes config only on SaveSettings**, through `toml_edit`, which
   keeps comments and layout (00 decision).
7. **Spelling.** Keys are `snake_case`, as is usual in TOML. Values that name
   things (actions, edges, modes) are `kebab-case` strings.

## 2. Files

Everything lives under `$XDG_CONFIG_HOME/vaht/` (normally `~/.config/vaht/`):

| File | Contains | Replaces (fluxland) |
|---|---|---|
| `settings.toml` | Behavior: focus, placement, workspaces, taskbar, menus, windows, Xwayland, session, style choice, wallpaper | `init` |
| `bindings.toml` | Key and mouse bindings, key modes | `keys` |
| `rules.toml` | Per-window rules | `apps` |
| `menus.toml` | Root menu, window menu, custom menus | `menu`, `windowmenu` |
| `outputs.toml` | Per-output settings, display profiles, profile hooks | `session.output.*` in `init`; av-output-switcher's `profiles.json` (video part), converted by hand |
| `styles/<name>.toml` | Styles; also read from `/usr/share/vaht/styles/` | `styles/*`, `overlay` |

- **`overlay` disappears.** My overlay existed only to stop styles from
  setting the background. In vaht styles cannot set the wallpaper at all; it
  belongs to `settings.toml`.
- **`startup` becomes `session.autostart`.** It is a list of commands, and my
  existing script can simply be one entry.
- **Reloading** happens automatically when a file changes (decided), and also
  on the `reload` action and on SIGHUP. Rules apply to windows opened after the
  change.
  - **No polling.** One inotify watch on the config directory and one on
    `styles/` are registered as a calloop source. vaht watches directories,
    not files, because editors save by writing a temporary file and renaming
    it, which a watch on the file would lose.
  - **Debounced.** An editor's burst of events (write, rename, chmod) settles
    into one reload after 200 ms of quiet.
  - **Partly valid.** At the start and on every reload vaht uses all of the
    configuration that is valid and says what it could not use. Each file is
    taken on its own: one that is not TOML or not valid stays what it was (the
    built-in one, at the start). `settings.toml` is taken a setting at a time:
    a setting that is not valid, a misspelled key or a value out of range, is
    left out, so it is the default, and every other setting, and every other
    file, is in effect. A style that cannot be read is the built-in style;
    `bindings.toml` and `menus.toml` that name each other's menus or key modes
    wrongly are both put back. Each thing left out is a problem. It is logged as
    an error and drawn as a red banner along the top of every output, naming the
    file, the setting, what is wrong and what vaht used instead, until the files
    are fixed; a click hides it (until the problems change). The banner is drawn
    from the built-in style, whatever style is in use, so a broken style cannot
    hide it. `vaht config files` and the settings window list the problems
    too. Fixing the files triggers the next load by itself. A change made from
    the settings window or `vaht config set` is still refused if it is
    itself what is not valid; problems the files already had do not stop it.

---

## 3. `settings.toml`

### 3.1 My `init`, converted

Only keys that differ from the default would be written. They are all shown
here so the mapping can be checked.

```toml
style = "nacitar"
dpi = 96

[focus]
model = "click"                 # click | sloppy | strict-sloppy
raise_on_click = true
auto_raise = true               # applies to sloppy focus only, as in Fluxbox
auto_raise_delay_ms = 250
focus_new_windows = true
stay_on_output = false          # focusSameHead
typing_grace_ms = 0             # noFocusWhileTypingDelay

[placement]
strategy = "row-min-overlap"    # row | column | row-min-overlap | cascade | under-pointer | center
row_direction = "left-to-right"
column_direction = "top-to-bottom"

[windows]
move = "opaque"                 # opaque | wireframe
resize = "wireframe"
resize_redraw_delay_ms = 50     # opaqueResizeDelay
show_position = false
edge_snap_distance = 10         # edgeSnapThreshold
edge_resize_snap_distance = 0
snap_zones = true               # enableWindowSnapping
snap_layout_picker = true
double_click_ms = 250
tooltip_delay_ms = 500

[windows.frame]
titlebar_left = []
titlebar_right = ["shade", "minimize", "maximize", "close"]
handle = false                  # defaultDeco 0x3fd = everything except the handle
icon = true                     # tabs.intitlebar and tabs.usePixmap

[windows.maximize]
ignore_size_increments = true   # maxIgnoreIncrement
cover_taskbar = false           # fullMaximization
allow_move = true               # maxDisableMove = false
allow_resize = true

[workspaces]
names = ["Workspace 1", "Workspace 2", "Workspace 3", "Workspace 4"]
wrap_at_edges = false           # workspacewarping (its offsets/axes keep their defaults)

[taskbar]
outputs = "primary"             # primary | all | [output matches]   (onhead 1)
edge = "bottom"                 # placement BottomCenter
alignment = "center"
width_percent = 100
height = "style"                # or pixels (height 0 = from the style)
rows = 1                        # new: a number, or "auto" up to max_rows (00 §5)
auto_hide = false
reserve_space = true            # maxOver = false
layer = "dock"
widgets = ["workspace-previous", "workspace-name", "workspace-next", "window-list", "tray", "clock"]

[taskbar.window_list]
show = { workspace = "current" }    # a window matcher (§5); iconbar.mode (workspace)
scope = "all-outputs"               # new: all-outputs | this-output
order = "stable"                    # {static}: opening order, not focus order
entry_width = "fill"                # alignment Relative; Left: { max = iconWidth }
icons = true                        # usePixmap
text_padding = 10                   # iconTextPadding

[taskbar.clock]
format = "%k:%M"
format_tall = "%k:%M\n%a %d %b"     # new: used when the taskbar has room for two lines

[taskbar.tray]
pin_left = []
pin_right = []
activation_grace_ms = 500

[menus]
presenter = "built-in"          # built-in | gtk (01 §6.4)
submenu_delay_ms = 200
search = "item-start"           # off | item-start | anywhere

[wallpaper]                     # new: native (00 §4)
image = "~/Pictures/wallpaper/gucken/Starry_Night_by_gucken_1920x1080.png"
fit = "fill"                    # fill | fit | center | tile | stretch
# [wallpaper.workspaces]        # optional: a workspace's own image, by number
# 2 = "~/Pictures/beach.png"

[xwayland]
resources = "~/.Xresources"
input_emulation = { default = "ask", allow = ["steam"], deny = [] }

[ipc]
allow_commands = true           # allowRemoteActions

[logging]
file_size_mb = 10               # new: vaht.log starts anew past this
files = 5                       # new: files kept, vaht.log included
# delete_after_days = 14        # new: older files unwritten that long go

[session]
autostart = ["~/.config/vaht/autostart"]
```

### 3.2 What is built

`settings.toml` accepts the sections whose features exist, and refuses the
rest by name, so a setting never silently does nothing:

- `[focus]`: `model` (`click`, `sloppy`, or `strict-sloppy`: as sloppy,
  but a window takes focus only when the pointer moves into it from
  elsewhere, never one raised or moved under it, however the pointer
  moves within it after; Fluxbox's `StrictMouseFocus`, which `vaht import`
  converts), `raise_on_click`, `auto_raise`, `auto_raise_delay_ms`,
  `focus_new_windows`.
- `[placement]`: every strategy and both directions; `column-min-overlap`
  was added for Fluxbox's `ColMinOverlapPlacement`. The smart strategies
  try only corners flush against edges, so `row` and `row-min-overlap`
  place alike.
- `[windows.snapping]` (fluxland's snap zones, from `enableWindowSnapping`
  and its kin): `enabled`, `zone_distance`; a window let go within it of
  the left, right or bottom edge takes that half, of a corner that quarter,
  of the top edge the whole usable area, restoring to where the move began;
  a blue preview just under the window shows where. `layout_picker`,
  `picker_size`, `picker_columns` and `layouts` (cells in thousandths, as
  `tile` takes them; fluxland's eight by default): dragging to the top edge
  shows a panel of layouts, and letting go on a cell puts the window there.
- `[windows]`: `edge_snap_distance`, `double_click_ms`, `focused_alpha`
  and `unfocused_alpha` (0 to 255; `set-alpha` changes one window's),
  `opaque_resize`
  (false shows an outline of the new frame, in the style's `wireframe`,
  while resizing, and the
  window takes the size when the drag ends; from Fluxbox's
  `opaqueResize`), and `[windows.frame]`
  with `titlebar_left` and `titlebar_right` (buttons `shade`, `stick`,
  `minimize`, `maximize`, `close`; the `maximize` button is a box, and
  while its window is maximized, two boxes: a restore button, as that is what
  clicking it does) and `handle` (the strip along the
  bottom with a grip at each end, 20 pixels wide as Fluxbox's; dragging
  it resizes the bottom edge, and a grip its corner, unless the `handle`
  and `grip-*` mouse contexts bind something). `vaht import` reads it from
  `defaultDeco`'s preset or mask. `icon` shows the window's icon left of
  its title (the one the taskbar shows); Fluxbox showed it where tabs were
  in the titlebar and showed icons (`tabs.intitlebar`, `tabs.usePixmap`).
- `[workspaces]`: `names`, at least one.
- `[session]`: `autostart`, and `update_activation_environment` (from
  fluxland's `session.updateActivationEnvironment`): `auto` (when vaht
  runs the session from a terminal, not nested or headless), `always` or
  `never` (a change is followed while vaht runs: turned on, vaht tells the
  environment of the session at once; turned off, it forgets it). vaht then
  runs `dbus-update-activation-environment --systemd`
  with `WAYLAND_DISPLAY`, `DISPLAY` (once Xwayland is ready),
  `XDG_CURRENT_DESKTOP`, `XDG_SESSION_DESKTOP`, `XDG_SESSION_TYPE` and
  `VAHT_SOCKET`, so portals
  and other D-Bus-activated services find the session (the autostart
  programs run once it is done), and empties them when it exits. With the
  first export it also starts `vaht-session.target` afresh (stopped first,
  in case a crashed vaht left it running), and stops it before emptying:
  that target binds `graphical-session.target`, so services `PartOf` it,
  the desktop portals among them, live as long as the session instead of
  outliving the vaht they were started for (a dead screen-cast portal
  left OBS and browsers no capture sources). `install.sh` installs it;
  without it, vaht says so on stderr and carries on. Programs vaht starts are also told
  `QT_QPA_PLATFORM=wayland;xcb`, `SDL_VIDEODRIVER=wayland,x11` and
  `_JAVA_AWT_WM_NONREPARENTING=1`, unless vaht's own environment sets
  them.
- `[wallpaper]`: `image` (PNG, JPEG, GIF, WebP, BMP, TIFF, TGA, PNM, QOI
  or ICO, known by its contents; `~/` is the home directory, a relative
  path is from the configuration directory) and `fit` (`fill`, `fit`,
  `center`, `tile`, `stretch`). It is decoded once, painted once for each
  output, at its size, and drawn beneath everything; where it does not reach, and with no
  image, the desktop is dark grey. `[wallpaper.workspaces]` gives
  particular workspaces, by number, an image of their own, shown in place
  of `image` while the workspace is (fluxland's
  `session.screen0.workspaceN.wallpaper`, which `vaht import` converts,
  N + 1 being the number); each image is painted for an output the first
  time it shows there and kept, so switching workspaces paints nothing.
- `[xwayland]` `enabled` (on by default; follows a change while vaht runs):
  vaht starts Xwayland and is its window manager, and programs it starts
  find it in `DISPLAY`. Turning it off closes the X11 windows' programs'
  connections (their windows go), stops Xwayland and takes `DISPLAY` out
  of the environment and the activation environment; turning it on starts
  it again. X11 windows are windows like any other (rules match their
  class as `app_id`, `close` sends `WM_DELETE_WINDOW`, `kill` ends the
  process their `_NET_WM_PID` names); their menus and tooltips
  (override-redirect windows) show over every window. The clipboard and
  primary selection pass both ways. `vaht --no-xwayland` overrides the
  setting (tests use it). `resources` names an X resources file
  (`~/.Xresources`; a relative path is from the configuration directory)
  loaded into Xwayland when it starts, as xrdb does, from fluxland's
  `session.xwayland.resources`. `[xwayland.input_emulation]` (fluxland's
  `session.xwayland.inputEmulation` and its `.allow` and `.deny`, which
  `vaht import` converts): X programs' XTest input (Steam Input's desktop
  controls, xdotool) reaches the session through an EIS server Xwayland
  is pointed at (`LIBEI_SOCKET`); only vaht's own Xwayland may connect.
  Each program, by the name Xwayland gives it (its command, such as
  `steam`), is let in if `allow` names it, refused if `deny` does, and
  otherwise as `default` says: `ask` (a menu asks; closing it refuses;
  "Allow" and "Deny" last until vaht exits, "Allow from now on" and "Deny
  from now on" add the program to `allow` or `deny` as `set-setting`
  does, which `save-settings` writes), `allow` or `deny`. Emulated input is
  dropped while the session is locked, and what a program holds is let go
  when it stops, leaves or the session locks. X Shape clipping is exact
  (04 §8.1), the pointer an X11 window sees stays in step while it moves
  (04 §8.2), and a window whose Motif hints ask for no decorations gets no
  frame.
- `[blur]` (new): `enabled` (on by default), `passes` (1 to 6, 3) and
  `spread` (1 to 10 pixels, 3). A program asking for what is behind part
  of a surface to be blurred (`ext-background-effect-v1`: foot's
  `blur=yes`, panels) has it drawn as frosted glass, with a dual Kawase
  blur: the background halved `passes` times and grown back, each step
  averaging samples `spread` pixels apart (more passes blur wider, a
  larger spread softer). Off, vaht tells programs it cannot blur, so they
  draw something else; one that bound before it was turned off learns it
  only when it binds again. Blurred again only when what is behind
  changes. On the GPU only: the headless (software) backend blurs nothing.
- `[logging]`: `file_size_mb` (1 to 1024) and `files` (1 to 100), the
  log file's limits (03 §4.3): `vaht.log` starts anew at each start and
  once it passes the size, the files before it kept as `vaht.log.1` (the
  newest), `.2`, …, so the logs take at most the size times the count.
  Read before anything is logged, so the first file obeys them, and
  followed as they change (a reload, `set-setting`); a lowered count
  clears the extra files at the next rotation. `delete_after_days` (1 to
  3650; unset, files are kept until rotation drops them) deletes older
  files not written for that many days, at start, at each rotation and
  hourly; `vaht.log` itself is never deleted. There is no rotation by
  time: a session's log stays one file, which is what is wanted after a
  crash.
- `[input]` (fluxland's `session.input.*`, which `vaht import` converts):
  libinput settings for pointing devices, in four scopes, the more specific
  winning: `[input.all]`, `[input.touchpad]` or `[input.pointer]` (mice,
  trackballs), and `[input.device."<name>"]` (by libinput's name for the
  device, or fluxland's form of it with each character but letters and
  digits made `_`). Settings: `tap`, `tap_drag`, `tap_drag_lock`,
  `tap_button_map` (`lrm`, `lmr`), `natural_scroll`, `left_handed`,
  `disable_while_typing`, `middle_emulation`, `accel_profile` (`flat`,
  `adaptive`), `accel_speed` (-1 to 1), `scroll_method` (`none`,
  `two-finger`, `edge`, `button`), `scroll_button`, `click_method`
  (`button-areas`, `click-finger`) and `send_events` (`enabled`,
  `disabled`, `disabled-on-external-mouse`). A setting set nowhere keeps
  libinput's default, and one taken out is undone on reload. They apply
  on the DRM backend; a nested vaht's input is the host's.
- `style`: the name of a style in `styles/` (§10); without it, the built-in
  style.
- `[taskbar]`: `enabled`, `output` (the name of the output it shows on,
  such as `HDMI-A-1`; the primary output without one, or while that one
  is not there), `on_every_output` (off by default: on, every output has a
  taskbar, the one on `output` being the main one, the only one with the
  tray, as a desktop has one; each keeps its own output's room and
  auto-hides at its own edge), `windows` (`all`, the default, or
  `own-output`: each taskbar lists only the windows whose middle is on its
  output), `edge` (`top` or `bottom`), `width_percent`
  (1 to 100), `rows` (new: a number of rows, each as tall as the style's
  taskbar, 1 by default; or `"auto"`: one, and another whenever the window
  list reaches its `row_limit`, up to `max_rows` (3). `row_limit` is one of
  three ways to say when: entries narrower than a share of the output's
  width (`{ percent = 15 }`, the default, so it suits a laptop's screen and
  a television alike), entries narrower than a number of pixels
  (`row_limit = 100`), or more windows in a row than a number
  (`{ windows = 8 }`); the taskbar uses the fewest rows that stay within it.
  The window list's entries fill the rows in order, each as wide as the rest; the tray wraps
  across the rows but never adds one; the arrows stay a row wide; the clock
  and workspace name span the rows. Windows keep off every row, and
  maximized ones resize as an `"auto"` taskbar grows or shrinks) and
  `align` (`left`, `center`, `right`; from `placement`'s
  second half), `auto_hide` and `auto_hide_delay_ms` (hidden until the
  pointer touches the edge, hidden again that long after it leaves; an
  auto-hiding taskbar reserves nothing, and its clock rests while hidden),
  `reserve_space`, `[taskbar.tray] activation_grace_ms` (from fluxland's
  `systray.activationGrace`: for that long after a click on a tray item,
  its program may bring its windows forward without a token, as its menu
  often cannot pass one; known by the process the bus reports),
  `[taskbar.tooltips]` `enabled` and `delay_ms` (from
  `tooltipDelay`;
  resting the pointer on an entry whose title does not fit shows the whole
  title, and on a tray item its tooltip, in the style's `tooltip`),
  `widgets` (`workspace-previous`, `workspace-name`, `workspace-next`,
  `window-previous`, `window-next`, `window-list`, `tray`, `clock`; the
  window arrows, Fluxbox's `prevwindow` and `nextwindow`, focus the window
  before or after in the window list's order, among the windows it shows),
  `[taskbar.window_list]` with `show` (a matcher),
  `entry_width` (`"fill"`, sharing the whole list, or `{ max }`, from
  `iconbar.alignment` and `iconWidth`) and `icons` (each entry's icon: the one the
  window names or draws over xdg-toplevel-icon (a drawn one is read back
  through the renderer, with no `unsafe`), else its desktop entry's `Icon`,
  found in the `hicolor` theme or `pixmaps`; PNG and SVG; from
  `iconbar.usePixmap`), `[taskbar.tray]` with `pin_left` and `pin_right`
  (item ids, from `systray.pinLeft` and `systray.pinRight`), and
  `[taskbar.clock]` with `format`, checked when read. The tray hosts
  StatusNotifierItems: it serves the watcher on the session bus, or hosts
  for another program's; it learns of items and their changes only from
  the bus's signals; a left, middle or right click asks the item to
  activate, act otherwise or show its menu, and the wheel scrolls it. A
  menu the item serves over dbusmenu is shown with vaht's menus, told it
  opens (`AboutToShow`) and told what is chosen: on a right click, and on
  a left click when the item is only a menu (`ItemIsMenu`) or has no action
  of its own (its `Activate` fails), as fluxland did; an item without one is
  asked to show its own. The taskbar spans the output; clicking an arrow shows the
  workspace before or after, clicking an entry brings its window forward
  or, if it has focus, minimizes it, and the `taskbar` and
  `window-list-entry` mouse bindings take every other press. A fullscreen
  window covers it.

A window is placed when its client first commits a size, so the smart
strategies know how large it is.

### 3.3 Settings dropped in conversion

| fluxland key | Why |
|---|---|
| `tabs.*`, `tab.*`, `tabFocusModel`, `tabsAttachArea`, `tabPadding` | Tabs removed |
| `slit.*`, `slitlistFile` | Slit removed |
| `*.alpha: 255` (window, menu, toolbar, slit) | Fully opaque is the default. Other values are kept and moved to the style, since opacity is appearance |
| `colorsPerChannel`, `cacheMax`, `cacheLife`, `configVersion`, `forcePseudoTransparency` | Have no effect on Wayland, or are already ignored by fluxland |
| `ignoreBorder` | The border is always part of the frame's move handle |
| `appsFile`, `keyFile`, `menuFile`, `windowMenu`, `styleFile`, `styleOverlay`, `slitlistFile` | Fixed file names (§1.4) |
| `toolbar.autoRaise`, `slit.autoRaise` | They raise a dock-layer panel that is already above windows |

---

## 4. Commands (shared by bindings, menus and IPC)

A command is written in one of three forms:

1. **An action**, as a string or as a table (both accepted, decided):

   | String | Table |
   |---|---|
   | `"close"` | `{ close = {} }` |
   | `"exec kitty"` | `{ exec = "kitty" }` |
   | `"workspace 3"` | `{ workspace = 3 }` |
   | `"snap-step left"` | `{ snap-step = "left" }` |
   | `"start-resizing nearest-corner"` | `{ start-resizing = "nearest-corner" }` |
   | `"set-setting taskbar.auto_hide true"` | `{ set-setting = { key = "taskbar.auto_hide", value = true } }` |

   - **One parser for both.** Each `Action` variant's serde form is the table;
     the string form is split into a name and its arguments and fed through
     the same typed parsing, so the two cannot disagree. A test round-trips
     every action through both forms.
   - **Writing.** Wherever vaht writes config (`vaht import`, SaveSettings),
     it uses the string form, so generated files look alike (P8).
   - **Reserved keys.** The structured forms below (`toggle`, `if`,
     `for_each`, `delay_ms`, `repeat`, `action`) are never action names, so a
     table is never ambiguous.
2. **An array**, meaning a sequence (MacroCmd): `["raise", "focus", "start-moving"]`.
3. **A table**, for the structured forms:

   | Table | Meaning (Fluxbox) |
   |---|---|
   | `{ toggle = [cmd, cmd, ...] }` | ToggleCmd |
   | `{ if = condition, then = cmd, else = cmd }` | If / Cond |
   | `{ for_each = matcher, do = cmd }` | ForEach / Map |
   | `{ delay_ms = 300, do = cmd }` | Delay |
   | `{ repeat = cmd, rate = 20 }` | Repeat (rate per second, optional) |
   | `{ action = "next-window", windows = matcher, order = "opening" }` | Action plus a window matcher argument; `order` is `focus` (the default: most recently focused first) or `opening` (the order windows opened in, as the taskbar lists them: Fluxbox's `{static}`) |

   A **condition** is a window matcher (§5), or `{ all = [..] }`,
   `{ any = [..] }`, or `{ not = .. }`.

**Action names (abridged).** All tab actions and Fluxbox aliases are gone.
`exec` is the only way to run a program.

| Area | Actions |
|---|---|
| Programs and session | `exec <command line>`, `reload`, `exit`, `set-environment NAME value`, `set-setting key value`, `unset-setting key`, `save-settings` |
| Window state | `close`, `kill`, `minimize`, `maximize [horizontal\|vertical]`, `fullscreen`, `shade`, `stick`, `raise`, `lower`, `set-layer <layer>`, `set-alpha <n>`, `toggle-frame`, `set-title [title]`, `set-title-dialog`, `mark-window <key>`, `go-to-marked-window <key>` |
| Window placement | `move <dx> <dy>`, `move-to <x> <y>`, `resize <dw> <dh>`, `resize-to <w> <h>`, `snap-step <direction>`, `tile <cell>`, `send-to-output next\|previous\|<n>`, `arrange <style>` |
| Grabs | `start-moving`, `start-resizing [nearest-corner\|<corner>]` |
| Focus | `focus`, `next-window`, `previous-window`, `focus-direction <direction>`, `show-desktop` |
| Workspaces | `workspace <n\|next\|previous\|last>`, `send-to-workspace <target>`, `take-to-workspace <target>`, `add-workspace`, `remove-last-workspace`, `rename-workspace` |
| Keys | `key-mode <name> [<end chord>]` |
| Menus and dialogs | `root-menu`, `window-menu`, `workspace-menu`, `window-list-menu`, `custom-menu <name>`, `hide-menus`, `run-dialog`, `command-dialog`, `settings` |
| Taskbar | `activate-window-list-entry <n>`, `activate-tray-item <n>`, `tray-item-menu <n>`, `toggle-taskbar`, `taskbar-to-output <target>` |
| Outputs | `apply-profile <name>`, `cycle-profile`, `cycle-primary-output` |
| Style | `set-style <name>` |

The complete table is generated from the `Action` enum's documentation, so it
cannot drift (D1).

---

## 5. Window matchers

These are used by rules, conditions, `for_each`, `next-window` and the window
list:

```toml
{ app_id = "kitty" }                              # exact: Wayland app_id, or X11 WM_CLASS class
{ app_id = "steam", title = { regex = "^Steam$" } }
{ instance = "Navigator", role = "browser" }      # X11 instance and role; also tag, layer, output
{ workspace = "current" }                         # or a number; output = "current" | name
{ minimized = false, hidden_from_window_list = false }
{ not = { app_id = "kitty" } }
```

- Plain strings match exactly. `{ regex = "..." }` is an anchored regex,
  compiled once at load. Fluxbox patterns were always regexes, which made
  `firefox.desktop` match `firefox_desktop`.
- All keys must match, so a matcher with several keys means "and".
- The `{N}` count from Fluxbox patterns is dropped; nothing in my setup uses
  it.
- `vaht import` turns a bare Fluxbox pattern (`(xterm)`), which Fluxbox
  matches against the `name` property, and `(name=...)` into an `app_id`
  regex matched in any letter case (`(?i)^(?:xterm)$`): WM_CLASS's instance
  is most often its class in other letters, and vaht keeps only the class.
- Built so far: `app_id`, `title`, `workspace`, `minimized`, `sticky`,
  `transient` (whether it is a dialog of another window),
  `hidden_from_window_list` and `not`. The X11 keys, `layer` and `output`
  come with the features they describe.

---

## 6. `bindings.toml`

My `keys`, converted (repetitive lines shortened):

```toml
# Keys: modifiers Super, Alt, Ctrl, Shift joined by "+", then an xkb key name.
# A chain is several chords separated by spaces: "Super+A Super+B".
[keys]
"Alt+Tab"         = { action = "next-window",     windows = { workspace = "current" } }
"Alt+Shift+Tab"   = { action = "previous-window", windows = { workspace = "current" } }
"Super+R"         = "run-dialog"
"Super+D"         = "exec discord"
"Super+T"         = "exec kitty"
"Super+B"         = "exec brave"
"Super+S"         = "exec spotify-launcher"
"Super+L"         = "exec xlock -mode noof -delay 7500"
"Super+minus"     = "cycle-profile"           # was av-output-switcher --cycle-profile
"Super+backslash" = "cycle-primary-output"    # was --cycle-primary
"Super+apostrophe" = "exec av-output-switcher --cycle-pactl-sink"   # audio stays external

"XF86AudioRaiseVolume"     = { repeat = "exec amixer sset Master 2%+" }
"XF86AudioLowerVolume"     = { repeat = "exec amixer sset Master 2%-" }
"XF86AudioMute"            = "exec amixer sset Master toggle"
"Alt+XF86AudioRaiseVolume" = { repeat = "exec mpris-control volume 0.02+" }
"Alt+XF86AudioLowerVolume" = { repeat = "exec mpris-control volume 0.02-" }
"Alt+XF86AudioPrev"        = { repeat = "exec mpris-control position 5-" }
"Alt+XF86AudioNext"        = { repeat = "exec mpris-control position 5+" }
"XF86AudioNext" = "exec mpris-control next"
"XF86AudioPlay" = "exec mpris-control play-pause"
"XF86AudioPrev" = "exec mpris-control previous"
"XF86AudioStop" = "exec mpris-control stop"

"Alt+F4"    = "close"
"Alt+F5"    = "kill"
"Alt+F9"    = "minimize"
"Alt+F10"   = "maximize"
"Alt+F11"   = "fullscreen"
"Alt+space" = "window-menu"

"Ctrl+Alt+Left"     = "workspace previous"
"Ctrl+Alt+Right"    = "workspace next"
"Super+Prior"       = "workspace previous"
"Super+Next"        = "workspace next"
"Super+Shift+Prior" = "take-to-workspace previous"
"Super+Shift+Next"  = "take-to-workspace next"
"Ctrl+Super+Left"   = "take-to-workspace previous"
"Ctrl+Super+Right"  = "take-to-workspace next"
"Super+Left"  = "snap-step left"
"Super+Right" = "snap-step right"
"Super+Up"    = "snap-step up"
"Super+Down"  = "snap-step down"
"Ctrl+F1"       = "workspace 1"            # ... through F12
"Super+F1"      = "send-to-workspace 1"    # ... through F12
"Ctrl+Super+F1" = "take-to-workspace 1"    # ... through F12

# Key modes: [key_modes.<name>] holds the only keys bound after
# "key-mode <name>", until Escape (or the end chord "key-mode <name> <chord>"
# gives), unbound in the mode, goes back to [keys]. For example:
#   "Super+M" = "key-mode moving"
#   [key_modes.moving]
#   "Left" = "move -10 0"

# Mouse: [mouse.<context>]. Contexts: desktop, taskbar, window-list-entry,
# titlebar, window (anywhere on a window, normally with a modifier), border,
# grip-left, grip-right, handle.
# Events: Left, Middle, Right, ScrollUp, ScrollDown, Back, Forward; prefix
# "Double+" for a double click, "Drag+" for a drag that starts with that
# button (Fluxbox's MoveN).
[mouse.desktop]
"Left"       = "hide-menus"
"Middle"     = "workspace-menu"
"Right"      = "root-menu"
"ScrollUp"   = "workspace previous"
"ScrollDown" = "workspace next"

[mouse.taskbar]
"ScrollUp"   = { action = "previous-window", windows = { minimized = "any", hidden_from_window_list = false } }
"ScrollDown" = { action = "next-window",     windows = { minimized = "any", hidden_from_window_list = false } }

[mouse.window]
"Alt+Left"   = ["raise", "focus", "start-moving"]
"Alt+Right"  = ["raise", "focus", "start-resizing nearest-corner"]
"Alt+Middle" = "lower"

[mouse.grip-left]
"Drag+Left" = "start-resizing bottom-left"

[mouse.grip-right]
"Drag+Left" = "start-resizing bottom-right"

[mouse.titlebar]
"Double+Left" = "maximize"
"Left"        = ["raise", "focus"]
"Drag+Left"   = "start-moving"
"Middle"      = "lower"
"Right"       = "window-menu"

# Switches (fluxland's session.switch.*, which vaht import moves here):
# lid-close, lid-open, tablet-mode-on, tablet-mode-off.
[switches]
lid-close = "exec swaylock"
```

**How bindings behave (as built).**
- `bindings.toml` replaces the built-in bindings as a whole, as Fluxbox's
  `keys` file does. `vaht config check --print-defaults` prints the built-in file to
  start from.
- A bound key press, and its release, never reach the focused window.
- A bound button press never reaches the window either: vaht holds the
  pointer until the button is released. A `Drag+` binding runs once the
  pointer has moved 3 pixels.
- `Double+` takes the place of the plain binding on the second press within
  250 ms. With no `Double+` binding, the plain one runs on both presses.
- `repeat` runs its command on the press, then again after the keyboard's
  repeat delay at its own rate, until any key is pressed or that key is
  released. Mouse bindings cannot repeat.
- A `[switches]` binding runs, for the focused window, when a laptop's lid
  or a convertible's tablet mode flips (on the DRM backend, which hears
  switches from libinput).
- Built into the frame, not bindings: titlebar buttons (pressed without
  modifiers), and, for a button the part has no binding for, dragging a
  border moves the window, as Fluxbox's does (`ignoreBorder` off), and
  dragging the handle or a grip resizes it. A `[mouse.border]` binding of
  `start-resizing nearest-corner` makes borders resize.
- Every action of §4 is built. The structured forms: a `toggle` runs its
  commands in turn, one per run (a reload starts it over); `if` asks its
  condition of the window the command is about (none never matches);
  `for_each` runs its command on each placed window matching, as the
  command's window; and `delay_ms` runs its command that long later, put
  off again if run again first.
- `activate-window-list-entry <n>` does what clicking the taskbar's `n`th
  window list entry from the left does: it activates the window, or
  minimizes it if it has focus. `activate-tray-item <n>` and
  `tray-item-menu <n>` ask the `n`th tray item for its main action or its
  menu, as a left or right click does. `taskbar-to-output
  next|previous|<head>|<connector>` shows the taskbar on another output,
  heads counted as `send-to-output` counts them; it is a change of
  `taskbar.output` in place, as `set-setting` makes, which `save-settings`
  writes.
- A chain (`"Super+X Super+T"`) runs once its chords are pressed in turn,
  each within 5 seconds of the last, as Fluxbox waits. Escape gives up on
  a chain, as does a chord it does not go on with, which then reaches the
  focused window as an unbound key does; modifiers pressed for the next
  chord do neither. A chain's beginning cannot be bound too, and a chain
  cannot repeat. A key mode's bindings, in `[key_modes.<name>]`, are the
  only ones bound while it lasts; keys it leaves unbound reach the
  window. `key-mode default` goes back to `[keys]`, as does the end
  chord, and a reload that drops the mode. `vaht config check` rejects a
  `key-mode` naming a mode the file does not have.
- `focus-direction <left|right|up|down>` focuses and raises the nearest
  shown window whose middle lies that way, within 45 degrees, from the
  focused one's (Fluxbox's `FocusLeft` and its kin). `mark-window <key>`
  marks the window with a word, one window per key, and
  `go-to-marked-window <key>` brings it forward. `send-to-output
  next|previous|<head>` moves the window to another output, heads
  numbered from 1 left to right then top to bottom: a floating window to
  the same place in proportion, a maximized or tiled one filling its part
  there (Fluxbox's `SendToNextHead`, `SendToPrevHead`, `SetHead`).
  `toggle-frame` shows the window without its frame, its content staying
  where it is, or with it again (Fluxbox's `ToggleDecor`).
  `toggle-taskbar` turns the taskbar off, giving back the room it
  reserved and resting its clock, or on again (Fluxbox's
  `ToggleToolbarHidden`). `rename-workspace <name>` names the workspace
  shown, until the settings are read again (Fluxbox's
  `SetWorkspaceName`). `arrange grid|columns|rows` tiles the windows shown
  on the window's output, in the order they opened: a grid with the last
  row sharing its width, side by side, or one above another (Fluxbox's
  `ArrangeWindows`, `ArrangeWindowsVertical`, `ArrangeWindowsHorizontal`);
  each can be restored as any tiled window can.
- `set-setting <key> <value>` changes a setting of `settings.toml` in
  place: the key is dotted (`taskbar.auto_hide`, `input.device."My
  Mouse".accel_speed`), the value TOML (`true`, `300`, `"sloppy"`) or a
  bare word taken as a string. `vaht config check` refuses a key or value
  `settings.toml` could not hold, and a change that leaves the whole
  configuration invalid (a style that is not there) is not made. As with
  `set-style`, which is `set-setting style <name>`, the change outlasts
  reloads until the file says otherwise at that key, and `save-settings`
  writes every change into `settings.toml`, keeping the rest of the file.
  `reapply-rules` gives the windows open the traits `rules.toml` now says
  (frame, layer, stickiness, opacity, pseudo-transparency, exemptions,
  focus stealing); where a window goes and how large it is are only for a
  window opening. `restart` and `reload-style` are refused with a pointer to `reload`,
  which reads every file, the style too; a Wayland compositor cannot
  restart without closing every window.
- `tile` takes a cell in thousandths of the usable area:
  `tile <x> <y> <width> <height>`, the left half being `tile 0 0 500 1000`.
- `next-window` and `previous-window` walk windows in the order they last
  had focus. The walk lasts while modifiers are held, and when they are
  let go the window reached becomes the most recent. Minimized windows
  count only if the matcher says `minimized = true` or `"any"`, and
  reaching one restores it. A window on another workspace shows that
  workspace.
- `snap-step` follows my fluxland ladder. A step down that minimizes can be
  undone by a step up while the modifiers are still held.

Dropped in conversion, and reported by `vaht import`:
- `Mod4 Tab`, `Mod4 Shift Tab`, `Mod4 1`–`9`, `OnTitlebar Control Mouse1 :StartTabbing` (tabs removed);
- the `ActivateTab` part of the titlebar click.

Key names are xkb keysym names. Both `Super` and `Mod4` are accepted when
parsing, but `vaht` itself always writes `Super`.

---

## 7. `rules.toml`

```toml
# Only kitty shows the wallpaper through its translucency
[[rule]]
match = { app_id = "kitty" }
pseudo_transparent = true

# Steam's tray icon cannot pass a focus token; let its windows take focus
[[rule]]
match = { app_id = "steam" }
focus_stealing = "allow"        # FocusProtection {Gain}; also "deny", "default"
```

- The run dialog's application id is `vaht-run`. `vaht import` turns a rule
  for `fluxland-run` or `fbrun` into one for it, so my centering rule carries
  over.
- **Rule keys:**
  - placement: `workspace`, `output`, `layer`, `position` (with an anchor and
    offset), `size`, `placement` (`maximized`, `fullscreen`, `tile <cell>`);
  - window state: `minimized`, `sticky`, `shaded`;
  - frame and appearance: `frame` (`full`, `titlebar-only`, `border-only`,
    `none`), `alpha` (focused, unfocused);
  - behavior: `focus_stealing`, `hidden_from_window_list`,
    `hidden_from_cycling`, `pseudo_transparent`, `ignore_size_hints`,
    `refuse_maximize`, `refuse_fullscreen`.
- Rules apply in file order, and later matching rules override earlier ones
  key by key.
- **Built so far:** `workspace`, `sticky`, `minimized`, `placement`
  (`maximized`, `maximized horizontal|vertical`, `fullscreen`, `tile <x> <y>
  <width> <height>`), `position = { anchor, offset }`, `size` (the frame's
  size, as fluxland's `[Dimensions]` was) and `pseudo_transparent` (where
  the window is translucent, only the wallpaper shows through: a copy of
  the wallpaper under its content hides the windows below) and `layer`
  (`above-dock`, `dock`, `top`, `normal`, `bottom`, `desktop`: Fluxbox's
  layers; a window is always above those of lower layers, raising and
  lowering stay within its own, and `above-dock` windows are over the
  taskbar; `set-layer` moves one), and `focus_stealing` (`allow`: the
  window comes forward whenever its program asks, as Steam's tray menu
  needs, from `[FocusProtection] {Gain}`; `deny`: never by its program's
  asking; `default`: when the focused program made the token, recently.
  It decides a new window's first focus too: `allow` takes it even when
  `focus_new_windows` is off, `deny` gives it back to the window that had
  it), `output` (a head number, from 1 left to right as Fluxbox's
  `[Head]`, or a connector's name), `shaded`, `frame` (`full` or `none`;
  `full` frames even a window whose program draws its own, as Fluxbox's
  `[Deco]` does, though the program's own stays too; without a rule, a
  Wayland program asking to draw its own through xdg-decoration, as
  Chromium and Firefox do, and an X program asking for no decorations,
  get no frame; `titlebar-only` and `border-only` are refused until frames
  can be drawn so), `alpha` (one opacity, or `[focused, unfocused]`, 0 to 255),
  `hidden_from_window_list` (left off the taskbar, `[IconHidden]`),
  `hidden_from_cycling` (`[FocusHidden]`; Fluxbox's `[Hidden]` sets
  both), `ignore_size_hints` (resizing by hand is not held to the sizes
  the program accepts) and `refuse_maximize` / `refuse_fullscreen` (the
  program's asking is refused; the user can still do either). They apply
  when the window first gets its place, an `output` before its position.
  The other keys are refused until their features exist.
  `vaht import fluxbox` converts `apps` (workspaces counted from 0 there)
  and reports the rest.

---

## 8. `menus.toml`

Menus are named tables. Submenus refer to other menus by name, which keeps
deep menus flat and readable in TOML.

```toml
[menu.root]
title = "vaht"
items = [
    { label = "kitty",   command = "exec kitty" },
    { label = "brave",   command = "exec brave" },
    { label = "steam",   command = "exec steam" },
    { label = "discord", command = "exec discord" },
    { label = "spotify", command = "exec spotify-launcher" },
    { label = "Misc",    submenu = "misc" },
    { label = "vaht",    submenu = "system" },
]

[menu.misc]
items = [{ label = "xterm", command = "exec xterm" }]

[menu.system]
items = [
    { label = "Styles",     generate = "styles" },
    { label = "Workspaces", generate = "workspaces" },
    { label = "Command",    command = "command-dialog" },
    { label = "Reload",     command = "reload" },
    { separator = true },
    { label = "Exit",       command = "exit" },
]

[menu.window]       # my windowmenu
items = [
    { toggle = "shade" }, { toggle = "stick" }, { toggle = "maximize" },
    { toggle = "minimize" }, { command = "raise" }, { command = "lower" },
    { command = "set-title" }, { generate = "send-to-workspace" },
    { generate = "layer" }, { generate = "alpha" },
    { separator = true }, { command = "close" },
]
```

- **Item forms:**
  - `command` (with a `label`; a missing label defaults to the action's name);
  - `submenu`;
  - `toggle` (a window state shown with a check mark);
  - `generate`: `styles`, `workspaces`, `window-list`, `send-to-workspace`,
    `layer`, `alpha`, `outputs`, `profiles`;
  - `pipe` (a command whose output is a menu in this same TOML form);
  - `separator`;
  - `icon` (optional on any item).
- **Built so far:** `command`, `submenu`, `toggle` (`shade`, `stick`,
  `maximize`, `minimize`, `fullscreen`), `generate` (`workspaces`,
  `send-to-workspace`, `window-list`, `alpha`, `layer`, `styles`; `inline =
  true` lists in place),
  `separator`, and a bare `label` (drawn disabled). The actions
  `root-menu`, `window-menu`, `workspace-menu`, `window-list-menu`,
  `custom-menu <name>` and `hide-menus`, bound as Fluxbox binds them by
  default. Menus open with their corner at the pointer and stay on the
  output; a submenu opens beside its item after `[menus] submenu_delay_ms`
  or at once on a click; releasing a button on an item chooses it; a press
  elsewhere closes the menus and goes on. A menu stays in the usable
  area, off the taskbar; the action that opened it, run again while it is
  open, closes it, as in fluxland. A `custom-menu` naming no menu is an
  error when the files are read. Keys: Up, Down, Left, Right,
  Enter, Escape, and typing: `[menus] search` (`item-start`, `anywhere` or
  `off`, from Fluxbox's `menuSearch`) highlights the first item found by
  what was typed, a letter finding nothing is not kept, and Backspace takes
  one back. `[menus] window_icons` (from `clientMenu.usePixmap`) shows
  each window's icon in lists of windows. `generate` also lists the
  display profiles (`profiles`, the one applied checked) and the outputs
  (`outputs`, choosing one makes it primary). An item running a command
  or opening a submenu may show an `icon`: an icon theme's name for it
  (`firefox`), or an image file if it has a `/` in it (`~/` is the home
  directory, a relative path is from the configuration directory);
  Fluxbox's `<icon>` on `[exec]` and `[submenu]` converts. A pipe menu,
  `{ label = "Recent", pipe = "~/bin/recent" }`, runs its command with the
  shell each time it opens and shows what it prints: TOML with `items` (as
  a `[menu.<name>]` table holds them, `pipe` items too) and optionally a
  `title`. It shows "…" while the command runs, read as it prints so a
  slow one never stalls vaht; "(no menu)" if the output is not a menu or
  passes 256 KiB; and nothing changes if the menu closed meanwhile. Not
  yet: the GTK presenter.
- **Dropped:**
  - Fluxbox's `[config]` editor menu: settings are reached through
    `set-setting`.
  - `[extramenus]`: nothing in vaht adds menus to it.
  - The fluxbox-generate_menu header and the `[encoding]` tag: TOML is UTF-8.

---

## 9. `outputs.toml`

```toml
# vaht prints a ready-to-paste `match` for each output: vaht outputs
[[output]]
match = { connector = "HDMI-A-1" }    # or { make = "...", model = "...", serial = "..." }
mode = "1920x1080@60"                 # or "1920x1080": the fastest refresh
position = [0, 0]
scale = 1.0                           # 0.25 to 8; windows on it are told it
transform = "normal"                  # 90, 180, 270, flipped, flipped-90, ...
primary = true
vrr = "fullscreen"                    # off | on | fullscreen (default)
hdr = false                           # new: HDR10, if the monitor can
sdr_white = 203                       # in HDR: SDR content's white, cd/m²
# hdr_peak = 600                      # in HDR: the peak, when the EDID is wrong

[[output]]
match = { connector = "DP-2" }
mode = "1920x1080@60"
position = [1920, 0]

[[output]]
match = { connector = "eDP-1" }
enabled = false

# Display profiles (av-output-switcher's video half). Optional.
[[profile]]
name = "desk"
when_connected = [{ connector = "HDMI-A-1" }, { connector = "DP-2" }]
primary = [{ connector = "HDMI-A-1" }, { connector = "DP-2" }]   # candidates, in order
outputs = [
    { match = { connector = "HDMI-A-1" }, mode = "1920x1080@60", position = [0, 0] },
    { match = { connector = "DP-2" },     mode = "1920x1080@60", position = [1920, 0] },
]
# outputs not listed are turned off while this profile is applied

[hooks]
profile_applied = "~/bin/on-profile"           # the profile's name as $1
primary_output_changed = "~/bin/on-primary"    # the output's connector as $1
```

- **Built so far:** `[[output]]`, applied at start and on every reload:
  the first entry matching an output (every field of its `match` agrees)
  is the one it follows. An output no entry matches keeps its preferred
  mode and scale 1; outputs without a `position` go to the right of those
  with one, in the order found. Without an entry saying `primary`, the
  primary output is Fluxbox's head 1: the leftmost, then topmost. If
  every output would be off, the first stays on. Windows on an output
  turned off move to the primary one. The nested backend's one output
  is the host window: it cannot be turned off, and its size is its mode.
  `vrr` is read; only the DRM backend will act on it.
- **HDR (built, new).** `hdr = true` drives a monitor whose EDID says it
  takes PQ in BT.2020 RGB in HDR10: vaht sets the connector's `Colorspace`
  to BT.2020 RGB and its `HDR_OUTPUT_METADATA` (the EDID's peak, or
  `hdr_peak`; its black and frame average), redraws everything in PQ, and
  tells programs (`wp_color_manager_v1`) the output shows BT.2100 PQ, so
  HDR games and video send PQ and show as they were mastered. SDR content
  (windows, vaht's own panels) is shown with its white at `sdr_white`
  (80 to 1000 cd/m², 203 by default, BT.2408's reference white); HDR
  content brighter than the monitor rolls off to its peak. A monitor that
  cannot is left in SDR; one the kernel refuses (a frame on its way) is
  asked again each frame, the refusal logged once. Leaving vaht takes
  every monitor out of HDR. Only the DRM backend acts on it.
  Screenshots and streams stay sRGB, HDR content tone-mapped into them.
  Alpha is blended in PQ rather than linear light, so translucent edges
  differ slightly from SDR.
- **Profiles (built).** A profile applies itself at start, on reload and
  when outputs come and go, if every output its `when_connected` lists is
  there (the one listing most wins, the first written on a tie); a
  profile applied stays while its outputs are connected. `apply-profile
  <name>`, `cycle-profile` (the next one written whose outputs are all
  connected) and `cycle-primary-output` (through the profile's `primary`
  candidates, else the outputs on, left to right) act at once;
  `set-primary-output <connector>` picks one. A primary output chosen so
  stays while it is on. Menus list them with `generate = "profiles"` and
  `generate = "outputs"`.
- **Hooks (built).** Each runs with the shell, given the profile's name
  or the primary output's connector as its one argument (`$1`), as
  av-output-switcher's hooks are: `profile_applied` after a profile is
  applied, `primary_output_changed` after the primary output changes,
  and both at start. `vaht outputs --json` gives the whole layout.
- **How the two sections combine.** `[[output]]` entries are the baseline.
  When `when_connected` selects a profile, that profile overrides the baseline
  for the outputs it lists. With no profiles, the baseline is everything.
- **What `vaht import` does.**
  - It converts fluxland's `session.output.*` into `[[output]]` entries, using
    connector names, because those are all fluxland records
    (`adaptiveSync` becomes `vrr` `on` or `off`). `toolbar.onoutput`
    becomes `[taskbar] output`, and so does `toolbar.onhead` past 1 when
    the positions in `init` say which output that head is.
- **av-output-switcher gets no importer (decided).** It has one user, so
  my `profiles.json` was converted to `[[profile]]` entries by hand. Its
  audio half (`pactl_sink_option_regexes`, `--cycle-pactl-sink`) stays
  with av-output-switcher.

---

**Windows and monitors coming and going (as built).** When a monitor
leaves, its windows come onto the monitors left. vaht remembers where each
was, by the monitor's name; when a monitor of that name is plugged in
again, each window still where it was brought goes back to its place
there (a maximized or tiled one fills its part of it), and one moved in
the meantime stays where it was put (`output_memory.rs`).

## 10. Styles: `styles/<name>.toml`

### 10.1 Model

A style gives a `Style` to each **element** in each **state** (01 §6). Styles
are resolved in four layers, each overriding the one before:

1. `[default]`: applies to everything (`*.font`, `*.color` in Fluxbox).
2. `[state.<state>]`: one state, any element (`*.unfocused.textColor`).
3. `[<element>]`: one element, any state.
4. `[<element>.<state>]`: one element, one state.

These four fixed layers replace Fluxbox's wildcard resource matching.
`vaht config check --style <name>` prints the fully resolved table, so it shows
exactly what each element gets.

- **Elements:**
  - window: `window.titlebar`, `window.label`, `window.button`,
    `window.handle`, `window.grip`, `window.border`;
  - taskbar: `taskbar`, `taskbar.workspace`, `taskbar.clock`,
    `taskbar.window-list.entry`, `taskbar.window-list.empty`, `taskbar.tray`;
  - menus: `menu.title`, `menu.frame`, `menu.item`;
  - dialogs: `dialog.field`, the line a dialog edits (its cursor takes the
    text color);
  - other: `tooltip`, `snap-picker`, `wireframe`.
- **States:** `focused`, `unfocused`, `hovered`, `pressed`, `active`,
  `disabled`. A menu item's highlight is its `hovered` state.
- **Style keys:**
  - `texture`: `{ solid = "#rrggbb" }`, or
    `{ gradient = "vertical", from, to, bevel = "raised"|"sunken", interlaced }`,
    or `{ image = "file.xpm" }` (an XPM beside the style file, as Fluxbox styles
    use, stretched to fill the element; `fit = "tile"` is not built yet), or
    `"parent"`;
  - text: `font = { family, size, weight, style }`, `text_color`,
    `text_effect` (`{ halo = color }` or `{ shadow = color, offset = [x, y] }`),
    `justify`;
  - menu items: `bullet` (`empty`, `triangle`, `square`, `diamond`: a
    submenu's mark, in the text color) and `bullet_position` (`left` or
    `right`: where it and a chosen toggle's check go; the check takes
    `glyph_color`), from Fluxbox's `menu.bullet`, `menu.bullet.position`
    and `menu.hilite.color`;
  - box: `border = { width, color }`, `padding = [top, right, bottom, left]`,
    `corner_radius`, `corners`, `alpha`;
  - sizes: `height` for bars and items;
  - glyphs: `glyph_color` for button pictures, and `glyph_images = { close =
    "file.xpm", minimize, maximize, restore, shade, stick }` to draw a button
    from an image of the style's own instead (the whole button: stretched over
    it, the state's own in `[window.button.pressed]` and the like). Optional, as
    every property is: a glyph without an image is drawn as before;
- **Colors are `#rrggbb`, `#rrggbbaa`, or an X11 color name** such as `grey55`
  or `SteelBlue` (decided).
  - The X11 list (`rgb.txt`) is large, about 750 names, but it has been frozen
    since the 1990s, so it needs no maintenance.
  - The importer needs it anyway to read fluxbox styles, so accepting names in
    styles reuses the same table. That table is a static, sorted array in
    `vaht-ui::color`, generated once from X.org's `rgb.txt` and checked against
    fluxland's `x11_colors.h` in a test.
  - The published crates for this are tiny and little used (the most popular
    has fewer than 200 downloads), so vaht doesn't add a dependency for it (X2).
  - `vaht import` keeps names as written, so my converted style still says
    `grey55`.

**Built so far.**
- `settings.toml`'s `style = "<name>"` reads `styles/<name>.toml` from the
  configuration directory, then from `/usr/share/vaht/styles/`. The file is
  watched like the others: editing it restyles every open window.
- Elements: the window's six, and `taskbar`, `taskbar.button`,
  `taskbar.workspace`, `taskbar.clock`, `taskbar.window-list.entry`,
  `taskbar.window-list.empty`, `menu.title`, `menu.frame`, `menu.item`,
  `dialog.field`, `wireframe` (a resize's outline, drawn in its
  `border`) and `tooltip` (converted from fluxland's `systray.tooltip.*`,
  rounded as `corner_radius` says, as frames and menus are). `alpha`, 0
  (clear) to 255 (opaque), draws the whole of `taskbar`, a menu (in
  `menu.frame`) or a tooltip translucent, as Fluxbox's `toolbar.alpha`,
  `menu.alpha` and fluxland's `systray.tooltip.alpha`, which `vaht import`
  converts into the style. States: `focused`, `unfocused`, `pressed`, `hovered`,
  `disabled`.
- Properties: `texture` (solid or gradient, with a bevel), `font`,
  `text_color`, `text_effect`, `justify`, `glyph_color`, `padding`,
  `border` and `height`. Window borders take their color and width from
  `window.border`'s `border`.
- Rounded corners: `corner_radius` (pixels) and `corners` (which of
  `top-left`, `top-right`, `bottom-left`, `bottom-right`; all four when
  left out), on `window.border` alone so far: they round the frame. A top
  corner rounds within the titlebar and a bottom one within the handle,
  so window content is never cut; without a handle the bottom stays
  square, as in Fluxbox. The importer turns `window.roundCorners` into
  radius 8, which cuts as much as Fluxbox's fixed 8-pixel shape.
- Properties, states and elements still to come are refused by name, at
  their line in the file.
- The default style is itself a style file, `vaht` (`data/styles/vaht.toml`
  in the source): installed as `/usr/share/vaht/styles/vaht.toml` for people to
  read and copy, and built into vaht, so it needs no file; printed by
  `vaht config check --print-defaults`. With no `style` chosen, `vaht` is the
  style, and it is found like any other: a `vaht.toml` in your `styles/`
  directory first, then the installed one, and the copy built in only when
  neither exists. So editing a `vaht.toml` changes the default style. (The
  config-problem banner is the exception: it always uses the copy built in, so
  a style of yours cannot hide it. `/usr/share/vaht/styles` is not watched, so
  an edit there is read on the next reload.)
- A style file is a whole style: what it leaves out takes vaht's fallback,
  not the built-in style's value.

### 10.2 `nacitar_fbstyle`, converted

Shown in hex, with the original names in comments, so the values can be
checked. The importer itself would keep the names (`from = "grey55"`).

```toml
# styles/nacitar.toml (from nacitar_fbstyle by nacitar sevaht)
[default]
font = { family = "DejaVu Sans", size = 8, weight = "bold" }
text_effect = { halo = "#000000" }
text_color = "#ffffff"
glyph_color = "#ffffff"
justify = "center"
texture = { gradient = "vertical", from = "#4d4d4d", to = "#0d0d0d" }   # grey30 to grey5
border = { width = 0, color = "#999999" }                               # grey60

[state.focused]
texture = { gradient = "vertical", from = "#8c8c8c", to = "#0d0d0d" }   # grey55 to grey5

[state.unfocused]
text_color = "#8c8c8c"
glyph_color = "#8c8c8c"

[state.pressed]
texture = { gradient = "cross-diagonal", from = "#333333", to = "#666666", bevel = "sunken" }

[state.disabled]
text_color = "#8c8c8c"

[window.titlebar]
height = 20

[window.border]
border = { width = 1, color = "#999999" }

[window.handle]
height = 1
texture = { gradient = "vertical", from = "#ffffff", to = "#999999", bevel = "raised" }

[window.grip]
texture = { gradient = "vertical", from = "#ffffff", to = "#999999", bevel = "raised" }

[taskbar]
height = 20

[taskbar.window-list.entry.focused]
texture = { gradient = "vertical", from = "#8c8c8c", to = "#0d0d0d", bevel = "raised" }

[taskbar.window-list.entry.unfocused]
texture = { gradient = "vertical", from = "#4d4d4d", to = "#0d0d0d", bevel = "sunken" }

[menu.frame]
font = { family = "DejaVu Sans", size = 8 }
justify = "left"
texture = { solid = "#000000" }
border = { width = 1, color = "#999999" }
corner_radius = 4
submenu_indicator = { glyph = "triangle", side = "right" }

[menu.title]
height = 20

[menu.item]
height = 20
justify = "left"

[menu.item.hovered]
texture = { gradient = "vertical", from = "#8c8c8c", to = "#0d0d0d", bevel = "raised" }

[tooltip]
font = { family = "DejaVu Sans", size = 11 }
text_effect = "none"
texture = { solid = "#000000" }
border = { width = 1, color = "#191919" }
padding = [8, 14, 8, 13]
corner_radius = 5
```

**What changed in the conversion.**
- `window.font.effect: none` was there only for fluxland's clock-format
  dialog. Dialogs are now client windows, so it goes away.
- The tray tooltip settings became the `tooltip` element, which covers every
  tooltip (00 §5).
- `bevelWidth: 0` and `toolbar.shaped: false` are the defaults.

---

### 10.3 Layouts: `layouts/<name>.toml`

A layout is a named set of *presentation* settings: what is shown and how it is
arranged, as opposed to how it is colored (a style) or how vaht behaves. It is a
partial `settings.toml`, in the same shape:

```toml
# layouts/slim.toml
[taskbar]
rows = 1
widgets = ["window-list", "clock"]
```

`settings.toml` chooses one with `layout = "slim"`, found like a style: the
configuration directory's `layouts/`, then `/usr/share/vaht/layouts`.

**Precedence, lowest to highest:** vaht's defaults, the layout, `settings.toml`,
then changes made in place (`set-setting`, the settings window). `settings.toml`
wins, so it is the user's overrides: it names only what differs from the layout,
and swapping the layout (`set-layout <name>`, the `layouts` menu list, the
settings window) never rewrites it. Tables merge key by key; a list or a value
replaces. "Reset to default" in the settings window goes back to the layout's
value, and a setting the layout supplies is marked "from layout".

**Only presentation settings are allowed:** `[taskbar]`, `windows.frame`, the snap
picker, blur, window transparency and menu icons. Anything else (focus, input,
placement, session, ...) in a layout is reported against the layout's own file
(`layouts/slim.toml`), left out, and the rest of the layout is used. A layout
that names a missing file, or holds a bad value, is reported the same way and the
configuration keeps running (the usual red banner, and `vaht config files`).

`vaht config get` marks the settings a layout supplies with `+` (and the ones
changed in place with `*`); `vaht config files` lists the layouts there are.

### 10.4 Themes: `themes/<name>.toml`

A theme is a style and a layout under one name, so a whole look is chosen at
once. It says nothing else:

```toml
# themes/calm.toml
style = "nord"
layout = "slim"
```

Either part may be left out. `settings.toml` picks one with `theme = "calm"`,
found like styles and layouts (`themes/`, then `/usr/share/vaht/themes`). A
`style` or `layout` `settings.toml` sets itself wins over the theme's, so a
theme can be taken with another style or layout. A theme that is missing or says
more than a style and a layout is reported against its own file, and the rest of
the configuration keeps running.

`set-theme <name>` (and the `themes` menu list) chooses a theme and puts back
the `style` and `layout` settings, so the theme's apply and nothing overrides
them; `save-settings` writes that.

## 11. `vaht import`

```
vaht import fluxbox  [--from ~/.fluxbox] [--to ~/.config/vaht] [--force]
vaht import fluxbox-style <path>... [--to ~/.config/vaht/styles] [--force] [--notes]
```

- **There is one importer, for the Fluxbox format (decided).** fluxland was
  never released, so it gets no import path of its own. My fluxland setup is
  converted with `vaht import fluxbox --from ~/.config/fluxland`:
  - its files are in the Fluxbox format;
  - the importer follows the file paths in `init` (`session.keyFile`,
    `session.appsFile`, `session.menuFile`, `session.styleFile`, and so on),
    so the fluxland directory layout needs nothing special;
  - the few keys and actions my fork added to that format (`SnapStep`,
    `snapLayoutPicker`, `systray.*`, `session.output.*`, `session.xwayland.*`,
    `PseudoTransparent`, `FocusProtection`, `RunDialog`) are understood as
    extensions of it.

- **`fluxbox-style` converts styles alone.** It reads and writes nothing but
  styles: no `init`, keys, menus or `settings.toml`, so it cannot disturb the
  rest of a configuration. Each path is a style file, a style directory (one
  holding `theme.cfg`), or a directory of styles, which is searched through,
  nested directories included: a file in it counts when it reads as a Fluxbox
  style (a line setting a `window.`, `toolbar.` or `menu.` resource), so a
  `README` is left alone. A style `Name` (or `Name.style`, or a directory
  `Name/`) becomes `<to>/Name.toml`, `--to` being a directory, by default the
  configuration directory's `styles`. Nothing is replaced without `--force`
  (so a second run over the same styles leaves the ones already there alone),
  a style that fails does not stop the others, and each is read as vaht reads
  a style before it is written. One style prints every line not converted;
  several print how many, and `--notes` lists them. Two styles of one run with
  the same name are not both written. Choosing the result is `vaht config set
  style Name`.
- **Built so far:** `init` to `settings.toml`, `keys` to `bindings.toml`,
  `apps` to `rules.toml`, and the style `session.styleFile` names (a file,
  or a directory with `theme.cfg`) to `styles/<name>.toml`. What `init` sets
  that is presentation (the toolbar, the window buttons, ...) goes to
  `layouts/<name>.toml`, a theme `themes/<name>.toml` ties the two, and the
  converted `settings.toml` holds only behavior and `theme = "<name>"`.
  Every setting, binding or pattern vaht lacks is reported as "not in vaht
  yet", so importing again as features arrive picks them up. The first
  wallpaper `startup` sets with `swaybg`, `feh --bg-*` or `fbsetbg` becomes
  `[wallpaper]` (`$HOME` written `~`). The rest of `startup` is
  reported, to be added to `session.autostart` by hand. Commands are translated by
  `FluxboxCommand`. A sequence loses the parts vaht drops on purpose
  (`ActivateTab`), but a part vaht lacks for now leaves the whole binding
  out.
- **The menus convert to `menus.toml`:** `session.menuFile` becomes
  `[menu.root]` and `session.screen0.windowMenu` becomes `[menu.window]`; each
  `[submenu]` becomes its own `[menu.<name>]`, named from its label and kept
  unique, and `[include]` files are read in place. A submenu with nothing
  vaht can show is left out. Vaht's own window and workspace menus are
  written where the source has none.
- **Restart converts to `reload`:** fluxland's `Restart` and `[restart]`
  reload the configuration; one naming another window manager to start is
  reported.
- **The styles menu converts:** `[stylesdir]` and `[stylesmenu]` become
  the `styles` list, which shows vaht's styles (the directory is reported).
  Choosing one runs `set-style <name>`, which uses the style at once
  without writing anything. The change outlasts reloads, until
  `settings.toml` is edited to name a style itself; `save-settings` writes
  it there (`style = "<name>"`, the rest of the file kept as it was).
- **The title dialog converts:** `SetTitleDialog` and the window menu's
  `[settitledialog]` become `set-title-dialog`, which shows the window's
  title to change (`vaht dialog title --window <id>`); `SetTitle` becomes
  `set-title`, and `set-title` alone gives the program's title back.
- **The command dialog converts:** `CommandDialog` and the menu's
  `[commanddialog]` become `command-dialog`, a line for a vaht command.
  A command vaht refuses keeps the dialog open, with the reason in its
  title.
- **The run window converts to the run dialog:** `RunDialog`, and `Exec
  fbrun` or `Exec fluxland-run`, become `run-dialog`; their options are
  reported, since the style's `dialog.field` styles the dialog. Fluxbox
  styles say nothing of `dialog.field`, so a converted style takes the
  built-in one's: black on white, as fbrun draws.
- **The toolbar converts to `[taskbar]`:** its placement, tools, `maxOver`,
  `strftimeFormat`, `iconbar.mode` (a client pattern) and
  `iconbar.iconWidth`; settings that already ask for what vaht does are
  accepted silently.
- **How a style converts.** Each vaht element in each state gets what
  Fluxbox would give the resources behind it, looked up as Xlib's resource
  manager does (wildcards, tight before loose bindings) and through
  fluxland's fallback chains (`window.label.focus.font`, then
  `window.font`). The style is then written as the fewest layers that give
  every element exactly that, which a test checks against vaht's own
  resolver. Padding, borders and bar heights are written per element only.
  Colors keep their X11 names. Pixmaps, interlacing and the gradients vaht
  lacks are reported; `ParentRelative` takes the parent element's texture.
- It never overwrites an existing file without `--force`, and never touches
  the source.
- It writes only keys that differ from vaht's defaults, with a header comment
  naming the source.
- It prints a report of every source line it dropped, and why: tabs, slit,
  ignored settings, unsupported pattern features.
- It converts fluxbox styles (resource cascade, X11 color names, XPM and other
  pixmaps to PNG beside the style).
- `vaht config check` runs on the result before it reports success.

---

## 12. IPC

vaht listens on a Unix socket beside its Wayland one:
`$XDG_RUNTIME_DIR/<WAYLAND_DISPLAY>.vaht.sock`. Every program it starts
finds the path in `VAHT_SOCKET`. The protocol is one JSON object per line
in each direction. A client writes a request and reads one reply for it,
in order. The types live in the `vaht-ipc` crate, which `vaht`'s commands that
ask the running vaht and the dialogs use too.

| Request | Fields | Reply |
|---|---|---|
| `{"request":"version"}` | | `{"reply":"version","version":"0.1.0"}` |
| `{"request":"run","command":"workspace 2"}` | `command`: as a binding writes it (§4, one action); `window`: an id, the focused window by default | `{"reply":"done"}` |
| `{"request":"windows"}` | | `{"reply":"windows","windows":[{"id":1,"title":"…","app_id":"kitty","workspace":1,"is_focused":true,"is_minimized":false,"is_shaded":false}]}`, topmost first; `workspace` is `null` for a window on every workspace |
| `{"request":"workspaces"}` | | `{"reply":"workspaces","workspaces":[{"number":1,"name":"…","is_current":true}]}` |
| `{"request":"subscribe","events":["windows","workspaces"]}` | `events`: any of `windows`, `workspaces`, `outputs` | `{"reply":"done"}`, then each list at once and whenever it changes, as `{"event":"windows","windows":[…]}` (and `workspaces`, `outputs` with `profile`), the same fields as the reply to asking for it; a client is sent a list only when it differs from the one it was last sent |
| `{"request":"outputs"}` | | `{"reply":"outputs","outputs":[{"connector":"HDMI-A-1","make":"…","model":"…","serial":"…","is_enabled":true,"is_primary":true,"mode":{"width":1920,"height":1080,"refresh_millihertz":60000},"position":[0,0],"scale":1.0}]}`, on and off, in the order found; `mode` is `null` for an output off |

- **Errors are replies:** `{"reply":"error","message":"…"}` for a request
  that is not understood, names an unknown field, or cannot be carried out.
- **Nothing is held for a client:** a line longer than 64 KiB, or replies
  left unread past 1 MiB, drop the connection.
- **The commands that ask the running vaht** are on the command line, with no
  prefix: `vaht run workspace 2`,
  `vaht windows`, `vaht workspaces`, `vaht outputs` (each
  output with the `match` lines to paste into `outputs.toml`),
  `vaht version`, `vaht subscribe windows workspaces outputs`
  (each list as it changes, until vaht stops); `--json` prints the reply,
  or each event, as sent. `vaht config check` and `vaht config schema` need
  no running vaht.
- **`vaht config`** reads the configuration as vaht has it, for the
  settings editor and for scripts: `files` (what can be read, whether each
  has changes made in place and not written, and the styles there are),
  `schema <file>` (JSON Schema, draft 2020-12, of the file: every key with
  its type, its doc comment as `description`, its default, enum values and
  ranges; the ranges are the constants vaht's own validation uses), and
  `get <file>` (what the file says now, its defaults filled in, a
  `key = value` line each, `*` marking the keys changed in place; `--json`
  gives the value, the defaults and the changed keys as data). Only
  `settings.toml` is described so far; the other files follow as their
  editors do. Reads are open to every client, as `windows` is.
  Edits are made in place and run at once: `set <key> <value>` (the value
  is JSON, or a bare word taken as a string), `unset <key>` (the setting is
  left out of the file when saved, so it is the default again), `save` (writes
  every change into its file, keeping the file's comments and layout) and
  `revert [file]` (drops the changes, so the file runs as it says). The
  `config-edit` request takes a list of edits and makes all of them or none:
  vaht checks the whole configuration with them made, and a refusal says
  which setting and why. A key is written as `set-setting` takes it:
  `focus.model`, or `input.device."My Mouse".accel_speed` where a part is
  not a plain word (one grammar, `vaht_ipc::ConfigPath`). Changing is a
  command, so `[ipc] allow_commands = false` refuses it except from vaht's
  own dialogs and the settings window. `vaht subscribe config` sends each file's state whenever
  vaht runs a new configuration (an edit, a save, a revert, a file changed
  on disk), with a counter so a refresh is never mistaken for the last one.
- **`vaht-settings`** is the settings window (`vaht run settings`, the
  `settings` command, the root menu's *Settings*, or `vaht-settings` from a
  launcher). It is a separate program (egui, Wayland only) and a client of
  `vaht config`: it draws the form from the schema vaht describes,
  sends each change as it is made, and so every change runs at once. *Save*
  writes the changes into `settings.toml` and *Revert* drops them; both are
  enabled only while changes are pending. A value vaht refuses is shown
  with the reason and changes nothing. Settings that are optional (`style`,
  `log.keep_days`...) have a *Set* box; *Reset* puts one back to its
  default. Every control is labelled with the setting's name, so a screen
  reader announces it. Only `settings.toml` has a page so far. No
  setting is typed as JSON: a value that may be written several ways (`rows`,
  a number or `auto`; `entry_width`; a matcher's text, yes/no/either and
  workspace) has a list to choose the way and the control for it; tables by
  name (`[input.device]`) have a section each, with a name to add one by and
  a *Remove*; plain tables (`[wallpaper.workspaces]`) have a row each. The
  window shows settings in the order the schema declares them, which is the
  order of the fields in vaht's code (so a table lists what belongs
  together together, not alphabetically), and a `x-vaht-group` on a setting
  puts a heading above it and its neighbours (the taskbar's page is
  *Taskbar*, *Position*, *Hiding*, *Contents*). A
  matcher's `not` shows one level deep (a `not` inside it is for the file). A
  test fails if a setting of `settings.toml` has no control of its own. The
  schema names what the window shows: `title` is a setting's or a way's name,
  and `description` what it does or means (shown beside the chosen way).
- **`vaht pick`** (`pick` request) is a dmenu-style chooser: the lines
  on standard input show as a vaht menu at the pointer (`--title` above
  them), in the user's style, and the one chosen is printed; closing the
  menu prints nothing and exits 1. The reply waits for the user; a
  connection's later requests are answered after it, in order. One pick
  shows at a time (a new one answers the one before with nothing), and
  none while the session is locked. The screen-share portal uses it in
  vaht sessions (`data/xdg-desktop-portal-wlr-vaht`, installed as
  `/etc/xdg/xdg-desktop-portal-wlr/vaht`), so sharing asks with vaht's
  menu rather than whichever launcher happens to be installed.
- **`[ipc] allow_commands`** (on by default; from Fluxbox's
  `allowRemoteActions`): off, only vaht's own dialogs may run commands,
  known by the process at the other end of the socket. Asking for windows
  and workspaces stays open to every client. The socket is the user's own
  (in `XDG_RUNTIME_DIR`, mode 0700), so this guards against the user's
  other programs, which can already type through the virtual keyboard
  protocol.
- **`vaht ipc-schema`** prints the protocol's JSON Schema (draft 2020-12),
  made from the message types themselves (`vaht_ipc::schema`), so it
  cannot drift: every message is a request, a reply or an event, each in
  `$defs`.

---

## 13. Resolved questions

1. **Reload on change:** yes, with inotify, debounced, all or nothing (§2).
2. **Colors:** hex or X11 names (§10.1).
3. **Commands:** both string and table forms (§4).
4. **Profile keys:** vaht's own profile actions. Audio stays on
   av-output-switcher for now. Tighter audio integration is a later topic, and
   the profile hook (`outputs.toml [hooks]`) is the seam for it.
