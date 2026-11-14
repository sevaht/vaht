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
   `vaht check --print-defaults` prints the complete effective config, so the
   defaults can be read without opening the source (K3: they are written down
   only once).
4. **No paths to other config files.** File names are fixed (K3). Fluxbox's
   `session.keyFile`, `session.menuFile` and the rest are gone.
5. **A bad reload keeps the old config.** vaht logs the error and posts a
   desktop notification over `org.freedesktop.Notifications`, the same channel
   other programs use. `vaht check` validates without running.
6. **vaht writes config only on SaveSettings**, through `toml_edit`, which
   keeps comments and layout (00 decision).
7. **Spelling.** Keys are `snake_case`, as is usual in TOML. Values that name
   things (actions, edges, modes) are `kebab-case` strings.

## 2. Files

Everything lives under `$XDG_CONFIG_HOME/vaht/` (normally `~/.config/vaht/`):

| File | Contains | Replaces (fluxland) |
|---|---|---|
| `settings.toml` | Behavior: focus, placement, workspaces, taskbar, menus, windows, Xwayland, session, theme choice, wallpaper | `init` |
| `bindings.toml` | Key and mouse bindings, key modes | `keys` |
| `rules.toml` | Per-window rules | `apps` |
| `menus.toml` | Root menu, window menu, custom menus | `menu`, `windowmenu` |
| `outputs.toml` | Per-output settings, display profiles, profile hooks | `session.output.*` in `init`; av-output-switcher's `profiles.json` (video part), converted by hand |
| `themes/<name>.toml` | Themes; also read from `/usr/share/vaht/themes/` | `styles/*`, `overlay` |

- **`overlay` disappears.** My overlay existed only to stop styles from
  setting the background. In vaht themes cannot set the wallpaper at all; it
  belongs to `settings.toml`.
- **`startup` becomes `session.autostart`.** It is a list of commands, and my
  existing script can simply be one entry.
- **Reloading** happens automatically when a file changes (decided), and also
  on the `reload` action and on SIGHUP. Rules apply to windows opened after the
  change.
  - **No polling.** One inotify watch on the config directory and one on
    `themes/` are registered as a calloop source. vaht watches directories,
    not files, because editors save by writing a temporary file and renaming
    it, which a watch on the file would lose.
  - **Debounced.** An editor's burst of events (write, rename, chmod) settles
    into one reload after 200 ms of quiet.
  - **All or nothing.** Every file is parsed and validated into a complete new
    `Config` first. Only if all of it is valid does vaht apply it. Otherwise the
    running config stays exactly as it was, and the user gets a desktop notification
    naming the file, line and problem (plus a log entry). Fixing the file
    triggers the next attempt by itself.

---

## 3. `settings.toml`

### 3.1 My `init`, converted

Only keys that differ from the default would be written. They are all shown
here so the mapping can be checked.

```toml
theme = "nacitar"
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
height = "theme"                # or pixels (height 0 = from the theme)
rows = 1                        # new: several rows (00 §5)
auto_hide = false
reserve_space = true            # maxOver = false
layer = "dock"
widgets = ["workspace-previous", "workspace-name", "workspace-next", "window-list", "tray", "clock"]

[taskbar.window_list]
show = { workspace = "current" }    # a window matcher (§5); iconbar.mode (workspace)
scope = "all-outputs"               # new: all-outputs | this-output
order = "stable"                    # {static}: opening order, not focus order
entry_width = { max = 128 }         # iconWidth with alignment Relative
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
# per_workspace = ["a.png", "b.png", ...]    # optional

[xwayland]
resources = "~/.Xresources"
input_emulation = { default = "ask", allow = ["steam"], deny = [] }

[ipc]
allow_commands = true           # allowRemoteActions

[session]
autostart = ["~/.config/vaht/autostart"]
```

### 3.2 What is built

`settings.toml` accepts the sections whose features exist, and refuses the
rest by name, so a setting never silently does nothing:

