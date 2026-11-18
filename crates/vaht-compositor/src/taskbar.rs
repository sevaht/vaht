//! The taskbar: workspace buttons and name, the window list, the tray and
//! the clock, along an output edge (01 §6.3).
//!
//! [`Taskbar`] is a panel like a window's frame: its content
//! ([`TaskbarContent`]) says what it shows, read from the window manager
//! and the clock each time a frame is prepared, and vaht-ui lays it out,
//! paints it and hit-tests it. Clicking an arrow or a window's entry holds
//! it down until released, as a titlebar button is; the compositor then
//! runs what it means ([`TaskbarHit`]). Other presses, and the wheel, go to
//! the `taskbar` and `window-list-entry` mouse bindings.
//!
//! An entry shows its window's icon ([`IconStore`]): the one the window
//! names, else its application's, found by app id. The tray shows programs'
//! status icons ([`Tray`]); any button on one, or the wheel, goes to its
//! program.
//!
//! It spans its edge, or a share of it ([`TaskbarSettings`]). An
//! auto-hiding taskbar stays hidden until the pointer touches its edge, and
//! hides again a while after the pointer leaves it; while hidden, its clock
//! does not wake vaht.
//!
//! The clock wakes vaht when its text changes: at each minute, or each
//! second if its format shows seconds, as the time of day has them, and
//! whenever the time of day jumps ([`crate::wall_clock::WallClock`]). The
//! taskbars share one such alarm and one tray ([`crate::taskbars`]).

use std::num::NonZeroUsize;
use std::time::Duration;

use jiff::{RoundMode, Timestamp, ToSpan, Unit, Zoned, ZonedRound};
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Logical, Point, Rectangle, Scale};
use vaht_ui::{
    Glyph, IconEmphasis, IconStore, Panel, PanelContent, SizePolicy, Theme, ThemeElement,
    Typesetter, Widget, WidgetIcon, WidgetState,
};

use crate::Compositor;
use crate::config::{
    TaskbarAlign, TaskbarEdge, TaskbarSettings, TaskbarWidget, WidthPercent, WindowListScope,
};
use crate::event_source::Timer;
use crate::resources::ResourceLedger;
use crate::scene::{PanelTexture, SceneElement};
use crate::tray::{ItemIcon, Tray, TrayItem, TrayItemKey};
use crate::window_manager::{Area, Exemption, Window, WindowId, WindowManager};

/// What clicking the taskbar can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskbarHit {
    /// The arrow to the workspace before.
    WorkspacePrevious,
    /// The arrow to the workspace after.
    WorkspaceNext,
    /// The workspace's name.
    WorkspaceName,
    /// The arrow to the window before in the window list.
    WindowPrevious,
    /// The arrow to the window after in the window list.
    WindowNext,
    /// A window's entry in the window list.
    Entry(WindowId),
    /// The clock.
    Clock,
    /// A program's status icon in the tray.
    TrayItem(TrayItemKey),
}

impl TaskbarHit {
    /// Returns the window an entry stands for.
    pub(crate) const fn window(self) -> Option<WindowId> {
        match self {
            Self::Entry(window) => Some(window),
            Self::WorkspacePrevious
            | Self::WorkspaceNext
            | Self::WorkspaceName
            | Self::WindowPrevious
            | Self::WindowNext
            | Self::Clock
            | Self::TrayItem(_) => None,
        }
    }

    /// Returns whether it is a button, held down by a press and clicked by
    /// a release on it.
    pub(crate) const fn is_button(self) -> bool {
        match self {
            Self::WorkspacePrevious
            | Self::WorkspaceNext
            | Self::WindowPrevious
            | Self::WindowNext
            | Self::Entry(_) => true,
            Self::WorkspaceName | Self::Clock | Self::TrayItem(_) => false,
        }
    }
}

/// The taskbar, along one edge of the output.
#[derive(Debug)]
pub(crate) struct Taskbar {
    /// What it shows, laid out and painted.
    panel: Panel<TaskbarContent>,
    /// The edge it runs along.
    edge: TaskbarEdge,
    /// How much of the edge it spans.
    width_percent: WidthPercent,
    /// Where along the edge it sits.
    align: TaskbarAlign,
    /// Whether maximized windows leave it uncovered.
    reserves_space: bool,
    /// How long after the pointer leaves it it hides; none if it never
    /// hides.
    hide_delay: Option<Duration>,
    /// Whether it shows, waits hidden for the pointer at its edge, or was
    /// turned off.
    visibility: Visibility,
    /// Hides it, once the pointer has been away long enough.
    hide_timer: Option<Timer<Compositor>>,
    /// Where it is, in global coordinates; set as frames are prepared.
    area: Area,
    /// Its pixels, as the renderer takes them.
    texture: PanelTexture,
    /// Whether its clock shows seconds, and so ticks each second.
    clock_shows_seconds: bool,
    /// How opaque it is drawn, as the theme says.
    opacity: f32,
    /// The output it is on, by connector.
    output: String,
    /// Whether its window list lists only its own output's windows.
    lists_own_output: bool,
    /// How many rows tall it is now.
    rows: u32,
}

/// Whether the taskbar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visibility {
    /// It shows.
    Shown,
    /// It auto-hides, and waits hidden for the pointer at its edge.
    AutoHidden,
    /// It was turned off (`toggle-taskbar`).
    Off,
}

