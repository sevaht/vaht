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
| wlr-output-management | wlr-randr, kanshi, outside tools | niri implements it. vaht's built-in display profiles go through the same internal output configuration code |
| wlr-foreign-toplevel-management | Taskbars and docks outside the compositor | niri implements it |
| ext-workspace | External pagers | niri implements it |
| ~~wlr-screencopy~~ | | dropped: the installed xdg-desktop-portal-wlr 0.8.4 and grim 1.5 use ext-image-copy-capture |
| wlr-gamma-control | Night-light tools | niri implements it |
| wlr-output-power-management | DPMS via `wlopm` | niri implements it |
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