- `[focus]`: `model` (`click` or `sloppy`; `strict-sloppy` is not built),
  `raise_on_click`, `auto_raise`, `auto_raise_delay_ms`, `focus_new_windows`.
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
  (false shows an outline of the new frame, in the theme's `wireframe`,
  while resizing, and the
  window takes the size when the drag ends; from Fluxbox's
  `opaqueResize`), and `[windows.frame]`
  with `titlebar_left` and `titlebar_right` (buttons `shade`, `stick`,
  `minimize`, `maximize`, `close`) and `handle` (the strip along the
  bottom with a grip at each end, 20 pixels wide as Fluxbox's; dragging
  it resizes the bottom edge, and a grip its corner, unless the `handle`
  and `grip-*` mouse contexts bind something). `vaht import` reads it from
  `defaultDeco`'s preset or mask.
- `[workspaces]`: `names`, at least one.
- `[session]`: `autostart`.
- `[wallpaper]`: `image` (PNG, JPEG, GIF, WebP, BMP, TIFF, TGA, PNM, QOI
  or ICO, known by its contents; `~/` is the home directory, a relative
  path is from the configuration directory) and `fit` (`fill`, `fit`,
  `center`, `tile`, `stretch`). It is decoded once, painted once for each
  output, at its size, and drawn beneath everything; where it does not reach, and with no
  image, the desktop is dark grey. `per_workspace` is not built.
- `[xwayland]` `enabled` (on by default; read at start): vaht starts
  Xwayland and is its window manager, and programs it starts find it in
  `DISPLAY`. X11 windows are windows like any other (rules match their
  class as `app_id`, `close` sends `WM_DELETE_WINDOW`, `kill` ends the
  process their `_NET_WM_PID` names); their menus and tooltips
  (override-redirect windows) show over every window. The clipboard and
  primary selection pass both ways. `vaht --no-xwayland` overrides the
  setting (tests use it). `resources` names an X resources file
  (`~/.Xresources`; a relative path is from the configuration directory)
  loaded into Xwayland when it starts, as xrdb does, from fluxland's
  `session.xwayland.resources`. Not yet: input emulation for X11 programs
  (`session.xwayland.inputEmulation`, an EIS server for Steam Input). X Shape clipping is exact
  (04 §8.1), the pointer an X11 window sees stays in step while it moves
  (04 §8.2), and a window whose Motif hints ask for no decorations gets no
  frame.
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
- `theme`: the name of a theme in `themes/` (§10); without it, the built-in
  theme.
