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
//! whenever the time of day jumps ([`WallClock`]).

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
use crate::bus::Bus;
use crate::config::{TaskbarAlign, TaskbarEdge, TaskbarSettings, TaskbarWidget, WidthPercent};
use crate::event_source::Timer;
use crate::resources::ResourceLedger;
use crate::scene::{PanelTexture, SceneElement};
use crate::tray::{ItemIcon, Tray, TrayAction, TrayHooks, TrayItem, TrayItemKey};
use crate::wall_clock::{ClockHooks, WallClock, ZoneSources};
use crate::window_manager::{Area, ClientIcon, Window, WindowId, WindowManager};

/// What clicking the taskbar can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskbarHit {
    /// The arrow to the workspace before.
    WorkspacePrevious,
    /// The arrow to the workspace after.
    WorkspaceNext,
    /// The workspace's name.
    WorkspaceName,
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
            | Self::Clock
            | Self::TrayItem(_) => None,
        }
    }

    /// Returns whether it is a button, held down by a press and clicked by
    /// a release on it.
    pub(crate) const fn is_button(self) -> bool {
        match self {
            Self::WorkspacePrevious | Self::WorkspaceNext | Self::Entry(_) => true,
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
    /// Whether it is hidden, waiting for the pointer at its edge.
    is_hidden: bool,
    /// Hides it, once the pointer has been away long enough.
    hide_timer: Option<Timer<Compositor>>,
    /// Where it is, in global coordinates; set as frames are prepared.
    area: Area,
    /// Its pixels, as the renderer takes them.
    texture: PanelTexture,
    /// Whether its clock shows seconds, and so ticks each second.
    clock_shows_seconds: bool,
    /// Wakes vaht when the clock's text next changes.
    wall_clock: Option<WallClock<Compositor>>,
    /// The entries' and tray items' icons.
    icons: IconStore,
    /// The tray, once started, if the taskbar shows one.
    tray: Option<Tray<Compositor>>,
}

impl Taskbar {
    /// Creates the taskbar `settings` describe, if they want one.
    pub(crate) fn new(settings: &TaskbarSettings) -> Option<Self> {
        settings.enabled.then(|| Self {
            panel: Panel::new(
                TaskbarContent {
                    widgets: settings.widgets.clone(),
                    workspace_name: String::new(),
                    entries: Vec::new(),
                    entry_max_width: settings.window_list.entry_width.max,
                    clock: String::new(),
                    tray: Vec::new(),
                    tray_slot: 0,
                },
                vaht_ui::Size::default(),
            ),
            edge: settings.edge,
            width_percent: settings.width_percent,
            align: settings.align,
            reserves_space: settings.reserve_space && !settings.auto_hide,
            hide_delay: settings.hide_delay(),
            // An auto-hiding taskbar starts hidden, as Fluxbox's does.
            is_hidden: settings.auto_hide,
            hide_timer: None,
            area: Area::default(),
            texture: PanelTexture::new(),
            clock_shows_seconds: settings.clock.format.shows_seconds()
                && settings.widgets.contains(&TaskbarWidget::Clock),
            wall_clock: None,
            icons: IconStore::from_environment(),
            tray: None,
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

    /// Returns whether the taskbar shows: not while auto-hidden.
    pub(crate) const fn is_shown(&self) -> bool {
        !self.is_hidden
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
        let pixel = point.to_i32_floor();
        let edge_row = match self.edge {
            TaskbarEdge::Top => whole.loc.y,
            TaskbarEdge::Bottom => whole.loc.y + whole.size.h - 1,
        };
        let is_at_edge =
            pixel.y == edge_row && pixel.x >= whole.loc.x && pixel.x < whole.loc.x + whole.size.w;
        if self.is_hidden {
            if is_at_edge {
                self.is_hidden = false;
                self.hide_timer = None;
                return true;
            }
            return false;
        }
        if is_at_edge || self.area.contains(pixel) {
            self.hide_timer = None;
        } else if self.hide_timer.as_ref().is_none_or(Timer::has_fired) {
            let (handle, ledger) = event_loop;
            let timer = Timer::once(handle, ledger, delay, |compositor: &mut Compositor| {
                if let Some(taskbar) = &mut compositor.taskbar {
                    taskbar.is_hidden = true;
                    tracing::debug!("taskbar hidden");
                    // Its clock rests while it cannot be seen.
                    taskbar.schedule_clock(
                        &Zoned::now(),
                        (&compositor.loop_handle, &compositor.ledger),
                    );
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
    /// on an output covering `whole`.
    pub(crate) fn reserve(&self, usable: Area, whole: Area, height: u32) -> Area {
        if !self.reserves_space {
            return usable;
        }
        let taken = self.area_in(whole, height);
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

    /// Returns whether the taskbar covers global `point`.
    pub(crate) fn covers(&self, point: Point<f64, Logical>) -> bool {
        self.area.contains(point.to_i32_floor())
    }

    /// Returns the widget at global `point` that clicking reports, if any.
    pub(crate) fn hit_at(&self, point: Point<f64, Logical>) -> Option<TaskbarHit> {
        self.panel.hit_at(self.local(point.to_i32_floor()))
    }

    /// Brings what the taskbar shows up to date with `window_manager`,
    /// `settings` and the time `now`, and paints it to fill its edge of an
    /// output covering `whole`, `height` pixels tall, as `theme` styles it.
    pub(crate) fn prepare(
        &mut self,
        view: TaskbarView<'_>,
        theme: &Theme,
        typesetter: &mut Typesetter,
    ) {
        let height = theme.metrics().taskbar_height;
        self.area = self.area_in(view.whole, height);
        let tray_items: Vec<&TrayItem> = self.tray.as_ref().map_or_else(Vec::new, |tray| {
            tray.ordered(&view.settings.tray.pin_left, &view.settings.tray.pin_right)
        });
        let content = TaskbarContent::of(
            &view,
            &self.panel.content().widgets,
            &tray_items,
            (&mut self.icons, height),
        );
        if *self.panel.content() != content {
            self.panel.update(|current| *current = content);
        }
        let size = vaht_ui::Size::new(
            u32::try_from(self.area.size.w).unwrap_or(0),
            u32::try_from(self.area.size.h).unwrap_or(0),
        );
        self.panel.resize(size);
        self.panel.paint(theme, typesetter);
        self.texture.upload(&self.panel);
    }

    /// Repaints the taskbar at the next prepare, as when the theme changed.
    pub(crate) fn invalidate(&mut self) {
        self.panel.invalidate();
    }

    /// Looks icons up again at the next prepare, so icons installed or
    /// changed since show.
    pub(crate) fn forget_icons(&mut self) {
        self.icons.forget();
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
            .render_element(renderer, self.area.loc, scale, 1.0)
    }

    /// Wakes vaht to draw the clock anew when its text next changes, at
    /// `now`: the next whole minute, or second if it shows seconds.
    pub(crate) fn schedule_clock(
        &mut self,
        now: &Zoned,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        if !self.panel.content().widgets.contains(&TaskbarWidget::Clock) {
            self.wall_clock = None;
            return;
        }
        if self.wall_clock.is_none() {
            let (handle, ledger) = event_loop;
            let hooks = ClockHooks {
                on_change: Compositor::show_time_change,
                find: Self::wall_clock_of,
            };
            let started = WallClock::start(handle, ledger, &ZoneSources::system(), hooks);
            match started {
                Ok(wall_clock) => self.wall_clock = Some(wall_clock),
                Err(error) => {
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        "the clock cannot tick"
                    );
                    return;
                }
            }
        }
        let Some(wall_clock) = &self.wall_clock else {
            return;
        };
        let armed = if self.is_hidden {
            wall_clock.disarm()
        } else {
            wall_clock.ring_at(Self::next_clock_change(now, self.clock_shows_seconds))
        };
        if let Err(error) = armed {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not schedule the clock"
            );
        }
    }

    /// Starts the tray, if the taskbar shows one and it has not started.
    pub(crate) fn start_tray(
        &mut self,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        if !self.panel.content().widgets.contains(&TaskbarWidget::Tray) || self.tray.is_some() {
            return;
        }
        let (handle, ledger) = event_loop;
        let hooks = TrayHooks {
            on_change: Compositor::schedule_render,
            find: Self::tray_of,
        };
        match Tray::start(handle, ledger, Bus::Session, hooks) {
            Ok(tray) => self.tray = Some(tray),
            Err(error) => tracing::warn!(%error, "the tray cannot start"),
        }
    }

    /// Asks the tray item `key` for `action`, the pointer at global `at`.
    pub(crate) fn act_on_tray(
        &self,
        key: TrayItemKey,
        action: TrayAction,
        at: Point<f64, Logical>,
    ) {
        if let Some(tray) = &self.tray {
            tray.act(key, action, at.to_i32_round());
        }
    }

    /// Returns the taskbar's tray in `compositor`, while it has one.
    fn tray_of(compositor: &mut Compositor) -> Option<&mut Tray<Compositor>> {
        compositor.taskbar.as_mut()?.tray.as_mut()
    }

    /// Returns the taskbar's wall clock in `compositor`, while it has one.
    fn wall_clock_of(compositor: &mut Compositor) -> Option<&mut WallClock<Compositor>> {
        compositor.taskbar.as_mut()?.wall_clock.as_mut()
    }

    /// Returns when a clock's text next changes after `now`: at the next
    /// whole second if it `shows_seconds`, else at the next whole minute.
    fn next_clock_change(now: &Zoned, shows_seconds: bool) -> Timestamp {
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
    /// How wide each tray item is: as tall as the taskbar.
    tray_slot: u32,
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
        let current = manager.current_workspace();
        let mut windows: Vec<&Window> = manager
            .windows_top_down()
            .filter(|window| {
                window.is_placed() && view.settings.window_list.show.matches(window, current)
            })
            .collect();
        windows.sort_by_key(|window| window.id());
        Self {
            widgets: widgets.to_vec(),
            workspace_name: manager.current_workspace_name().to_owned(),
            entries: windows
                .into_iter()
                .map(|window| WindowListEntry {
                    window: window.id(),
                    title: window.title().to_owned(),
                    is_focused: manager.focused() == Some(window.id()),
                    is_minimized: window.state().is_minimized,
                    icon: shows_icons
                        .then(|| {
                            match window.icon() {
                                Some(ClientIcon::Named(name)) => store.named(name, icon_size),
                                Some(ClientIcon::Pixels(icon)) => Some(icon.clone()),
                                None => None,
                            }
                            .or_else(|| store.app_icon(window.app_id(), icon_size))
                        })
                        .flatten(),
                })
                .collect(),
            entry_max_width: view.settings.window_list.entry_width.max,
            clock: view.settings.clock.format.format(view.now),
            tray,
            tray_slot: WidgetIcon::span(height),
        }
    }

    /// Returns the widget a taskbar widget is.
    fn widget(&self, widget: TaskbarWidget) -> Widget<TaskbarHit> {
        match widget {
            TaskbarWidget::WorkspacePrevious => {
                Widget::glyph(ThemeElement::TaskbarButton, Glyph::ArrowLeft)
                    .with_button(TaskbarHit::WorkspacePrevious)
            }
            TaskbarWidget::WorkspaceNext => {
                Widget::glyph(ThemeElement::TaskbarButton, Glyph::ArrowRight)
                    .with_button(TaskbarHit::WorkspaceNext)
            }
            TaskbarWidget::WorkspaceName => {
                Widget::text(ThemeElement::TaskbarWorkspace, self.workspace_name.clone())
                    .with_hit(TaskbarHit::WorkspaceName)
            }
            TaskbarWidget::WindowList => Widget::row(
                ThemeElement::WindowListEmpty,
                self.entries
                    .iter()
                    .map(|entry| entry.widget(self.entry_max_width))
                    .collect(),
            ),
            TaskbarWidget::Clock => Widget::text(ThemeElement::TaskbarClock, self.clock.clone())
                .with_hit(TaskbarHit::Clock),
            TaskbarWidget::Tray => self.tray_widget(),
        }
    }

    /// Returns the tray: a slot per item, drawn on the taskbar's own
    /// background, as wide as its items need.
    fn tray_widget(&self) -> Widget<TaskbarHit> {
        if self.tray.is_empty() {
            return Widget::fill(ThemeElement::Taskbar).with_size(SizePolicy::Fixed(0));
        }
        let margin = || {
            Widget::fill(ThemeElement::Taskbar).with_size(SizePolicy::Fixed(Self::TRAY_END_MARGIN))
        };
        let slots = self.tray.iter().map(|entry| {
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
        });
        let count = u32::try_from(self.tray.len()).unwrap_or(u32::MAX);
        let width = count
            .saturating_mul(self.tray_slot)
            .saturating_add(2 * Self::TRAY_END_MARGIN);
        Widget::row(
            ThemeElement::Taskbar,
            std::iter::once(margin())
                .chain(slots)
                .chain(std::iter::once(margin()))
                .collect(),
        )
        .with_size(SizePolicy::Fixed(width))
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
    /// Returns the entry's widget, at most `max_width` wide.
    fn widget(&self, max_width: u32) -> Widget<TaskbarHit> {
        let title = if self.is_minimized {
            format!("({})", self.title)
        } else {
            self.title.clone()
        };
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
        Taskbar::new(&settings).expect("enabled by default")
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
    fn a_taskbar_that_reserves_nothing_leaves_the_usable_area_alone() {
        let settings = TaskbarSettings {
            reserve_space: false,
            ..TaskbarSettings::default()
        };
        let bar = Taskbar::new(&settings).expect("enabled");
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
        let bar = Taskbar::new(&settings).expect("enabled");
        let area = bar.area_in(whole(), 20);
        assert_eq!((area.loc.x, area.size.w), (left, 960));
    }

    #[test]
    fn an_auto_hiding_taskbar_starts_hidden_and_reserves_nothing() {
        let settings = TaskbarSettings {
            auto_hide: true,
            ..TaskbarSettings::default()
        };
        let bar = Taskbar::new(&settings).expect("enabled");
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
        let mut bar = Taskbar::new(&settings).expect("enabled");
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
        assert!(Taskbar::new(&settings).is_none());
    }

    #[test]
    fn the_tray_is_a_slot_per_item_with_a_pixel_at_each_end() {
        let content = TaskbarContent {
            widgets: vec![TaskbarWidget::Tray],
            workspace_name: String::new(),
            entries: Vec::new(),
            entry_max_width: 128,
            clock: String::new(),
            tray: [1, 2]
                .map(|key| TrayEntry {
                    key: TrayItemKey(key),
                    icon: None,
                })
                .to_vec(),
            tray_slot: 20,
        };
        let tray = content.tray_widget();
        assert_eq!(
            tray.size,
            SizePolicy::Fixed(42),
            "two 20-pixel slots and two 1-pixel ends"
        );
        let vaht_ui::widget::WidgetKind::Row(children) = tray.kind else {
            panic!("the tray is a row");
        };
        let hits: Vec<_> = children.iter().map(|child| child.hit).collect();
        assert_eq!(
            hits,
            [
                None,
                Some(TaskbarHit::TrayItem(TrayItemKey(1))),
                Some(TaskbarHit::TrayItem(TrayItemKey(2))),
                None
            ]
        );
        assert!(!children[1].is_button, "an item acts as it is pressed");
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