impl Taskbar {
    /// Creates the taskbar `settings` describe, if they want one.
    pub(crate) fn new(settings: &TaskbarSettings, output: &str, is_main: bool) -> Option<Self> {
        // A desktop has one tray: the main taskbar's.
        let widgets: Vec<TaskbarWidget> = settings
            .widgets
            .iter()
            .copied()
            .filter(|widget| is_main || *widget != TaskbarWidget::Tray)
            .collect();
        settings.enabled.then(|| Self {
            panel: Panel::new(
                TaskbarContent {
                    widgets: widgets.clone(),
                    workspace_name: String::new(),
                    entries: Vec::new(),
                    entry_max_width: settings.window_list.entry_width.max(),
                    clock: String::new(),
                    tray: Vec::new(),
                    tray_slot: 0,
                    rows: settings.row_range().0,
                    row_height: 0,
                },
                vaht_ui::Size::default(),
            ),
            edge: settings.edge,
            width_percent: settings.width_percent,
            align: settings.align,
            reserves_space: settings.reserve_space && !settings.auto_hide,
            hide_delay: settings.hide_delay(),
            // An auto-hiding taskbar starts hidden, as Fluxbox's does.
            visibility: if settings.auto_hide {
                Visibility::AutoHidden
            } else {
                Visibility::Shown
            },
            hide_timer: None,
            area: Area::default(),
            texture: PanelTexture::new(),
            clock_shows_seconds: settings.clock.format.shows_seconds()
                && widgets.contains(&TaskbarWidget::Clock),
            opacity: 1.0,
            output: output.to_owned(),
            lists_own_output: settings.window_list.windows == WindowListScope::OwnOutput,
            rows: settings.row_range().0,
        })
    }

    /// Returns the area the taskbar `height` pixels tall takes along its
    /// edge of an output covering `whole`.
    pub(crate) fn area_in(&self, whole: Area, height: u32) -> Area {
        let height = i32::try_from(height)
            .expect("a taskbar is far shorter than i32::MAX")
            .min(whole.size.h);
        let top = match self.edge {
            TaskbarEdge::Top => whole.loc.y,
            TaskbarEdge::Bottom => whole.loc.y + whole.size.h - height,
        };
        let width = self.width_percent.of(whole.size.w);
        let left = self.align.start(whole.loc.x, whole.size.w, width);
        Rectangle::new((left, top).into(), (width, height).into())
    }

    /// Returns whether the taskbar shows: not while auto-hidden or turned
    /// off.
    pub(crate) const fn is_shown(&self) -> bool {
        matches!(self.visibility, Visibility::Shown)
    }

    /// Turns the taskbar off (`toggle-taskbar`), or on again: shown, or
    /// hidden at its edge if it auto-hides. While off it shows for nothing
    /// and reserves no room.
    pub(crate) fn toggle(&mut self) {
        self.hide_timer = None;
        self.visibility = match (self.visibility, self.hide_delay) {
            (Visibility::Off, Some(_)) => Visibility::AutoHidden,
            (Visibility::Off, None) => Visibility::Shown,
            (Visibility::Shown | Visibility::AutoHidden, _) => Visibility::Off,
        };
        tracing::debug!(visibility = ?self.visibility, "taskbar toggled");
    }