- `[taskbar]`: `enabled`, `output` (the name of the output it shows on,
  such as `HDMI-A-1`; the primary output without one, or while that one
  is not there), `edge` (`top` or `bottom`), `width_percent`
  (1 to 100) and `align` (`left`, `center`, `right`; from `placement`'s
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
  title, and on a tray item its tooltip, in the theme's `tooltip`),
  `widgets` (`workspace-previous`, `workspace-name`, `workspace-next`,
  `window-list`, `tray`, `clock`),
  `[taskbar.window_list]` with `show` (a matcher),
  `entry_width = { max }` and `icons` (each entry's icon: the one the
  window names or draws over xdg-toplevel-icon (a drawn one is read back
  through the renderer, with no `unsafe`), else its desktop entry's `Icon`,
  found in the `hicolor` theme or `pixmaps`; PNG and SVG; from
  `iconbar.usePixmap`), `[taskbar.tray]` with `pin_left` and `pin_right`
  (item ids, from `systray.pinLeft` and `systray.pinRight`), and
  `[taskbar.clock]` with `format`, checked when read. The tray hosts
  StatusNotifierItems: it serves the watcher on the session bus, or hosts
  for another program's; it learns of items and their changes only from
  the bus's signals; a left, middle or right click asks the item to
  activate, act otherwise or show its menu, and the wheel scrolls it. Not
  yet: dbusmenu menus (with vaht's menus), tooltips, and
  `activation_grace_ms`. The taskbar spans the output; clicking an arrow shows the
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
| `*.alpha: 255` (window, menu, toolbar, slit) | Fully opaque is the default. Other values are kept and moved to the theme, since opacity is appearance |
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
| Programs and session | `exec <command line>`, `reload`, `exit`, `restart`, `set-environment NAME value`, `set-setting key value`, `save-settings` |
| Window state | `close`, `kill`, `minimize`, `maximize [horizontal\|vertical]`, `fullscreen`, `shade`, `stick`, `raise`, `lower`, `set-layer <layer>`, `set-alpha <n>`, `toggle-frame`, `set-title [title]`, `set-title-dialog`, `mark-window <key>`, `go-to-marked-window <key>` |
| Window placement | `move <dx> <dy>`, `move-to <x> <y>`, `resize <dw> <dh>`, `resize-to <w> <h>`, `snap-step <direction>`, `tile <cell>`, `send-to-output next\|previous\|<n>`, `arrange <style>` |
| Grabs | `start-moving`, `start-resizing [nearest-corner\|<corner>]` |
| Focus | `focus`, `next-window`, `previous-window`, `focus-direction <direction>`, `show-desktop` |
| Workspaces | `workspace <n\|next\|previous\|last>`, `send-to-workspace <target>`, `take-to-workspace <target>`, `add-workspace`, `remove-last-workspace`, `rename-workspace` |
| Menus and dialogs | `root-menu`, `window-menu`, `workspace-menu`, `window-list-menu`, `custom-menu <name>`, `hide-menus`, `run-dialog`, `command-dialog` |
| Taskbar | `activate-window-list-entry <n>`, `activate-tray-item <n>`, `tray-item-menu <n>`, `toggle-taskbar`, `taskbar-to-output <target>` |
| Outputs | `apply-profile <name>`, `cycle-profile`, `cycle-primary-output` |
| Theme | `set-theme <name>`, `reload-theme` |

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
- Built so far: `app_id`, `title`, `workspace`, `minimized`, `sticky` and
  `not`. The X11 keys, `layer`, `output` and `hidden_from_window_list` come
  with the features they describe.

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

# Key modes: [keys.mode.<name>] holds bindings active after "key-mode <name>".

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
```

**How bindings behave (as built).**
- `bindings.toml` replaces the built-in bindings as a whole, as Fluxbox's
  `keys` file does. `vaht check --print-defaults` prints the built-in file to
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
- Built into the frame, not bindings: titlebar buttons (pressed without
  modifiers), and resizing by dragging a border when the border has no
  binding for that button.
- Actions of §4 that vaht does not have yet are rejected by name ("not
  available in vaht yet"), as are key chains, key modes and the structured
  forms other than `repeat` and `action`. A file that uses them fails
  `vaht check` instead of half-working.
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
  it). They apply
  when the window first
  gets its place. The other keys are refused until their features exist.
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
    { label = "Themes",     generate = "themes" },
    { label = "Workspaces", generate = "workspaces" },
    { label = "Command",    command = "command-dialog" },
    { label = "Reload",     command = "reload" },
    { label = "Restart",    command = "restart" },
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
  - `generate`: `themes`, `workspaces`, `window-list`, `send-to-workspace`,
    `layer`, `alpha`, `outputs`, `profiles`;
  - `pipe` (a command whose output is a menu in this same TOML form);
  - `separator`;
  - `icon` (optional on any item).
- **Built so far:** `command`, `submenu`, `toggle` (`shade`, `stick`,
  `maximize`, `minimize`, `fullscreen`), `generate` (`workspaces`,
  `send-to-workspace`, `window-list`, `alpha`, `layer`, `themes`; `inline =
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
  (`outputs`, choosing one makes it primary). Not yet: `pipe`, `icon`,
  and the GTK presenter.
- **Dropped:**
  - Fluxbox's `[config]` editor menu: settings are reached through
    `set-setting`.
  - `[extramenus]`: nothing in vaht adds menus to it.
  - The fluxbox-generate_menu header and the `[encoding]` tag: TOML is UTF-8.

---

## 9. `outputs.toml`

```toml
# vaht prints a ready-to-paste `match` for each output: vaht msg outputs
[[output]]
match = { connector = "HDMI-A-1" }    # or { make = "...", model = "...", serial = "..." }
mode = "1920x1080@60"                 # or "1920x1080": the fastest refresh
position = [0, 0]
scale = 1.0                           # 0.25 to 8
transform = "normal"                  # 90, 180, 270, flipped, flipped-90, ...
primary = true
vrr = "fullscreen"                    # off | on | fullscreen (default)

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
  and both at start. `vaht msg outputs --json` gives the whole layout.
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

## 10. Themes: `themes/<name>.toml`

### 10.1 Model

A theme gives a `Style` to each **element** in each **state** (01 §6). Styles
are resolved in four layers, each overriding the one before:

1. `[default]`: applies to everything (`*.font`, `*.color` in Fluxbox).
2. `[state.<state>]`: one state, any element (`*.unfocused.textColor`).
3. `[<element>]`: one element, any state.
4. `[<element>.<state>]`: one element, one state.

These four fixed layers replace Fluxbox's wildcard resource matching.
`vaht check --theme <name>` prints the fully resolved table, so it shows
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
    or `{ image = "path", fit = "tile"|"stretch" }`, or `"parent"`;
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
  - glyphs: `glyph_color` for button pictures.
- **Colors are `#rrggbb`, `#rrggbbaa`, or an X11 color name** such as `grey55`
  or `SteelBlue` (decided).
  - The X11 list (`rgb.txt`) is large, about 750 names, but it has been frozen
    since the 1990s, so it needs no maintenance.
  - The importer needs it anyway to read fluxbox styles, so accepting names in
    themes reuses the same table. That table is a static, sorted array in
    `vaht-ui::color`, generated once from X.org's `rgb.txt` and checked against
    fluxland's `x11_colors.h` in a test.
  - The published crates for this are tiny and little used (the most popular
    has fewer than 200 downloads), so vaht doesn't add a dependency for it (X2).
  - `vaht import` keeps names as written, so my converted theme still says
    `grey55`.

**Built so far.**
- `settings.toml`'s `theme = "<name>"` reads `themes/<name>.toml` from the
  configuration directory, then from `/usr/share/vaht/themes/`. The file is
  watched like the others: editing it restyles every open window.
- Elements: the window's six, and `taskbar`, `taskbar.button`,
  `taskbar.workspace`, `taskbar.clock`, `taskbar.window-list.entry`,
  `taskbar.window-list.empty`, `menu.title`, `menu.frame`, `menu.item`,
  `dialog.field`, `wireframe` (a resize's outline, drawn in its
  `border`) and `tooltip` (converted from fluxland's `systray.tooltip.*`,
  rounded as `corner_radius` says, as frames and menus are; its `alpha` is
  not built). States: `focused`, `unfocused`, `pressed`, `hovered`,
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
- The built-in theme is itself a theme file (`vaht-ui`'s
  `theme/built_in.toml`), printed by `vaht check --print-defaults`.
- A theme file is a whole theme: what it leaves out takes vaht's fallback,
  not the built-in theme's value.

### 10.2 `nacitar_fbstyle`, converted

Shown in hex, with the original names in comments, so the values can be
checked. The importer itself would keep the names (`from = "grey55"`).

```toml
# themes/nacitar.toml (from nacitar_fbstyle by nacitar sevaht)
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

## 11. `vaht import`

```
vaht import fluxbox  [--from ~/.fluxbox] [--to ~/.config/vaht] [--force]
vaht import fluxbox-style <style file or directory> --to ~/.config/vaht/themes/<name>.toml
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

- **Built so far:** `init` to `settings.toml`, `keys` to `bindings.toml`,
  `apps` to `rules.toml`, and the style `session.styleFile` names (a file,
  or a directory with `theme.cfg`) to `themes/<name>.toml`, which the
  converted `settings.toml` chooses.
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
  the `themes` list, which shows vaht's themes (the directory is reported).
  Choosing one runs `set-theme <name>`, which uses the theme at once
  without writing anything. The change outlasts reloads, until
  `settings.toml` is edited to name a theme itself; `save-settings` writes
  it there (`theme = "<name>"`, the rest of the file kept as it was).
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
  reported, since the theme's `dialog.field` styles the dialog. Fluxbox
  styles say nothing of `dialog.field`, so a converted theme takes the
  built-in one's: black on white, as fbrun draws.
- **The toolbar converts to `[taskbar]`:** its placement, tools, `maxOver`,
  `strftimeFormat`, `iconbar.mode` (a client pattern) and
  `iconbar.iconWidth`; settings that already ask for what vaht does are
  accepted silently.
- **How a style converts.** Each vaht element in each state gets what
  Fluxbox would give the resources behind it, looked up as Xlib's resource
  manager does (wildcards, tight before loose bindings) and through
  fluxland's fallback chains (`window.label.focus.font`, then
  `window.font`). The theme is then written as the fewest layers that give
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
  pixmaps to PNG beside the theme).
