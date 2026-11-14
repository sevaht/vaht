# 04 — wlroots → Smithay mapping

How what fluxland gets from wlroots maps onto Smithay, and what that means for
the design. Smithay facts are from its `master` branch tree as of 2026-11-12.
Items marked **(verify)** must be confirmed in a spike before the design
relies on them.

## 1. The shift that matters: Smithay is a toolkit, not a framework

| Concern | wlroots (fluxland) | Smithay (vaht) |
|---|---|---|
| Event loop | `wl_event_loop`, C callbacks via `wl_listener` | `calloop`, sources owned by a `RegistrationToken`; handlers are closures or trait methods on *vaht's* state |
| Compositor state | Library structs, with fluxland's state hung off them with listeners | **vaht owns one state struct**; Smithay calls `*Handler` traits implemented on it |
| Scene graph | `wlr_scene` does layout, damage, direct scanout | **None.** `desktop::Space` is optional and minimal. vaht produces a list of `RenderElement`s per output per frame; `OutputDamageTracker` / `DrmCompositor` diffs them |
| vaht's own drawing | Cairo surface → `wlr_scene_buffer` | Pixels → `MemoryRenderBuffer` / `TextureBuffer` element, or `SolidColorRenderElement` for flat fills |
| Lifetimes | Manual `wl_list_remove` on destroy signals | Wayland objects carry `UserData`; dropping vaht's wrapper drops state; client disconnect cleans resources |
| X11 | wlroots xwm (C, xcb) | Smithay's `X11Wm` in Rust (x11rb); no xcb C dependencies |

**Design consequence.** Since there is no scene graph, *vaht* decides what is
drawn each frame. That fits the unified panel model very well. Every
compositor-drawn thing (a frame, the taskbar, a menu, a tooltip, an overlay)
implements one trait that yields render elements and handles hits. The
compositor collects `[client surfaces + panels]` per output in stacking order.
Damage tracking comes from element ids and commit counters, so a panel
re-renders its buffer only when its content changes.

## 2. Protocol mapping

### Provided by Smithay

| Protocol / facility | fluxland file | Smithay module |
|---|---|---|
| compositor, subcompositor, shm, dmabuf, viewporter, single-pixel-buffer, alpha-modifier, fixes | `protocols.c` | `wayland::{compositor, shm, dmabuf, viewporter, single_pixel_buffer, alpha_modifier, fixes}` |
| xdg-shell, xdg-decoration, KDE server-decoration, xdg-dialog | `view_xdg.c`, `decoration.c` | `wayland::shell::{xdg, xdg::decoration, xdg::dialog, kde}` |
| layer-shell | `layer_shell.c` | `wayland::shell::wlr_layer` |
| xdg-output | | `wayland::output::xdg` |
| seat, data-device, primary selection, wlr and ext data-control | | `wayland::{seat, selection::*}` |
| cursor-shape, pointer-constraints, relative-pointer, pointer-gestures, pointer-warp | | `wayland::*` |
| tablet, text-input v3, input-method v2, virtual keyboard and pointer, keyboard-shortcuts-inhibit | `tablet.c`, `text_input.c` | `wayland::*` |
| xdg-activation, xdg-foreign, system-bell, toplevel-icon, toplevel-tag | `protocols.c` | `wayland::*` |
| session-lock, idle-notify, idle-inhibit, security-context | `session_lock.c`, `idle.c` | `wayland::*` |
| presentation-time, content-type, fractional-scale | `presentation.c` | `wayland::*` |
| **fifo-v1, commit-timing-v1** | `frame_pacing.c` (736 lines, hand-written) | `wayland::{fifo, commit_timing}` |
| linux-drm-syncobj (explicit sync), drm-lease | `drm_syncobj.c`, `drm_lease.c` | `wayland::{drm_syncobj, drm_lease}` |
| ext-image-copy-capture, image-capture-source | `screencopy.c` | `wayland::{image_copy_capture, image_capture_source}` |
| ext-foreign-toplevel-list | `foreign_toplevel.c` | `wayland::foreign_toplevel_list` |
| **libei / EIS** (emulated input from X11 programs, Steam Input) | `eis.c` (959 lines, hand-written) | `backend::libei` |
| **Xwayland keyboard grab**, xwayland-shell | `xwayland_grab.c` | `wayland::{xwayland_keyboard_grab, xwayland_shell}` |
| background-effect (a client asks for blur behind it) | none | `wayland::background_effect`: new, optional |