    /// Follows the pointer, now at global `point` on an output covering
    /// `whole`: an auto-hiding taskbar shows when the pointer touches its
    /// edge, and hides a while after the pointer leaves it. Returns whether
    /// it showed, so the output is drawn again.
    pub(crate) fn follow_pointer(
        &mut self,
        point: Point<f64, Logical>,
        whole: Area,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) -> bool {
        let Some(delay) = self.hide_delay else {
            return false;
        };
        if self.visibility == Visibility::Off {
            return false;
        }
        let pixel = point.to_i32_floor();
        let edge_row = match self.edge {
            TaskbarEdge::Top => whole.loc.y,
            TaskbarEdge::Bottom => whole.loc.y + whole.size.h - 1,
        };
        let is_at_edge =
            pixel.y == edge_row && pixel.x >= whole.loc.x && pixel.x < whole.loc.x + whole.size.w;
        if self.visibility == Visibility::AutoHidden {
            if is_at_edge {
                self.visibility = Visibility::Shown;
                self.hide_timer = None;
                return true;
            }
            return false;
        }
        if is_at_edge || self.area.contains(pixel) {
            self.hide_timer = None;
        } else if self.hide_timer.as_ref().is_none_or(Timer::has_fired) {
            let (handle, ledger) = event_loop;
            let output = self.output.clone();
            let timer = Timer::once(handle, ledger, delay, move |compositor: &mut Compositor| {
                if let Some(taskbar) = compositor.taskbars.on_output_mut(&output)
                    && taskbar.visibility == Visibility::Shown
                {
                    taskbar.visibility = Visibility::AutoHidden;
                    tracing::debug!(output, "taskbar hidden");
                    // Its clock rests while it cannot be seen.
                    compositor.schedule_clock();
                }
                compositor.schedule_render();
            });
            match timer {
                Ok(timer) => self.hide_timer = Some(timer),
                Err(error) => tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "the taskbar cannot hide"
                ),
            }
        }
        false
    }

    /// Returns `usable` less the taskbar's area, if it reserves its space,
    /// on an output covering `whole`, each of its rows `row_height` pixels
    /// tall.
    pub(crate) fn reserve(&self, usable: Area, whole: Area, row_height: u32) -> Area {
        if !self.reserves_space || self.visibility == Visibility::Off {
            return usable;
        }
        let taken = self.area_in(whole, self.height(row_height));
        let (top, bottom) = (usable.loc.y, usable.loc.y + usable.size.h);
        let (top, bottom) = match self.edge {
            TaskbarEdge::Top => (top.max(taken.loc.y + taken.size.h), bottom),
            TaskbarEdge::Bottom => (top, bottom.min(taken.loc.y)),
        };
        Rectangle::new(
            (usable.loc.x, top).into(),
            (usable.size.w, (bottom - top).max(0)).into(),
        )
    }

    /// Returns how tall the taskbar is, its rows `row_height` pixels tall.
    const fn height(&self, row_height: u32) -> u32 {
        self.rows.saturating_mul(row_height)
    }

    /// Returns whether the taskbar covers global `point`.
    pub(crate) fn covers(&self, point: Point<f64, Logical>) -> bool {
        self.area.contains(point.to_i32_floor())
    }

    /// Returns the widget at global `point` that clicking reports, if any.
    pub(crate) fn hit_at(&self, point: Point<f64, Logical>) -> Option<TaskbarHit> {
        self.panel.hit_at(self.local(point.to_i32_floor()))
    }

    /// Returns the output it is on, by connector.
    pub(crate) fn output(&self) -> &str {
        &self.output
    }

    /// Returns the window of the `number`th entry, from 1 at the left, of
    /// its window list, showing `window_manager`'s windows as `settings`
    /// say, on an output covering `whole`.
    pub(crate) fn entry(
        &self,
        window_manager: &WindowManager,
        settings: &TaskbarSettings,
        (number, whole): (NonZeroUsize, Area),
    ) -> Option<WindowId> {
        let own = self.lists_own_output.then_some(whole);
        TaskbarContent::listed(window_manager, settings, own)
            .get(number.get() - 1)
            .map(|window| window.id())
    }

    /// Returns the global point at the middle of the tray item `key`, or of
    /// the taskbar until it is laid out.
    pub(crate) fn tray_item_middle(&self, key: TrayItemKey) -> Point<f64, Logical> {
        let local = self.panel.area_of(TaskbarHit::TrayItem(key)).map_or_else(
            || {
                Point::from((self.area.size.w, self.area.size.h))
                    .to_f64()
                    .downscale(2.0)
            },
            |rect| {
                Point::from((
                    f64::from(rect.x) + f64::from(rect.width) / 2.0,
                    f64::from(rect.y) + f64::from(rect.height) / 2.0,
                ))
            },
        );
        local + self.area.loc.to_f64()
    }

    /// Returns whether it shows the tray: only the main taskbar does.
    pub(crate) fn has_tray(&self) -> bool {
        self.panel.content().widgets.contains(&TaskbarWidget::Tray)
    }

    /// Returns whether it shows a clock, and whether that shows seconds.
    pub(crate) fn clock(&self) -> Option<bool> {
        self.panel
            .content()
            .widgets
            .contains(&TaskbarWidget::Clock)
            .then_some(self.clock_shows_seconds)
    }

    /// Returns the tooltip for global `point`: a window list entry's title
    /// if it does not fit, as `theme` and `typesetter` lay it out, or a
    /// tray item's text; none elsewhere.
    pub(crate) fn tooltip_at(
        &self,
        point: Point<f64, Logical>,
        theme: &Theme,
        typesetter: &mut Typesetter,
    ) -> Option<String> {
        let hit = self.hit_at(point)?;
        let content = self.panel.content();
        match hit {
            TaskbarHit::Entry(window) => {
                let entry = content
                    .entries
                    .iter()
                    .find(|entry| entry.window == window)?;
                let area = self.panel.area_of(hit)?;
                let style = theme.style(ThemeElement::WindowListEntry, WidgetState::Focused);
                let shown = entry.shown_title();
                let icon = entry
                    .icon
                    .as_ref()
                    .map_or(0, |_| WidgetIcon::span(area.height));
                let needed = typesetter.text_width(&shown, &style.text.font)
                    + style.padding.horizontal()
                    + icon;
                (needed > area.width).then(|| entry.title.clone())
            }
            TaskbarHit::TrayItem(key) => content
                .tray
                .iter()
                .find(|entry| entry.key == key)
                .and_then(|entry| entry.tooltip.clone()),
            TaskbarHit::WorkspacePrevious
            | TaskbarHit::WorkspaceName
            | TaskbarHit::WorkspaceNext
            | TaskbarHit::WindowPrevious
            | TaskbarHit::WindowNext
            | TaskbarHit::Clock => None,
        }
    }

    /// Brings what the taskbar shows up to date with `window_manager`,
    /// `settings` and the time `now`, and paints it to fill its edge of an
    /// output covering `whole`, as `theme` styles it, as many rows tall as
    /// the settings say or its window list needs. Returns whether it
    /// changed height, so windows keep off it anew.
    pub(crate) fn prepare(
        &mut self,
        view: TaskbarView<'_>,
        theme: &Theme,
        typesetter: &mut Typesetter,
        icons: &mut IconStore,
    ) -> bool {
        let row_height = theme.metrics().taskbar_height;
        let old_height = self.area.size.h;
        self.opacity = theme
            .style(ThemeElement::Taskbar, WidgetState::Focused)
            .opacity();
        let shows_tray = self.panel.content().widgets.contains(&TaskbarWidget::Tray);
        let tray_items: Vec<&TrayItem> = view
            .tray
            .filter(|_| shows_tray)
            .map_or_else(Vec::new, |tray| {
                tray.ordered(&view.settings.tray.pin_left, &view.settings.tray.pin_right)
            });
        let own = self.lists_own_output.then_some(view.whole);
        let mut content = TaskbarContent::of(
            &view,
            own,
            &self.panel.content().widgets,
            &tray_items,
            (icons, row_height),
        );
        content.rows = self.rows;
        let width = u32::try_from(self.area_in(view.whole, row_height).size.w).unwrap_or(0);
        if *self.panel.content() != content || self.panel.size().width != width {
            let (fewest, most) = view.settings.row_range();
            let min_entry_width = view
                .settings
                .window_list
                .min_entry_width
                .pixels(view.whole.size.w);
            self.rows = (fewest..most)
                .find(|rows| {
                    content.rows = *rows;
                    content.entries_fit(width, min_entry_width, theme, typesetter)
                })
                .unwrap_or(most);
            content.rows = self.rows;
        }
        self.area = self.area_in(view.whole, self.height(row_height));
        if *self.panel.content() != content {
            self.panel.update(|current| *current = content);
        }
        let size = vaht_ui::Size::new(width, u32::try_from(self.area.size.h).unwrap_or(0));
        self.panel.resize(size);
        self.panel.paint(theme, typesetter);
        self.texture.upload(&self.panel);
        self.area.size.h != old_height
    }

    /// Repaints the taskbar at the next prepare, as when the theme changed.
    pub(crate) fn invalidate(&mut self) {
        self.panel.invalidate();
    }

    /// Lays out and paints the taskbar again at the next prepare, so icons
    /// looked up again show.
    pub(crate) fn forget_icons(&mut self) {
        self.panel.invalidate();
    }

    /// Holds down the button at global `point`, if there is one; returns
    /// whether there was.
    pub(crate) fn press(&mut self, point: Point<f64, Logical>) -> bool {
        let local = self.local(point.to_i32_floor());
        if self.panel.hit_at(local).is_some_and(TaskbarHit::is_button) {
            self.panel.press(local);
            true
        } else {
            false
        }
    }

    /// Lets go of the held button with the pointer at global `point`;
    /// returns it if that clicks it.
    pub(crate) fn release(&mut self, point: Point<f64, Logical>) -> Option<TaskbarHit> {
        self.panel.pressed()?;
        self.panel.release(self.local(point.to_i32_floor()))
    }

    /// Returns the element that draws the taskbar.
    pub(crate) fn render_element<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Option<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.texture
            .render_element(renderer, self.area.loc, scale, self.opacity)
    }

    /// Returns when a clock's text next changes after `now`: at the next
    /// whole second if it `shows_seconds`, else at the next whole minute.
    pub(crate) fn next_clock_change(now: &Zoned, shows_seconds: bool) -> Timestamp {
        let unit = if shows_seconds {
            Unit::Second
        } else {
            Unit::Minute
        };
        let start = now
            .round(ZonedRound::new().smallest(unit).mode(RoundMode::Trunc))
            .expect("truncating a time to a whole minute or second stays in range");
        let next = if shows_seconds {
            start.checked_add(1.second())
        } else {
            start.checked_add(1.minute())
        };
        next.expect("a time a minute ahead is in range").timestamp()
    }

    /// Returns global `pixel` in the taskbar's own pixels.
    fn local(&self, pixel: Point<i32, Logical>) -> vaht_ui::Point {
        let inside = pixel - self.area.loc;
        vaht_ui::Point::new(inside.x, inside.y)
    }
}

