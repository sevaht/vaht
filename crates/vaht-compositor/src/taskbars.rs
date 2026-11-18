//! The taskbars: one on the main output (`[taskbar] output`, else the
//! primary one), or one on every output with `on_every_output` (02 §3).
//!
//! Each [`Taskbar`] is on one output, by connector, and lists every
//! output's windows or only its own (`windows`). A desktop has
//! one tray, shown on the main taskbar, and one clock alarm, which wakes
//! vaht to draw every taskbar's clock anew; both live here, so the hooks
//! that find them in the compositor find one of each.

use std::num::NonZeroUsize;

use jiff::Zoned;
use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Logical, Point};
use vaht_ui::{IconStore, Style, Typesetter};

use crate::Compositor;
use crate::bus::Bus;
use crate::config::{TaskbarSettings, TaskbarWidget};
use crate::resources::ResourceLedger;
use crate::taskbar::{Taskbar, TaskbarView};
use crate::tray::{Tray, TrayAction, TrayHooks, TrayItemKey};
use crate::wall_clock::{ClockHooks, WallClock, ZoneSources};
use crate::window_manager::{Area, WindowManager};

/// The taskbars, the tray and the clock alarm.
#[derive(Debug)]
pub(crate) struct Taskbars {
    /// What the settings say of taskbars.
    settings: TaskbarSettings,
    /// Each taskbar, the main one first.
    bars: Vec<Taskbar>,
    /// The tray, once started, if the main taskbar shows one.
    tray: Option<Tray<Compositor>>,
    /// Wakes vaht when a clock's text next changes.
    wall_clock: Option<WallClock<Compositor>>,
}

impl Taskbars {
    /// The taskbars `settings` describe, on no output yet.
    pub(crate) fn new(settings: &TaskbarSettings) -> Self {
        Self {
            settings: settings.clone(),
            bars: Vec::new(),
            tray: None,
            wall_clock: None,
        }
    }

    /// Takes `settings`, making the taskbars anew if they changed; the
    /// tray and clock alarm stay.
    pub(crate) fn apply(&mut self, settings: &TaskbarSettings) {
        if *settings != self.settings {
            settings.clone_into(&mut self.settings);
            self.bars.clear();
        }
    }