### Not in Smithay; vaht implements them (niri and cosmic-comp are references)

| Protocol | Why vaht needs it | Note |
|---|---|---|
| wlr-output-management | wlr-randr, kanshi, outside tools | **built** (§8.9), through the same output arrangement as `outputs.toml` and its profiles |
| wlr-foreign-toplevel-management | Taskbars and docks outside the compositor | **built** (§8.6) |
| ext-workspace | External pagers | niri implements it |
| ~~wlr-screencopy~~ | | dropped: the installed xdg-desktop-portal-wlr 0.8.4 and grim 1.5 use ext-image-copy-capture |
| wlr-gamma-control | Night-light tools | **built** (§8.8) |
| wlr-output-power-management | DPMS via `wlopm` | **built** (§8.7) |
| **tearing-control** | Games | Needs async page flips in the DRM compositor **(verify Smithay support)** |
| color-representation, color-management | HDR | later |
| transient-seat, export-dmabuf | | drop: unused or deprecated |

Each of these sits behind a module of its own with a narrow handler (M4
gateway). They are mechanical work, not design work.

## 3. Backends and rendering

| Concern | Smithay | Plan |
|---|---|---|
| Real session | `backend::{session (libseat), udev, drm, libinput, allocator::gbm}` | `DrmCompositor` per output: plane assignment (direct scanout), damage, page flips |
| Nested development session | `backend::winit` (and `x11`) | Run vaht in a window under fluxland, so iteration never needs a VT switch |
| Renderer | `renderer::{gles, glow, pixman, multigpu}`, `vulkan` allocator only | GLES via `multigpu` (hybrid laptops); pixman as a fallback |
| VRR | `DrmCompositor::{vrr_supported, use_vrr}` (verified, §6) | Output-level policy (see 00 §12) |
| vaht's own 2D drawing | Not provided | **Decide in 01.** Candidates: `tiny-skia` (pure Rust, small) plus `cosmic-text` (shaping, fallback, halo and shadow done ourselves); or `pangocairo` bindings (parity with today's fonts and Pango markup, heavier C dependency) |
| Images and icons | Not provided | `image` (PNG, XPM via a small parser if Fluxbox pixmaps need it), `resvg` for SVG icons |
| D-Bus (tray, dbusmenu, activation environment) | Not provided | `zbus`, run on its own thread or executor and bridged to calloop through a channel |

## 4. Games and media: what Smithay makes possible

1. **Direct scanout.** `DrmCompositor` puts a fullscreen client buffer on the
   primary plane, or on overlay planes, when nothing covers it. vaht's job is to
   draw no panel or frame over a fullscreen window, and to keep cursor planes
   working.
2. **Explicit sync.** `drm_syncobj` is required for good NVIDIA and Proton
   behavior.