/// What the taskbar shows comes from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TaskbarView<'state> {
    /// The windows and workspaces.
    pub(crate) window_manager: &'state WindowManager,
    /// The taskbar's settings.
    pub(crate) settings: &'state TaskbarSettings,
    /// The output's area.
    pub(crate) whole: Area,
    /// The time.
    pub(crate) now: &'state Zoned,
    /// The tray, if it started.
    pub(crate) tray: Option<&'state Tray<Compositor>>,
}

/// What the taskbar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskbarContent {
    /// Its widgets, left to right.
    widgets: Vec<TaskbarWidget>,
    /// The name of the workspace shown.
    workspace_name: String,
    /// The window list's entries, in the order the windows opened.
    entries: Vec<WindowListEntry>,
    /// The most an entry is wide.
    entry_max_width: u32,
    /// The clock's text.
    clock: String,
    /// The tray's items, in their order.
    tray: Vec<TrayEntry>,
    /// How wide each tray item is: as tall as a row.
    tray_slot: u32,
    /// How many rows the taskbar is.
    rows: u32,
    /// How tall each row is.
    row_height: u32,
}

impl TaskbarContent {
    /// How much room the tray leaves at each end, beside the gap each icon
    /// keeps around it: with it, icons are 2 pixels apart and 2 from the
    /// ends, as Fluxbox lays its tray out.
    const TRAY_END_MARGIN: u32 = 1;