- `vaht check` runs on the result before it reports success.

---

## 12. IPC

vaht listens on a Unix socket beside its Wayland one:
`$XDG_RUNTIME_DIR/<WAYLAND_DISPLAY>.vaht.sock`. Every program it starts
finds the path in `VAHT_SOCKET`. The protocol is one JSON object per line
in each direction. A client writes a request and reads one reply for it,
in order. The types live in the `vaht-ipc` crate, which `vaht msg` and the
dialogs use too.

| Request | Fields | Reply |
|---|---|---|
| `{"request":"version"}` | | `{"reply":"version","version":"0.1.0"}` |
| `{"request":"run","command":"workspace 2"}` | `command`: as a binding writes it (§4, one action); `window`: an id, the focused window by default | `{"reply":"done"}` |
| `{"request":"windows"}` | | `{"reply":"windows","windows":[{"id":1,"title":"…","app_id":"kitty","workspace":1,"is_focused":true,"is_minimized":false,"is_shaded":false}]}`, topmost first; `workspace` is `null` for a window on every workspace |
| `{"request":"workspaces"}` | | `{"reply":"workspaces","workspaces":[{"number":1,"name":"…","is_current":true}]}` |
| `{"request":"outputs"}` | | `{"reply":"outputs","outputs":[{"connector":"HDMI-A-1","make":"…","model":"…","serial":"…","is_enabled":true,"is_primary":true,"mode":{"width":1920,"height":1080,"refresh_millihertz":60000},"position":[0,0],"scale":1.0}]}`, on and off, in the order found; `mode` is `null` for an output off |