3. **Frame pacing.** fifo, commit-timing and presentation-time come built in,
   and Vulkan FIFO mode (Mesa's Wayland WSI) relies on them.
4. **Tearing.** It is off by default, and VRR is the low-latency path.
   Implement tearing-control so the hint is understood. Flip asynchronously only
   when a setting allows it and a fullscreen, directly scanned out surface asks
   **(verify)**.
5. **VRR policy.** Turn it on while a fullscreen surface whose content-type is
   game or video is shown.
6. **Input.** Relative pointer, pointer constraints and pointer warp; Xwayland
   keyboard grab for X11 games; libei for Steam Input.
7. **Idle inhibit** for video players, and a policy option to inhibit idle
   while a fullscreen surface is shown.
8. **DRM lease** for VR headsets.

## 5. Versioning

- The latest crates.io release is `smithay 0.7.0` (2025-06), and `master` is
  active (its last commit was on 2026-11-12). Serious compositors pin a
  `master` git revision; niri pins one, for example. **Plan:** pin a git rev in
  `Cargo.toml` and bump it deliberately, one bump per commit.
- Smithay is on edition 2024 with MSRV 1.88. The local toolchain is
  rustc 1.99.1.
- The project skeleton comes from the `sevaht-rust-project` template (R-T0)
  once the design is approved.

## 6. Spike results (2026-11-12, Smithay `ae6921b`)

1. **Text rendering.** `tiny-skia` + `cosmic-text` and `pangocairo` were given
   the same input: the `nacitar_fbstyle` focused label (DejaVu Sans Bold 8 at
   96 dpi, white text with a 2 px black halo, on a grey55 → grey5 gradient).
   Viewed at 4× zoom, the two are nearly indistinguishable.

   | | Pango + Cairo | tiny-skia + cosmic-text |
   |---|---|---|
   | Dynamically linked C libraries | 40 (GLib, HarfBuzz, FreeType, fontconfig…) | 5 |
   | Release binary | 0.5 MB | 4.4 MB (static) |
   | Startup cost | none measurable | about 55 ms once, to index system fonts |
   | Halo | `layout_path` + stroke | glyph outlines from swash + stroke (about 30 lines) |
   | Memory and leak analysis | GLib reference counting across FFI | Plain Rust ownership |

   **Recommendation:** `tiny-skia` + `cosmic-text`. It is pure Rust, the same
   stack COSMIC uses, and tiny-skia also covers gradients, bevels and rounded
   corners. That means one drawing library for every panel.
2. **VRR: supported.** `DrmCompositor::vrr_supported(connector)` reports
   `NotSupported`, `RequiresModeset` or `Supported`, and `use_vrr(bool)`
   applies from the next frame. The flag `FrameFlags::SKIP_CURSOR_ONLY_UPDATES`
   lets cursor movement skip a frame so it does not disturb the refresh rate.
3. **Tearing: not supported** by Smithay's DRM backend; there are no async page
   flips. That matches the decision. vaht does not advertise tearing-control,
   and games get vsync plus VRR.
4. **Nested panel.** Smithay's `smallvil` example, extended with a taskbar-like
   `Panel`:
   - it draws with tiny-skia into a `MemoryRenderBuffer`;
   - it redraws only when its hover or pressed state changes;
   - it composites over client windows as a custom render element;
   - it hit-tests itself and consumes its own clicks.

   It builds against master. Interactive check: see the conversation.

## 7. Spikes before 01 is final (original plan)

These are short throwaway experiments in the scratchpad, not in vaht. Each one
answers a question the object model depends on.

1. **Nested winit compositor.** Check that xdg-shell windows plus one custom
   `MemoryRenderBuffer` panel render and take hits. This confirms the panel
   rendering approach.
2. **Text rendering.** Render a titlebar label in DejaVu Sans 8 bold with a
   halo, once with `tiny-skia` + `cosmic-text` and once with `pangocairo`.
   Compare quality against fluxland and the dependency weight.
3. **DRM tearing and VRR.** Find out what the DRM compositor exposes for async
   page flips and VRR at the pinned revision.

## 8. Xwayland problems: two fluxland could not solve on wlroots, and the clipboard

Both are recorded in fluxland's README ("Known issues") and
COMPATIBILITY.md; Smithay gives what wlroots lacked, so vaht builds them
right with Xwayland.

### 8.1 X Shape clipping, exactly

XWayland does not show a window's X Shape (the bounding region Wine's
layered windows and Proton's focus overlays use, which are rings), so the
compositor must. fluxland fetched the shape's rectangles over XCB and made
one clipped `wlr_scene` subsurface tree per rectangle, up to 32; a shape of
more (a ring with rounded corners easily has hundreds) fell back to the
whole, unclipped window.

vaht clips with one render element per surface and no limit: a
`ShapedElement` wraps the window's surface elements and holds the shape as
a list of rectangles (an X shape *is* a set of rectangles, so this is exact,
not an approximation). Its `draw` gets the damage to redraw and passes on
only the damage that lies within the shape; its `opaque_regions` and
`damage_since` are cut to the shape too, so the damage tracker never
assumes a hole is covered. Hit-testing (`Scene::target_at`) asks the same
shape, so a click in a hole goes to what shows through it. The shape comes
from a connection of vaht's own to Xwayland (x11rb with the `shape`
feature): `ShapeSelectInput` per managed window, `ShapeGetRectangles` on map
and on each `ShapeNotify`, so it follows the shape as it changes.