    /// Returns what the taskbar of `widgets` shows of `view`, and of
    /// `tray_items`, its icons from `icons`' store, sized for a taskbar of
    /// its height.
    fn of(
        view: &TaskbarView<'_>,
        own: Option<Area>,
        widgets: &[TaskbarWidget],
        tray_items: &[&TrayItem],
        icons: (&mut IconStore, u32),
    ) -> Self {
        let (store, height) = icons;
        let icon_size = WidgetIcon::side(height);
        let tray = tray_items
            .iter()
            .map(|item| TrayEntry {
                key: item.key,
                tooltip: item
                    .tooltip
                    .clone()
                    .filter(|text| !text.trim().is_empty())
                    .or_else(|| (!item.title.is_empty()).then(|| item.title.clone())),
                icon: match &item.icon {
                    ItemIcon::Named {
                        name,
                        own_directory,
                    } => store.named_in(name, icon_size, own_directory.as_deref()),
                    ItemIcon::Pixels(icon) => Some(icon.clone()),
                    ItemIcon::Missing => None,
                },
            })
            .collect();
        let shows_icons = view.settings.window_list.icons && icon_size > 0;
        let manager = view.window_manager;
        Self {
            widgets: widgets.to_vec(),
            workspace_name: manager.current_workspace_name().to_owned(),
            entries: Self::listed(manager, view.settings, own)
                .into_iter()
                .map(|window| WindowListEntry {
                    window: window.id(),
                    title: window.title().to_owned(),
                    is_focused: manager.focused() == Some(window.id()),
                    is_minimized: window.state().is_minimized,
                    icon: shows_icons
                        .then(|| window.icon_in(store, icon_size))
                        .flatten(),
                })
                .collect(),
            entry_max_width: view.settings.window_list.entry_width.max(),
            clock: view.settings.clock.format.format(view.now),
            tray,
            tray_slot: WidgetIcon::span(height),
            rows: 1,
            row_height: height,
        }
    }