- **Errors are replies:** `{"reply":"error","message":"…"}` for a request
  that is not understood, names an unknown field, or cannot be carried out.
- **Nothing is held for a client:** a line longer than 64 KiB, or replies
  left unread past 1 MiB, drop the connection.
- **`vaht msg`** asks from the command line: `vaht msg run workspace 2`,
  `vaht msg windows`, `vaht msg workspaces`, `vaht msg outputs` (each
  output with the `match` lines to paste into `outputs.toml`),
  `vaht msg version`; `--json` prints the reply as sent.
- **`[ipc] allow_commands`** (on by default; from Fluxbox's
  `allowRemoteActions`): off, only vaht's own dialogs may run commands,
  known by the process at the other end of the socket. Asking for windows
  and workspaces stays open to every client. The socket is the user's own
  (in `XDG_RUNTIME_DIR`, mode 0700), so this guards against the user's
  other programs, which can already type through the virtual keyboard
  protocol.
- **Not built yet:** event subscriptions and the JSON Schema export.

---

## 13. Resolved questions

1. **Reload on change:** yes, with inotify, debounced, all or nothing (§2).
2. **Colors:** hex or X11 names (§10.1).
3. **Commands:** both string and table forms (§4).
4. **Profile keys:** vaht's own profile actions. Audio stays on
   av-output-switcher for now. Tighter audio integration is a later topic, and
   the profile hook (`outputs.toml [hooks]`) is the seam for it.