**Built.** `xwayland/shape.rs` follows shapes over vaht's own x11rb
connection; `scene/shaped_element.rs` clips. Each shape gives the
window's surfaces new element ids, so a shape that changes redraws the
window whole. Tests shape a window to a ring (its hole shows what is
below, and a click there goes to the window below) and to a hundred
rectangles. X11 windows whose Motif hints ask for no decorations (a
frameless Electron window, such as the ROTK launcher) get no frame, as in
Fluxbox.

### 8.2 The pointer seen by an X11 window that vaht moves

Seen with the ROTK launcher (Wine/Chromium, a frameless Electron window
dragged by its own title bar): while it moves, title bar entries that slide
under where the drag began light up as if hovered. While the compositor
moves a window it sends the pointer to nobody, so XWayland's pointer stays
where the grab began; XWayland places the pointer on the X screen as (the
window's position it knows) + (the surface-local position last sent), and
the window's position comes over the X connection, unordered with the
Wayland events. An X client that asks (Wine's `GetCursorPos`) sees the two
out of step. fluxland's workaround, sending motion once a frame, changed
nothing, since it could not tell when the X server had the new position.

Smithay can tell: `XwmHandler::configure_notify` is called for every
`ConfigureNotify` of a mapped window. vaht's move of an X11 window will:

1. `configure()` it at the new position and remember the position asked for;
2. on `configure_notify()` with that position, send the pointer to the
   window's surface at its surface-local position relative to the position
   the X server now has (a motion with the pointer focus kept on the
   window during the grab, as the X server expects for a window being
   dragged);
3. on release, the same once more, so XWayland's pointer ends where the real
   one is before anything else is hovered.

**Built.** Each X11 toplevel keeps the position the X server last confirmed
(`Toplevel::confirm_position`, from `configure_notify`); the pointer is
given to an X11 surface relative to it (`Toplevel::pointer_origin`), and a
move an X11 window asked for keeps the pointer on it during the drag
(`Compositor::x11_drag_focus`), re-sent on every confirmation. The test
`an_x11_window_moving_itself_sees_the_pointer_where_it_is` asks the X
server where the pointer is, during and after such a drag; without the
fix it reports the drag's start, as fluxland's ROTK launcher showed.

Steps 2 and 3 order the pointer after the X server has the position, which
is the most a compositor can do; the rest of the race is XWayland's (one
request setting the window position and the pointer together would end it).
Test it with the ROTK launcher, and with `xdotool getmouselocation` during
and after a drag, which should follow the real pointer.

### 8.3 The clipboard between X11 and Wayland programs

Smithay's X window manager stands in for X programs in both directions,
but the compositor must connect it to the Wayland selections itself
(`SelectionHandler::new_selection` and `send_selection`, in
`protocols/selection.rs`); without that, Wayland clients could neither
paste what an X program copied nor offer X programs what they copied.
`X11Wm::new_selection` also queues the change of owner without sending it,
so an X program pasting right after a Wayland copy found no owner until
the window manager next wrote to X; vaht sends it at once
(`XwaylandServer::offer_selection`). The tests
`an_x11_program_pastes_what_a_wayland_program_copied` and
`a_wayland_program_pastes_what_an_x11_program_copied` drive both
directions, and both selections, through a clipboard manager (data
control) and an X program of the tests' own.

### 8.4 Sandboxed programs

A sandbox engine (Flatpak) asks over `wp_security_context_v1` for a socket
of its own; vaht listens there until the engine closes its end
(`protocols/sandbox.rs`, with `event_source::EndingSource` noticing the
listener end), and marks each client accepted there as sandboxed
(`ClientState::is_unrestricted`). Sandboxed programs are not offered the
globals that reach beyond their own windows: data control (every
selection), the toplevel list and toplevel management, input methods,
output management, power and gamma, session lock, screen capture, virtual
pointer and keyboard, layer shell, and security contexts themselves. The
test `a_sandboxed_program_is_offered_windows_but_not_the_privileged_globals`
drives a sandbox engine's socket and lists what each side is offered.

### 8.5 Input methods

fcitx5 or ibus binds `zwp_input_method_v2` and types for the focused
window through its `zwp_text_input_v3`; Smithay routes the text and
follows keyboard focus, and vaht shows the input method's candidate popup
over the window through its `PopupManager`
(`protocols/input_method.rs`). Sandboxed programs cannot be the input
method, which reads every key. The test
`an_input_method_types_into_the_focused_window` types Japanese through a
test input method into a test window.

### 8.6 Taskbars outside vaht

`zwlr_foreign_toplevel_manager_v1` is not in Smithay; vaht serves it
(`toplevel_management.rs`, `protocols/toplevel_management.rs`), as
fluxland did. Each program binding it gets a handle per window, told the
title, program, state and outputs; `WindowFacts::changes` decides what to
tell again, and requests to activate, minimize, maximize, fullscreen or
close a window go to the window manager. Sandboxed programs are not
offered it. The test `a_taskbar_outside_vaht_sees_and_works_the_windows`
works two windows through it.

### 8.7 Monitors off and on

`zwlr_output_power_manager_v1` (`output_power.rs`) turns an output's
monitor off without changing the layout, as `wlopm` and swayidle's
`output * power off` ask: on the DRM backend its CRTC is cleared (DPMS
off) and it is not drawn until turned on, when it is drawn anew. Any input
turns every output that is off on again, so a screen is never left dark
with no way back; each control is told the mode whenever it changes. The
test `an_output_turned_off_comes_on_again_when_asked_or_at_any_input`
drives two controls and the pointer.

### 8.8 Night light

`zwlr_gamma_control_manager_v1` (`gamma.rs`) lets one program at a time
set an output's gamma ramps, as wlsunset and gammastep do: it is told the
CRTC's ramp size and sends a red, green and blue ramp. On the DRM backend
they are set with the legacy gamma call, which atomic drivers turn into
the CRTC's gamma LUT, and set again when vaht's terminal is shown again;
when the control goes, or sends too little, the identity ramp puts the
colors back. Nested, the host's colors are used, so controls fail. The
test `one_night_light_at_a_time_changes_an_outputs_colors` drives three
controls. Not yet seen on hardware: whether NVIDIA's driver takes the
legacy call.

### 8.9 Outputs arranged by other programs

`zwlr_output_manager_v1` (`output_management.rs`) gives each program a
head per connected output with its modes and state, told again with a
new serial whenever anything changes (`HeadState`, compared after each
loop). A configuration applied is checked (some output stays on, each mode
is one the output has, scales from 0.25 to 8) and carried out as
`[[output]]` entries arranged exactly, no profile applying; it lasts until
`outputs.toml` is read again or monitors come or go, when `outputs.toml`
applies again (kanshi applies its own again then too). A configuration
made for outputs since changed is cancelled. The test
`a_program_arranging_outputs_sees_them_and_moves_them` swaps two outputs,
is cancelled, fails to turn both off, and turns one off.


## 9. The DRM backend (built; not yet run on hardware)

`vaht` with no host compositor (or `--backend drm`) takes the seat with
libseat (logind or seatd), opens every GPU udev lists, and drives each
monitor with a `DrmOutput`. Everything that decides is in
`backend/drm/plan.rs` and tested without hardware (RUST.md R-TEST8); the
rest only carries the decisions out.

- **GPUs.** The primary GPU (the one udev calls primary, else the first)
  composes. Every GPU with a render node is added to Smithay's `GpuManager`;
  a display-only device (no render node, such as `simpledrm`) is drawn for
  by the primary one. Smithay makes each GPU's EGL display and renderer
  itself, so the backend has no `unsafe` code.
- **Drivers.** vaht is GPU-agnostic; what one driver needs done differently
  is in `GpuQuirks::for_driver`, keyed by the driver's name. Today:
  NVIDIA's proprietary driver shows overlay planes wrongly, so windows are
  not put on them there (Smithay's anvil does the same). AMD (`amdgpu`),
  Intel and the rest use every plane. Machines in mind: the RTX 3080
  Mobile laptop now (NVIDIA 615, one GPU) and a flagship AMD desktop to
  come; each new quirk is a row in that function and its test.
- **Monitors.** Each connector is scanned when its GPU appears, on udev's
  change events (plugged and unplugged), and on resume. Outputs are named
  by connector (`HDMI-A-1`) and carry the EDID's make, model and serial,
  which `outputs.toml` matches. Modes come from `plan::choose_mode`: the
  size asked at the refresh rate nearest the one asked (within half a hertz
  counts as exact), the fastest of that size without one, the monitor's
  preferred mode when it lacks the size. Colour is 10 bits per channel
  where possible. VR headsets (`non-desktop`) are left alone.
- **Frames.** Each output's `FrameState` (idle, due, queued, throttled)
  follows its vertical blank: a frame asked for while one is on its way
  waits for it, and a frame that changed nothing waits one refresh before
  the next, so a client committing unchanged frames cannot make vaht spin.
  Windows go on planes directly when they can (`FrameFlags::DEFAULT`).
- **Variable refresh.** Each frame, `plan::wants_vrr` decides per output from
  its `vrr` setting and whether a fullscreen window is on top there.
- **The pointer** is drawn by vaht (`Kind::Cursor`, so it goes on the cursor
  plane), from the xcursor theme.
- **Terminals.** Ctrl+Alt+F1…F12 switch terminals, before any binding or
  menu sees the key. Switching away pauses libinput and the GPUs; back,
  they are resumed, monitors scanned again and every output drawn anew.
- **Input** comes from libinput on the seat; keyboards' lights follow Caps,
  Num and Scroll Lock. Touchpad gestures go to the client under the
  pointer.
- **Sync.** A client's GPU buffer is shown only once drawn
  (`buffer_fences.rs`): at the acquire point the client named with
  explicit sync (`wp_linux_drm_syncobj_v1`, offered when the primary GPU
  supports `syncobj_eventfd`; NVIDIA needs it for games and Xwayland
  without flicker), else when the buffer's own fence allows reading.
- **Presentation.** Each frame carries its surfaces' presentation feedback
  to the page flip, reported with the flip's time and sequence; fifo
  barriers lift at the flip, or after the refresh wait of an unchanged
  frame (`frame_pacing.rs`).
- **Input settings** (`[input]`, 02 §3) are applied to each pointing
  device as libinput reports it, and to all again on reload
  (`pointing_devices.rs`, tested with a recording device).
- **Not yet:** DRM leasing for headsets; dmabuf feedback per surface;
  tearing (not in this Smithay).
- **First run on hardware**, from a free terminal: `vaht` (log in
  `~/.local/state/vaht/vaht.log`); Ctrl+Alt+F<n> back to the old terminal
  at any time.