    /// Returns the windows of `manager` the window list shows as `settings`
    /// say, in the order they opened.
    fn listed<'state>(
        manager: &'state WindowManager,
        settings: &TaskbarSettings,
        own: Option<Area>,
    ) -> Vec<&'state Window> {
        let current = manager.current_workspace();
        let is_on_own_output = |window: &Window| {
            own.is_none_or(|own| {
                let area = window.content_area();
                let middle = area.loc + area.size.downscale(2).to_point();
                own.contains(middle)
            })
        };
        let mut windows: Vec<&Window> = manager
            .windows_top_down()
            .filter(|window| {
                window.is_placed()
                    && !window.is_exempt(Exemption::WindowList)
                    && settings.window_list.show.matches(window, current)
                    && is_on_own_output(window)
            })
            .collect();
        windows.sort_by_key(|window| window.id());
        windows
    }

    /// Returns the widget a taskbar widget is.
    fn widget(&self, widget: TaskbarWidget) -> Widget<TaskbarHit> {
        match widget {
            TaskbarWidget::WorkspacePrevious => {
                self.arrow(Glyph::ArrowLeft, TaskbarHit::WorkspacePrevious)
            }
            TaskbarWidget::WorkspaceNext => {
                self.arrow(Glyph::ArrowRight, TaskbarHit::WorkspaceNext)
            }
            TaskbarWidget::WindowPrevious => {
                self.arrow(Glyph::ArrowLeft, TaskbarHit::WindowPrevious)
            }
            TaskbarWidget::WindowNext => self.arrow(Glyph::ArrowRight, TaskbarHit::WindowNext),
            TaskbarWidget::WorkspaceName => {
                Widget::text(ThemeElement::TaskbarWorkspace, self.workspace_name.clone())
                    .with_hit(TaskbarHit::WorkspaceName)
            }
            TaskbarWidget::WindowList => self.window_list(),
            TaskbarWidget::Clock => Widget::text(ThemeElement::TaskbarClock, self.clock.clone())
                .with_hit(TaskbarHit::Clock),
            TaskbarWidget::Tray => self.tray_widget(),
        }
    }

    /// Returns an arrow button showing `glyph`, one row wide however many
    /// rows the taskbar is.
    fn arrow(&self, glyph: Glyph, hit: TaskbarHit) -> Widget<TaskbarHit> {
        Widget::glyph(ThemeElement::TaskbarButton, glyph)
            .with_size(SizePolicy::Fixed(self.row_height))
            .with_button(hit)
    }

    /// Returns how many entries go on each of the taskbar's rows: the
    /// rows fill in order, all but the last full.
    fn per_row(&self, count: usize) -> usize {
        let rows = usize::try_from(self.rows.max(1)).unwrap_or(1);
        count.div_ceil(rows).max(1)
    }

    /// Returns the window list: its entries in order, wrapped across the
    /// taskbar's rows, every entry as wide as the rest; a short last row
    /// leaves the rest of it empty.
    fn window_list(&self) -> Widget<TaskbarHit> {
        let per_row = if self.rows <= 1 {
            self.entries.len()
        } else {
            self.per_row(self.entries.len())
        };
        let entry_row = |entries: &[WindowListEntry]| {
            let gap = Widget::fill(ThemeElement::WindowListEmpty).with_size(SizePolicy::Share {
                max: self.entry_max_width,
            });
            let mut widgets: Vec<Widget<TaskbarHit>> = entries
                .iter()
                .map(|entry| entry.widget(self.entry_max_width))
                .collect();
            widgets.resize(per_row.max(widgets.len()), gap);
            Widget::row(ThemeElement::WindowListEmpty, widgets)
        };
        if self.rows <= 1 {
            return entry_row(&self.entries);
        }
        let mut rows: Vec<Widget<TaskbarHit>> =
            self.entries.chunks(per_row).map(entry_row).collect();
        let count = usize::try_from(self.rows).unwrap_or(usize::MAX);
        rows.resize_with(count.max(rows.len()), || entry_row(&[]));
        Widget::column(ThemeElement::WindowListEmpty, rows)
    }

    /// Returns whether the window list's entries are at least
    /// `min_entry_width` wide with the taskbar as many rows as it is now,
    /// `width` wide, as `theme` styles it.
    fn entries_fit(
        &self,
        width: u32,
        min_entry_width: u32,
        theme: &Theme,
        typesetter: &mut Typesetter,
    ) -> bool {
        if self.entries.is_empty() {
            return true;
        }
        let size = vaht_ui::Size::new(width, self.rows.saturating_mul(self.row_height));
        let layout = Panel::lay_out(self, size, theme, typesetter);
        // The list is the first widget drawn on its background.
        let Some(list) = layout
            .placed()
            .iter()
            .find(|placed| placed.element == ThemeElement::WindowListEmpty)
        else {
            return true;
        };
        let per_row = u32::try_from(self.per_row(self.entries.len())).unwrap_or(u32::MAX);
        list.rect.width / per_row >= min_entry_width
    }

    /// Returns the tray: a slot per item, drawn on the taskbar's own
    /// background, as wide as its items need, wrapped across the taskbar's
    /// rows.
    fn tray_widget(&self) -> Widget<TaskbarHit> {
        if self.tray.is_empty() {
            return Widget::fill(ThemeElement::Taskbar).with_size(SizePolicy::Fixed(0));
        }
        let margin = || {
            Widget::fill(ThemeElement::Taskbar).with_size(SizePolicy::Fixed(Self::TRAY_END_MARGIN))
        };
        let slots: Vec<Widget<TaskbarHit>> = self
            .tray
            .iter()
            .map(|entry| {
                let slot = Widget::fill(ThemeElement::Taskbar)
                    .with_size(SizePolicy::Fixed(self.tray_slot))
                    .with_hit(TaskbarHit::TrayItem(entry.key));
                match &entry.icon {
                    Some(icon) => slot.with_icon(WidgetIcon {
                        icon: icon.clone(),
                        emphasis: IconEmphasis::Full,
                    }),
                    None => slot,
                }
            })
            .collect();
        let per_row = self.per_row(slots.len());
        let columns = u32::try_from(per_row).unwrap_or(u32::MAX);
        let width = columns
            .saturating_mul(self.tray_slot)
            .saturating_add(2 * Self::TRAY_END_MARGIN);
        let tray_row = |row: &[Widget<TaskbarHit>]| {
            Widget::row(
                ThemeElement::Taskbar,
                std::iter::once(margin())
                    .chain(row.iter().cloned())
                    .chain(std::iter::once(Widget::fill(ThemeElement::Taskbar)))
                    .collect(),
            )
        };
        let rows: Vec<Widget<TaskbarHit>> = slots.chunks(per_row).map(tray_row).collect();
        let tray = match <[_; 1]>::try_from(rows) {
            Ok([row]) => row,
            Err(rows) => {
                let mut rows = rows;
                let count = usize::try_from(self.rows).unwrap_or(usize::MAX);
                rows.resize_with(count.max(rows.len()), || tray_row(&[]));
                Widget::column(ThemeElement::Taskbar, rows)
            }
        };
        tray.with_size(SizePolicy::Fixed(width))
    }
}

impl PanelContent for TaskbarContent {
    type Hit = TaskbarHit;

    fn build(&self) -> Widget<TaskbarHit> {
        Widget::row(
            ThemeElement::Taskbar,
            self.widgets
                .iter()
                .map(|widget| self.widget(*widget))
                .collect(),
        )
    }

    fn state(&self) -> WidgetState {
        WidgetState::Focused
    }
}

/// One item of the tray as the taskbar shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayEntry {
    /// The item.
    key: TrayItemKey,
    /// Its tooltip: its own, else its title; none when both are empty.
    tooltip: Option<String>,
    /// Its icon, if one was found.
    icon: Option<vaht_ui::Icon>,
}

/// One window's entry in the window list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowListEntry {
    /// The window.
    window: WindowId,
    /// Its title.
    title: String,
    /// Whether it has focus.
    is_focused: bool,
    /// Whether it is minimized: its title is then shown in parentheses, as
    /// Fluxbox shows it, and its icon faded.
    is_minimized: bool,
    /// Its icon, if it shows one.
    icon: Option<vaht_ui::Icon>,
}