    /// Follows the outputs on: `main` has the main taskbar, and with
    /// `on_every_output` each of `others` one too. Taskbars on outputs
    /// still there are kept; returns whether any came or went.
    pub(crate) fn follow_outputs<'name>(
        &mut self,
        main: &'name str,
        others: impl IntoIterator<Item = &'name str>,
    ) -> bool {
        if !self.settings.enabled {
            let had_any = !self.bars.is_empty();
            self.bars.clear();
            return had_any;
        }
        let mut wanted = vec![main];
        if self.settings.on_every_output {
            wanted.extend(others.into_iter().filter(|name| *name != main));
        }
        let is_same = self.bars.len() == wanted.len()
            && self
                .bars
                .iter()
                .zip(&wanted)
                .all(|(bar, name)| bar.output() == *name);
        if is_same {
            return false;
        }
        let mut kept = std::mem::take(&mut self.bars);
        for (index, name) in wanted.iter().enumerate() {
            let is_main = index == 0;
            // The main taskbar alone has the tray, so one that becomes or
            // stops being main is made anew.
            let reused = kept
                .iter()
                .position(|bar| bar.output() == *name && (index == 0) == bar.has_tray())
                .map(|position| kept.remove(position));
            if let Some(bar) = reused.or_else(|| Taskbar::new(&self.settings, name, is_main)) {
                self.bars.push(bar);
            }
        }
        true
    }

    /// Brings each taskbar up to date with `state` (the windows and the
    /// settings) and paints it, as `style` styles it, on its output, whose
    /// area `whole_of` gives by name. Returns whether any changed height,
    /// so windows keep off it anew.
    pub(crate) fn prepare(
        &mut self,
        whole_of: impl Fn(&str) -> Area,
        state: (&WindowManager, &TaskbarSettings),
        style: &Style,
        tools: (&mut Typesetter, &mut IconStore),
    ) -> bool {
        let (window_manager, settings) = state;
        let (typesetter, icons) = tools;
        let now = Zoned::now();
        let Self { bars, tray, .. } = self;
        let mut has_resized = false;
        for bar in bars {
            let view = TaskbarView {
                window_manager,
                settings,
                whole: whole_of(bar.output()),
                now: &now,
                tray: tray.as_ref(),
            };
            has_resized |= bar.prepare(view, style, typesetter, icons);
        }
        has_resized
    }

    /// Returns the main taskbar, if there is one.
    pub(crate) fn main(&self) -> Option<&Taskbar> {
        self.bars.first()
    }

    /// Returns every taskbar, the main one first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Taskbar> {
        self.bars.iter()
    }

    /// Returns every taskbar to change.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Taskbar> {
        self.bars.iter_mut()
    }

    /// Returns the taskbar on the output `name`, if there is one.
    pub(crate) fn on_output(&self, name: &str) -> Option<&Taskbar> {
        self.bars.iter().find(|bar| bar.output() == name)
    }

    /// Returns the taskbar on the output `name` to change.
    pub(crate) fn on_output_mut(&mut self, name: &str) -> Option<&mut Taskbar> {
        self.bars.iter_mut().find(|bar| bar.output() == name)
    }

    /// Returns the shown taskbar covering global `point`, if any.
    pub(crate) fn at(&self, point: Point<f64, Logical>) -> Option<&Taskbar> {
        self.bars
            .iter()
            .find(|bar| bar.is_shown() && bar.covers(point))
    }

    /// Returns the shown taskbar covering global `point` to change.
    pub(crate) fn at_mut(&mut self, point: Point<f64, Logical>) -> Option<&mut Taskbar> {
        self.bars
            .iter_mut()
            .find(|bar| bar.is_shown() && bar.covers(point))
    }

    /// Returns whether there is a taskbar at all.
    pub(crate) fn is_enabled(&self) -> bool {
        self.settings.enabled
    }

    /// Turns every taskbar off (`toggle-taskbar`), or on again.
    pub(crate) fn toggle(&mut self) {
        for bar in &mut self.bars {
            bar.toggle();
        }
    }

    /// Starts the tray, if the main taskbar shows one and it has not
    /// started.
    pub(crate) fn start_tray(
        &mut self,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        let shows_tray =
            self.settings.enabled && self.settings.widgets.contains(&TaskbarWidget::Tray);
        if !shows_tray || self.tray.is_some() {
            return;
        }
        let (handle, ledger) = event_loop;
        let hooks = TrayHooks {
            on_change: Compositor::schedule_render,
            on_menu: Compositor::show_tray_menu,
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

    /// Tells the tray item `key` that the entry `id` of its menu was chosen.
    pub(crate) fn choose_tray_menu_entry(&self, key: TrayItemKey, id: i32) {
        if let Some(tray) = &self.tray {
            tray.choose_menu_entry(key, id);
        }
    }

    /// Returns the `number`th tray item, from 1 at the left, in the order
    /// the settings pin, and the global point at its middle on the main
    /// taskbar.
    pub(crate) fn tray_item(
        &self,
        number: NonZeroUsize,
    ) -> Option<(TrayItemKey, Point<f64, Logical>)> {
        let tray = self.tray.as_ref()?;
        let key = tray
            .ordered(&self.settings.tray.pin_left, &self.settings.tray.pin_right)
            .get(number.get() - 1)?
            .key;
        Some((key, self.main()?.tray_item_middle(key)))
    }

    /// Returns the process of the tray item `key`, if known.
    pub(crate) fn tray_item_pid(&self, key: TrayItemKey) -> Option<u32> {
        self.tray.as_ref()?.pid_of(key)
    }

    /// Wakes vaht to draw the clocks anew when their text next changes,
    /// after `now`: each second if a shown one shows seconds, else each
    /// minute; not at all while none shows.
    pub(crate) fn schedule_clock(
        &mut self,
        now: &Zoned,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        let clocks: Vec<bool> = self
            .bars
            .iter()
            .filter(|bar| bar.is_shown())
            .filter_map(Taskbar::clock)
            .collect();
        let has_clock = self.bars.iter().any(|bar| bar.clock().is_some());
        if !has_clock {
            self.wall_clock = None;
            return;
        }
        if self.wall_clock.is_none() {
            let (handle, ledger) = event_loop;
            let hooks = ClockHooks {
                on_change: Compositor::show_time_change,
                find: Self::wall_clock_of,
            };
            match WallClock::start(handle, ledger, &ZoneSources::system(), hooks) {
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
        let armed = if clocks.is_empty() {
            wall_clock.disarm()
        } else {
            let shows_seconds = clocks.iter().any(|seconds| *seconds);
            wall_clock.ring_at(Taskbar::next_clock_change(now, shows_seconds))
        };
        if let Err(error) = armed {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not schedule the clock"
            );
        }
    }

    /// Returns the tray in `compositor`, while it has one.
    fn tray_of(compositor: &mut Compositor) -> Option<&mut Tray<Compositor>> {
        compositor.taskbars.tray.as_mut()
    }

    /// Returns the clock alarm in `compositor`, while it has one.
    fn wall_clock_of(compositor: &mut Compositor) -> Option<&mut WallClock<Compositor>> {
        compositor.taskbars.wall_clock.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_taskbar_is_on_the_main_output_unless_every_output_has_one() {
        let mut taskbars = Taskbars::new(&TaskbarSettings::default());
        assert!(taskbars.follow_outputs("A", ["A", "B"]));
        let outputs = |taskbars: &Taskbars| {
            taskbars
                .iter()
                .map(|bar| bar.output().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(outputs(&taskbars), ["A"]);
        assert!(!taskbars.follow_outputs("A", ["A", "B"]), "nothing changed");

        taskbars.apply(&TaskbarSettings {
            on_every_output: true,
            ..TaskbarSettings::default()
        });
        taskbars.follow_outputs("B", ["A", "B"]);
        assert_eq!(outputs(&taskbars), ["B", "A"], "the main one first");
        assert!(taskbars.main().is_some_and(Taskbar::has_tray));
        assert!(
            !taskbars.on_output("A").is_some_and(Taskbar::has_tray),
            "one tray"
        );

        let mut off = Taskbars::new(&TaskbarSettings {
            enabled: false,
            ..TaskbarSettings::default()
        });
        off.follow_outputs("A", ["A"]);
        assert!(off.main().is_none());
    }
}