impl WindowListEntry {
    /// Returns the title as the entry shows it: in parentheses when the
    /// window is minimized, as Fluxbox shows it.
    fn shown_title(&self) -> String {
        if self.is_minimized {
            format!("({})", self.title)
        } else {
            self.title.clone()
        }
    }

    /// Returns the entry's widget, at most `max_width` wide.
    fn widget(&self, max_width: u32) -> Widget<TaskbarHit> {
        let title = self.shown_title();
        let state = if self.is_focused {
            WidgetState::Focused
        } else {
            WidgetState::Unfocused
        };
        let entry = Widget::text(ThemeElement::WindowListEntry, title)
            .with_size(SizePolicy::Share { max: max_width })
            .with_button(TaskbarHit::Entry(self.window))
            .with_state(state);
        match &self.icon {
            Some(icon) => entry.with_icon(WidgetIcon {
                icon: icon.clone(),
                emphasis: if self.is_minimized {
                    IconEmphasis::Faded
                } else {
                    IconEmphasis::Full
                },
            }),
            None => entry,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A 1920 × 1080 output.
    fn whole() -> Area {
        Rectangle::from_size((1920, 1080).into())
    }

    /// The default taskbar, on `edge`.
    fn taskbar(edge: TaskbarEdge) -> Taskbar {
        let settings = TaskbarSettings {
            edge,
            ..TaskbarSettings::default()
        };
        Taskbar::new(&settings, "OUT", true).expect("enabled by default")
    }

    #[rstest]
    #[case::bottom(TaskbarEdge::Bottom, (0, 1060), (0, 1060))]
    #[case::top(TaskbarEdge::Top, (0, 0), (20, 1060))]
    fn the_taskbar_runs_along_its_edge_and_windows_keep_off_it(
        #[case] edge: TaskbarEdge,
        #[case] corner: (i32, i32),
        #[case] usable_span: (i32, i32),
    ) {
        let bar = taskbar(edge);
        let area = bar.area_in(whole(), 20);
        assert_eq!((area.loc.x, area.loc.y), corner);
        assert_eq!((area.size.w, area.size.h), (1920, 20));
        let usable = bar.reserve(whole(), whole(), 20);
        assert_eq!((usable.loc.y, usable.size.h), usable_span);
    }

    #[test]
    fn a_taskbar_turned_off_shows_for_nothing_and_reserves_no_room() {
        let mut bar = taskbar(TaskbarEdge::Bottom);
        bar.toggle();
        assert!(!bar.is_shown());
        assert_eq!(bar.reserve(whole(), whole(), 20), whole());
        bar.toggle();
        assert!(bar.is_shown(), "on again");
        assert_ne!(bar.reserve(whole(), whole(), 20), whole());
    }

    #[test]
    fn a_taskbar_that_reserves_nothing_leaves_the_usable_area_alone() {
        let settings = TaskbarSettings {
            reserve_space: false,
            ..TaskbarSettings::default()
        };
        let bar = Taskbar::new(&settings, "OUT", true).expect("enabled");
        assert_eq!(bar.reserve(whole(), whole(), 20), whole());
    }

    #[rstest]
    #[case::left(TaskbarAlign::Left, 0)]
    #[case::center(TaskbarAlign::Center, 480)]
    #[case::right(TaskbarAlign::Right, 960)]
    fn a_narrower_taskbar_sits_where_it_is_aligned(#[case] align: TaskbarAlign, #[case] left: i32) {
        let settings = TaskbarSettings {
            width_percent: WidthPercent::new(50).expect("a width"),
            align,
            ..TaskbarSettings::default()
        };
        let bar = Taskbar::new(&settings, "OUT", true).expect("enabled");
        let area = bar.area_in(whole(), 20);
        assert_eq!((area.loc.x, area.size.w), (left, 960));
    }

    #[test]
    fn an_auto_hiding_taskbar_starts_hidden_and_reserves_nothing() {
        let settings = TaskbarSettings {
            auto_hide: true,
            ..TaskbarSettings::default()
        };
        let bar = Taskbar::new(&settings, "OUT", true).expect("enabled");
        assert!(!bar.is_shown());
        assert_eq!(bar.reserve(whole(), whole(), 20), whole());
    }

    #[test]
    fn touching_its_edge_shows_an_auto_hiding_taskbar() {
        let event_loop: calloop::EventLoop<'static, Compositor> =
            calloop::EventLoop::try_new().expect("a loop");
        let ledger = ResourceLedger::new();
        let settings = TaskbarSettings {
            auto_hide: true,
            ..TaskbarSettings::default()
        };
        let mut bar = Taskbar::new(&settings, "OUT", true).expect("enabled");
        let handle = event_loop.handle();
        assert!(!bar.follow_pointer((500.0, 1070.0).into(), whole(), (&handle, &ledger)));
        assert!(!bar.is_shown(), "near the edge is not at it");
        assert!(bar.follow_pointer((500.0, 1079.5).into(), whole(), (&handle, &ledger)));
        assert!(bar.is_shown());
    }

    #[test]
    fn widths_outside_one_to_a_hundred_percent_are_refused() {
        assert!(WidthPercent::new(0).is_none());
        assert!(WidthPercent::new(101).is_none());
        assert_eq!(WidthPercent::new(1).expect("a width").of(1920), 19);
    }

    #[test]
    fn no_taskbar_when_disabled() {
        let settings = TaskbarSettings {
            enabled: false,
            ..TaskbarSettings::default()
        };
        assert!(Taskbar::new(&settings, "OUT", true).is_none());
    }

    /// A taskbar's content, `rows` rows of 20 pixels, with a tray of
    /// `items` items and a window list of `windows` windows.
    fn content(rows: u32, items: u64, windows: u64) -> TaskbarContent {
        TaskbarContent {
            widgets: vec![TaskbarWidget::WindowList, TaskbarWidget::Tray],
            workspace_name: String::new(),
            entries: (1..=windows)
                .map(|window| WindowListEntry {
                    window: WindowId::new(window),
                    title: format!("window {window}"),
                    is_focused: false,
                    is_minimized: false,
                    icon: None,
                })
                .collect(),
            entry_max_width: 128,
            clock: String::new(),
            tray: (1..=items)
                .map(|key| TrayEntry {
                    key: TrayItemKey(key),
                    tooltip: None,
                    icon: None,
                })
                .collect(),
            tray_slot: 20,
            rows,
            row_height: 20,
        }
    }

    /// Returns what clicking each child of `widget`, a row, reports.
    fn hits_of(widget: &Widget<TaskbarHit>) -> Vec<Option<TaskbarHit>> {
        let vaht_ui::widget::WidgetKind::Row(children) = &widget.kind else {
            panic!("a row");
        };
        children.iter().map(|child| child.hit).collect()
    }

    /// Returns the children of `widget`, a column.
    fn rows_of(widget: &Widget<TaskbarHit>) -> &[Widget<TaskbarHit>] {
        let vaht_ui::widget::WidgetKind::Column(rows) = &widget.kind else {
            panic!("a column");
        };
        rows
    }

    #[test]
    fn the_tray_is_a_slot_per_item_with_a_pixel_at_each_end() {
        let tray = content(1, 2, 0).tray_widget();
        assert_eq!(
            tray.size,
            SizePolicy::Fixed(42),
            "two 20-pixel slots and two 1-pixel ends"
        );
        let item = |key| Some(TaskbarHit::TrayItem(TrayItemKey(key)));
        assert_eq!(hits_of(&tray), [None, item(1), item(2), None]);
        let vaht_ui::widget::WidgetKind::Row(children) = tray.kind else {
            panic!("the tray is a row");
        };
        assert!(!children[1].is_button, "an item acts as it is pressed");
    }

    #[test]
    fn a_taller_taskbar_wraps_its_tray_into_its_rows() {
        let tray = content(2, 3, 0).tray_widget();
        assert_eq!(
            tray.size,
            SizePolicy::Fixed(42),
            "two columns of 20-pixel slots, and the ends"
        );
        let item = |key| Some(TaskbarHit::TrayItem(TrayItemKey(key)));
        let rows = rows_of(&tray);
        assert_eq!(hits_of(&rows[0]), [None, item(1), item(2), None]);
        assert_eq!(hits_of(&rows[1]), [None, item(3), None], "the rest");
    }

    #[test]
    fn a_taller_taskbar_wraps_its_window_list_and_keeps_empty_rows() {
        let entry = |window| Some(TaskbarHit::Entry(WindowId::new(window)));
        let list = content(2, 0, 3).window_list();
        let rows = rows_of(&list);
        assert_eq!(hits_of(&rows[0]), [entry(1), entry(2)]);
        assert_eq!(hits_of(&rows[1]), [entry(3), None], "and room for one more");
        let list = content(3, 0, 1).window_list();
        assert_eq!(rows_of(&list).len(), 3, "a row per taskbar row");
        assert_eq!(hits_of(&content(1, 0, 3).window_list()).len(), 3, "one row");
    }

    #[test]
    fn arrows_stay_one_row_wide_on_a_taller_taskbar() {
        let arrow = content(3, 0, 0).arrow(Glyph::ArrowLeft, TaskbarHit::WindowPrevious);
        assert_eq!(arrow.size, SizePolicy::Fixed(20));
    }

    #[test]
    fn entries_are_buttons_drawn_focused_or_not_and_minimized_in_parentheses() {
        let entry = WindowListEntry {
            window: WindowId::new(3),
            title: "kitty".to_owned(),
            is_focused: false,
            is_minimized: true,
            icon: None,
        };
        let widget = entry.widget(128);
        assert_eq!(
            widget.kind,
            vaht_ui::widget::WidgetKind::Text("(kitty)".to_owned())
        );
        assert_eq!(widget.hit, Some(TaskbarHit::Entry(WindowId::new(3))));
        assert!(widget.is_button);
        assert_eq!(widget.state, Some(WidgetState::Unfocused));
    }

    #[rstest]
    #[case::minutes(false, "2026-11-13T09:42:00Z")]
    #[case::seconds(true, "2026-11-13T09:41:43Z")]
    fn the_clock_wakes_vaht_when_its_text_changes(
        #[case] shows_seconds: bool,
        #[case] expected: &str,
    ) {
        let now = jiff::civil::date(2026, 11, 13)
            .at(9, 41, 42, 250_000_000)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .expect("a valid time");
        assert_eq!(
            Taskbar::next_clock_change(&now, shows_seconds),
            expected.parse::<Timestamp>().expect("a timestamp")
        );
    }
}
