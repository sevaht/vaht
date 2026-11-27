//! Menus: the root menu, the window menu and the others, shown by vaht
//! (01 §6.4).
//!
//! What a menu shows is a [`MenuModel`] ([`model`]), built from
//! `menus.toml` as it opens. How it is shown is [`MenuStack`]'s: the menus
//! open now, each a panel drawn with the style ([`content`]). The first is
//! opened by an action; each further one is a submenu of the item highlighted
//! in the one before.
//!
//! The pointer highlights the item under it, and resting on a submenu's
//! item for a moment opens the submenu (a one-shot timer, `submenu_delay_ms`).
//! Releasing a button on an item chooses it; a press outside every menu
//! closes them, and goes on to what is under it, as in Fluxbox. While menus
//! are open, keys are theirs: Up and Down move the highlight, Right or Enter
//! open a submenu or choose, Left closes the last menu, Escape closes all.

mod content;
mod model;
mod pipe;

use std::time::Duration;

use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::input::keyboard::xkb::Keysym;
use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Logical, Point, Scale};
use vaht_ui::{Panel, Style, Typesetter};

pub(crate) use self::content::{MenuContent, MenuGeometry};
pub(crate) use self::model::{MenuBuilder, MenuChoice, MenuItem, MenuModel, OutputChoices};
pub(crate) use self::pipe::{PipeRead, parse_printed};
use crate::Compositor;
use crate::config::{MenuRequest, MenuSearch};
use crate::event_source::Timer;
use crate::resources::ResourceLedger;
use crate::scene::{MenuTexture, SceneElement};
use crate::window_manager::{Area, WindowId};

/// A key a menu acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuKey {
    /// Highlights the item above.
    Up,
    /// Highlights the item below.
    Down,
    /// Closes the last menu.
    Back,
    /// Opens the highlighted submenu.
    Forward,
    /// Typed a letter: the search goes on with it.
    Type(char),
    /// Backspace: the search goes back a letter.
    Erase,
    /// Opens the highlighted submenu, or chooses the highlighted item.
    Choose,
    /// Closes every menu.
    Close,
    /// Any other key, which menus take and ignore.
    Other,
}

impl MenuKey {
    /// Returns what the key `keysym` means to a menu.
    pub(crate) fn of(keysym: Keysym) -> Self {
        match keysym {
            Keysym::Up | Keysym::KP_Up => Self::Up,
            Keysym::Down | Keysym::KP_Down => Self::Down,
            Keysym::Left | Keysym::KP_Left => Self::Back,
            Keysym::Right | Keysym::KP_Right => Self::Forward,
            Keysym::Return | Keysym::KP_Enter | Keysym::space => Self::Choose,
            Keysym::Escape => Self::Close,
            Keysym::BackSpace => Self::Erase,
            _ => keysym
                .key_char()
                .filter(|character| !character.is_control())
                .map_or(Self::Other, Self::Type),
        }
    }
}

/// The menus open now, the first opened by an action and each further one
/// a submenu of the item highlighted in the one before.
#[derive(Debug, Default)]
pub(crate) struct MenuStack {
    /// The open menus, first to last.
    menus: Vec<OpenMenu>,
    /// Where menus must stay: the output.
    bounds: Area,
    /// Opens the highlighted submenu, once the pointer has rested on it.
    submenu_timer: Option<Timer<Compositor>>,
    /// The menu and item a button was pressed on, which its release
    /// chooses.
    pressed: Option<(usize, usize)>,
    /// The action that opened the menus, if one did.
    opened_by: Option<MenuRequest>,
    /// Where the first menu was asked to open, to open it again there.
    anchor: Point<i32, Logical>,
    /// A pipe menu waiting for its command's output: its number, its
    /// level, and its item in the menu before; and the command, until the
    /// compositor takes it to run.
    pipe: Option<PipeWait>,
    /// Numbers pipe menus as they open.
    pipe_count: u64,
    /// The output of the pipe menu's command, as it is read.
    pipe_read: Option<PipeRead>,
    /// The window the menus are about, if any.
    opened_for: Option<WindowId>,
    /// How typing finds an item.
    search_mode: MenuSearch,
    /// What was typed in the last menu, to find an item by.
    search: String,
    /// A menu's title held by the pointer, to move the menu by.
    dragging: Option<Drag>,
}

/// A menu being moved by its title.
#[derive(Debug, Clone, Copy)]
struct Drag {
    /// The menu's level in the stack.
    level: usize,
    /// Where in the menu it was taken, from its top-left corner.
    grab: Point<i32, Logical>,
}

/// A pipe menu open, waiting for what its command prints.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PipeWait {
    /// Which opening it is: a later one replaces it.
    number: u64,
    /// The menu's level in the stack.
    level: usize,
    /// Its item in the menu before.
    item: usize,
    /// The command line, until the compositor takes it to run.
    command: Option<String>,
}

/// How the menus were open: enough to open them again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuSnapshot {
    /// What opened them.
    pub(crate) request: MenuRequest,
    /// The window they are about, if any.
    pub(crate) window: Option<WindowId>,
    /// Where the first one opened.
    pub(crate) at: Point<i32, Logical>,
    /// The item highlighted in each menu, first to last.
    pub(crate) path: Vec<Option<usize>>,
    /// Where each menu was moved to by its title, if it was.
    pub(crate) moved: Vec<Option<Point<i32, Logical>>>,
    /// What the menus were kept within: the usable part of the output they
    /// were last on.
    pub(crate) bounds: Area,
}

/// One open menu.
#[derive(Debug)]
struct OpenMenu {
    /// What it shows, laid out and painted.
    panel: Panel<MenuContent>,
    /// Where its parts go.
    geometry: MenuGeometry,
    /// Its buffers.
    texture: MenuTexture,
    /// Where it was moved to by its title, if it was.
    moved: Option<Point<i32, Logical>>,
}

impl MenuStack {
    /// Returns whether any menu is open.
    pub(crate) const fn is_open(&self) -> bool {
        !self.menus.is_empty()
    }

    /// Returns whether a menu covers global `point`.
    pub(crate) fn covers(&self, point: Point<f64, Logical>) -> bool {
        self.menus.iter().any(|menu| menu.geometry.covers(point))
    }

    /// Opens `model`, closing any menu open, its top-left corner at `at`
    /// unless that would put it off `bounds` (the output), as `style`
    /// styles menus.
    pub(crate) fn open(
        &mut self,
        model: MenuModel,
        at: Point<i32, Logical>,
        bounds: Area,
        style: (&Style, &mut Typesetter),
    ) {
        self.close();
        self.bounds = bounds;
        self.anchor = at;
        self.push(model, at, None, style);
    }

    /// Returns no menus, typing finding items as `mode` says.
    pub(crate) fn with_search(mode: MenuSearch) -> Self {
        Self {
            search_mode: mode,
            ..Self::default()
        }
    }

    /// Makes typing find items as `mode` says.
    pub(crate) const fn set_search(&mut self, mode: MenuSearch) {
        self.search_mode = mode;
    }

    /// Notes that `request` opened the menus.
    pub(crate) fn mark_opened_by(&mut self, request: MenuRequest, window: Option<WindowId>) {
        self.opened_by = Some(request);
        self.opened_for = window;
    }

    /// Returns the window the menus are about, if any.
    pub(crate) const fn opened_for(&self) -> Option<WindowId> {
        self.opened_for
    }

    /// Reads the pipe menu's command's output through `read`.
    pub(crate) fn read_pipe(&mut self, read: PipeRead) {
        self.pipe_read = Some(read);
    }

    /// Keeps `bytes`, more of pipe menu `number`'s output; returns whether
    /// to read on.
    pub(crate) fn pipe_output(&mut self, number: u64, bytes: &[u8]) -> bool {
        self.pipe_read
            .as_mut()
            .is_some_and(|read| read.take_in(number, bytes))
    }

    /// Returns what pipe menu `number`'s command printed, once it ended.
    pub(crate) fn take_pipe_output(&mut self, number: u64) -> Option<Result<Vec<u8>, String>> {
        self.pipe_read.as_mut()?.take_output(number)
    }

    /// Returns whether the menus open now were opened by `request`.
    pub(crate) fn is_opened_by(&self, request: &MenuRequest) -> bool {
        self.is_open() && self.opened_by.as_ref() == Some(request)
    }

    /// Closes every menu.
    pub(crate) fn close(&mut self) {
        self.pipe = None;
        self.pipe_read = None;
        self.opened_by = None;
        self.search.clear();
        self.menus.clear();
        self.submenu_timer = None;
        self.pressed = None;
    }

    /// Highlights the item under global `point`, closes submenus the pointer
    /// left behind, and starts the wait before a highlighted submenu opens;
    /// returns whether anything shown changed.
    pub(crate) fn follow_pointer(
        &mut self,
        point: Point<f64, Logical>,
        delay: Duration,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) -> bool {
        if self.dragging.is_some() {
            return false;
        }
        let Some((level, Some(item))) = self.item_at(point) else {
            return false;
        };
        let menu = &mut self.menus[level];
        if menu.panel.content().hovered() == Some(item) {
            return false;
        }
        let changed = menu
            .panel
            .update_if(|content| content.set_hovered(Some(item)));
        // A submenu of another item closes once the pointer rests here.
        self.menus.truncate(level + 1);
        self.submenu_timer = None;
        let is_submenu = matches!(
            self.menus[level].panel.content().model().items.get(item),
            Some(MenuItem::Submenu { .. })
        );
        if is_submenu {
            let (handle, ledger) = event_loop;
            match Timer::once(handle, ledger, delay, Compositor::open_highlighted_submenu) {
                Ok(timer) => self.submenu_timer = Some(timer),
                Err(error) => tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "a submenu cannot wait to open"
                ),
            }
        }
        changed
    }

    /// Notes a button pressed at global `point`, on a menu; returns
    /// whether a menu was there. A press elsewhere closes every menu.
    pub(crate) fn press(&mut self, point: Point<f64, Logical>) -> bool {
        if !self.covers(point) {
            self.close();
            return false;
        }
        // A title is held to move its menu by, as in Fluxbox.
        if let Some(level) = self.title_at(point) {
            let corner = self.menus[level].geometry.outer.loc;
            self.dragging = Some(Drag {
                level,
                grab: point.to_i32_floor() - corner,
            });
            self.pressed = None;
            return true;
        }
        self.pressed = self
            .item_at(point)
            .and_then(|(level, item)| Some((level, item?)));
        true
    }

    /// Lets go of the button with the pointer at global `point`: an item
    /// there opens its submenu, or is chosen, which closes every menu and
    /// returns the choice; with `keep_open`, the menus stay.
    pub(crate) fn release(
        &mut self,
        point: Point<f64, Logical>,
        style: (&Style, &mut Typesetter),
        keep_open: bool,
    ) -> Option<MenuChoice> {
        self.pressed = None;
        // Letting go of a title ends the move and chooses nothing.
        if self.dragging.take().is_some() {
            return None;
        }
        let (level, item) = self.item_at(point)?;
        let item = item?;
        self.menus[level]
            .panel
            .update_if(|content| content.set_hovered(Some(item)));
        self.menus.truncate(level + 1);
        self.choose_highlighted(style, keep_open)
    }

    /// Returns how the menus are open now, to open them again as they were
    /// (after what a choice changed): what opened them, for which window,
    /// where, and the item highlighted in each menu.
    pub(crate) fn snapshot(&self) -> Option<MenuSnapshot> {
        Some(MenuSnapshot {
            request: self.opened_by.clone()?,
            window: self.opened_for,
            at: self.anchor,
            path: self
                .menus
                .iter()
                .map(|menu| menu.panel.content().hovered())
                .collect(),
            moved: self.menus.iter().map(|menu| menu.moved).collect(),
            bounds: self.bounds,
        })
    }

    /// Returns whether a menu's title is held to move it.
    pub(crate) const fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    /// Returns the level of the menu whose title is at global `point`.
    fn title_at(&self, point: Point<f64, Logical>) -> Option<usize> {
        let (level, menu) = self
            .menus
            .iter()
            .enumerate()
            .rev()
            .find(|(_, menu)| menu.geometry.covers(point))?;
        let line = menu.geometry.lines[4];
        let is_titled = line.size.w > 0;
        let pixel: Point<i32, Logical> = point.to_i32_floor();
        (is_titled && pixel.y < line.loc.y).then_some(level)
    }

    /// Moves the menu being dragged by its title so the pointer, at global
    /// `point`, keeps its place in it, within `bounds`, the usable part of the
    /// output the pointer is on (so a menu is carried from one output to
    /// another, and the submenus opened after fit where it is now); returns
    /// whether it moved.
    pub(crate) fn drag_to(&mut self, point: Point<f64, Logical>, bounds: Area) -> bool {
        let Some(drag) = self.dragging else {
            return false;
        };
        self.bounds = bounds;
        let point: Point<i32, Logical> = point.to_i32_floor();
        self.move_menu(drag.level, point - drag.grab, true)
    }

    /// Puts the corner of the menu of `level` at `wanted`, kept within the
    /// bounds if `is_fitted`; returns whether it moved.
    fn move_menu(&mut self, level: usize, wanted: Point<i32, Logical>, is_fitted: bool) -> bool {
        let bounds = self.bounds;
        let Some(menu) = self.menus.get_mut(level) else {
            return false;
        };
        let outer = menu.geometry.outer;
        let origin = if is_fitted {
            Self::fitted(wanted, (outer.size.w, outer.size.h), bounds)
        } else {
            wanted
        };
        if origin == outer.loc {
            return false;
        }
        let panel = menu.geometry.panel.size;
        let size = vaht_ui::Size::new(
            u32::try_from(panel.w).unwrap_or(0),
            u32::try_from(panel.h).unwrap_or(0),
        );
        menu.geometry = MenuGeometry::at(origin, size, menu.panel.content());
        menu.moved = Some(origin);
        true
    }

    /// Highlights the items of `path`, opening the submenu of each but the
    /// last: the menus as `snapshot` found them.
    pub(crate) fn restore(
        &mut self,
        path: &[Option<usize>],
        moved: &[Option<Point<i32, Logical>>],
        bounds: Area,
        style: (&Style, &mut Typesetter),
    ) {
        let (style, typesetter) = style;
        self.bounds = bounds;
        for (position, item) in path.iter().enumerate() {
            let Some(menu) = self.menus.last_mut() else {
                return;
            };
            menu.panel.update_if(|content| content.set_hovered(*item));
            // A menu moved by its title stays where it was put, and its
            // submenu opens beside it there.
            if let Some(Some(origin)) = moved.get(position) {
                self.move_menu(position, *origin, false);
            }
            if position + 1 < path.len() {
                self.open_highlighted_submenu((style, &mut *typesetter));
            }
        }
    }

    /// Acts on `key`; returns the choice it made, if it chose an item.
    pub(crate) fn key(
        &mut self,
        key: MenuKey,
        style: (&Style, &mut Typesetter),
    ) -> Option<MenuChoice> {
        if !matches!(key, MenuKey::Type(_) | MenuKey::Erase) {
            self.search.clear();
        }
        match key {
            MenuKey::Type(character) => {
                self.search_for(character);
                None
            }
            MenuKey::Erase => {
                self.search.pop();
                self.highlight_found();
                None
            }
            MenuKey::Up | MenuKey::Down => {
                let step = if key == MenuKey::Up { -1 } else { 1 };
                if let Some(menu) = self.menus.last_mut() {
                    let next = Self::next_selectable(menu.panel.content(), step);
                    menu.panel.update_if(|content| content.set_hovered(next));
                }
                None
            }
            MenuKey::Back => {
                if self.menus.len() > 1 {
                    self.menus.pop();
                }
                None
            }
            MenuKey::Forward => {
                self.open_highlighted_submenu(style);
                None
            }
            MenuKey::Choose => self.choose_highlighted(style, false),
            MenuKey::Close => {
                self.close();
                None
            }
            MenuKey::Other => None,
        }
    }

    /// Opens the submenu of the item highlighted in the last menu, if it is
    /// a submenu's.
    pub(crate) fn open_highlighted_submenu(&mut self, style: (&Style, &mut Typesetter)) {
        self.submenu_timer = None;
        let Some(last) = self.menus.last() else {
            return;
        };
        let Some(index) = last.panel.content().hovered() else {
            return;
        };
        let Some(MenuItem::Submenu { menu, pipe, .. }) =
            last.panel.content().model().items.get(index)
        else {
            return;
        };
        let (model, pipe) = (menu.clone(), pipe.clone());
        let level = self.menus.len() - 1;
        self.pipe = None;
        self.push(model, Point::default(), Some((level, index)), style);
        if let Some(command) = pipe {
            self.pipe_count += 1;
            self.pipe = Some(PipeWait {
                number: self.pipe_count,
                level: level + 1,
                item: index,
                command: Some(command),
            });
        }
    }

    /// Takes the command of the pipe menu just opened, to run, with the
    /// number to fill the menu by.
    pub(crate) fn take_pipe_command(&mut self) -> Option<(u64, String)> {
        let pipe = self.pipe.as_mut()?;
        Some((pipe.number, pipe.command.take()?))
    }

    /// Fills the pipe menu `number` with `model`, what its command
    /// printed, if it is still open; returns whether it was.
    pub(crate) fn fill_pipe(
        &mut self,
        number: u64,
        model: MenuModel,
        style: (&Style, &mut Typesetter),
    ) -> bool {
        let Some(pipe) = self.pipe.take_if(|pipe| pipe.number == number) else {
            return false;
        };
        let is_still_open = self.menus.len() > pipe.level
            && self.menus[pipe.level - 1].panel.content().hovered() == Some(pipe.item)
            && matches!(
                self.menus[pipe.level - 1]
                    .panel
                    .content()
                    .model()
                    .items
                    .get(pipe.item),
                Some(MenuItem::Submenu { pipe: Some(_), .. })
            );
        if !is_still_open {
            return false;
        }
        self.menus.truncate(pipe.level);
        self.push(
            model,
            Point::default(),
            Some((pipe.level - 1, pipe.item)),
            style,
        );
        true
    }

    /// Paints every menu that changed, as `style` styles menus.
    pub(crate) fn prepare(&mut self, style: &Style, typesetter: &mut Typesetter) {
        for menu in &mut self.menus {
            menu.panel.set_corners(menu.geometry.panel_corners());
            menu.panel.paint(style, typesetter);
            menu.texture.update(&menu.panel, &menu.geometry, style);
        }
    }

    /// Returns the elements that draw the menus, topmost first.
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.menus
            .iter()
            .rev()
            .flat_map(|menu| {
                menu.texture
                    .render_elements(&menu.geometry, renderer, scale)
            })
            .collect()
    }

    /// Opens `model` after the open menus: at `at`, or for a submenu beside
    /// item `index` of menu `level`.
    fn push(
        &mut self,
        model: MenuModel,
        at: Point<i32, Logical>,
        beside: Option<(usize, usize)>,
        style: (&Style, &mut Typesetter),
    ) {
        let (style, typesetter) = style;
        let content = MenuContent::new(model, style.metrics().menu);
        let size = content.size(style, typesetter);
        let probe = MenuGeometry::at(Point::default(), size, &content);
        let (width, height) = (probe.outer.size.w, probe.outer.size.h);
        let border = to_i32(style.metrics().menu.border_width);
        let wanted = beside.map_or(at, |(level, index)| {
            // The submenu's first item lines up with its item in the parent,
            // their outlines overlapping.
            let parent = &self.menus[level];
            let row = parent.geometry.panel.loc.y + to_i32(parent.panel.content().row_top(index));
            let first_row = border + to_i32(content.row_top(0));
            let right = parent.geometry.outer.loc.x + parent.geometry.outer.size.w - border;
            let x = if right + width <= self.bounds.loc.x + self.bounds.size.w {
                right
            } else {
                parent.geometry.outer.loc.x - width + border
            };
            (x, row - first_row).into()
        });
        let origin = Self::fitted(wanted, (width, height), self.bounds);
        let geometry = MenuGeometry::at(origin, size, &content);
        self.menus.push(OpenMenu {
            panel: Panel::new(content, size),
            geometry,
            texture: MenuTexture::new(),
            moved: None,
        });
    }

    /// Returns `wanted` moved as little as keeps a menu of `size` on
    /// `bounds`.
    fn fitted(wanted: Point<i32, Logical>, size: (i32, i32), bounds: Area) -> Point<i32, Logical> {
        let (width, height) = size;
        let fit = |position: i32, length: i32, start: i32, span: i32| {
            position.min(start + span - length).max(start)
        };
        (
            fit(wanted.x, width, bounds.loc.x, bounds.size.w),
            fit(wanted.y, height, bounds.loc.y, bounds.size.h),
        )
            .into()
    }

    /// Opens the highlighted item's submenu, or chooses it, closing every
    /// menu (unless `keep_open`) and returning the choice.
    fn choose_highlighted(
        &mut self,
        style: (&Style, &mut Typesetter),
        keep_open: bool,
    ) -> Option<MenuChoice> {
        let last = self.menus.last()?;
        let index = last.panel.content().hovered()?;
        match last.panel.content().model().items.get(index)? {
            MenuItem::Submenu { .. } => {
                self.open_highlighted_submenu(style);
                None
            }
            MenuItem::Choice { choice, .. } => {
                let choice = choice.clone();
                if !keep_open && !choice.keeps_menus_open() {
                    self.close();
                }
                Some(choice)
            }
            MenuItem::Inert { .. } | MenuItem::Separator => None,
        }
    }

    /// Returns the menu at global `point` (the last one there), with the
    /// item there, if any.
    fn item_at(&self, point: Point<f64, Logical>) -> Option<(usize, Option<usize>)> {
        self.menus
            .iter()
            .enumerate()
            .rev()
            .find(|(_, menu)| menu.geometry.covers(point))
            .map(|(level, menu)| (level, menu.panel.hit_at(menu.geometry.panel_point(point))))
    }

    /// Goes on with the search with `character`, if an item of the last
    /// menu is found by it; as in Fluxbox, a letter finding nothing is not
    /// kept.
    fn search_for(&mut self, character: char) {
        self.search.push(character);
        if !self.highlight_found() {
            self.search.pop();
        }
    }

    /// Highlights the first selectable item of the last menu the search
    /// finds; returns whether one was found. An empty search finds nothing
    /// and changes nothing.
    fn highlight_found(&mut self) -> bool {
        let (mode, typed) = (self.search_mode, self.search.as_str());
        let Some(menu) = self.menus.last_mut() else {
            return false;
        };
        if typed.is_empty() {
            return false;
        }
        let found = menu.panel.content().model().items.iter().position(|item| {
            item.is_selectable() && item.label().is_some_and(|label| mode.finds(label, typed))
        });
        if found.is_some() {
            menu.panel.update_if(|content| content.set_hovered(found));
        }
        found.is_some()
    }

    /// Returns the selectable item `step` (1 down, -1 up) from the
    /// highlighted one in `content`, wrapping around; the first or last when
    /// none is highlighted.
    fn next_selectable(content: &MenuContent, step: isize) -> Option<usize> {
        let items = &content.model().items;
        let count = items.len();
        if count == 0 {
            return None;
        }
        let start = content
            .hovered()
            .unwrap_or(if step > 0 { count - 1 } else { 0 });
        (1..=count)
            .map(|offset| {
                let offset = isize::try_from(offset).unwrap_or(0) * step;
                let count = isize::try_from(count).unwrap_or(1);
                let index = (isize::try_from(start).unwrap_or(0) + offset).rem_euclid(count);
                usize::try_from(index).unwrap_or(0)
            })
            .find(|index| items[*index].is_selectable())
    }
}

/// Converts a style's pixel extent to a coordinate.
fn to_i32(extent: u32) -> i32 {
    i32::try_from(extent).expect("menu sizes are far below i32::MAX")
}

#[cfg(test)]
mod tests {
    use smithay::utils::Rectangle;

    use super::*;
    use crate::config::{Action, Command, Config, Menus};
    use crate::window_manager::{OutputArea, OutputAreas, WindowManager};

    /// A 1920 × 1080 output.
    fn bounds() -> Area {
        Rectangle::from_size((1920, 1080).into())
    }

    /// An item choosing to run `action`.
    fn choice(label: &str, action: Action) -> MenuItem {
        MenuItem::Choice {
            label: label.to_owned(),
            choice: MenuChoice::Run {
                command: Command::Run(action),
                window: None,
            },
            mark: None,
            icon: None,
        }
    }

    /// A menu: reload, a separator, a submenu holding exit, and inert text.
    fn model() -> MenuModel {
        MenuModel {
            title: Some("vaht".to_owned()),
            items: vec![
                choice("Reload", Action::Reload),
                MenuItem::Separator,
                MenuItem::Submenu {
                    label: "More".to_owned(),
                    menu: MenuModel {
                        title: None,
                        items: vec![choice("Exit", Action::Exit)],
                    },
                    icon: None,
                    pipe: None,
                },
                MenuItem::Inert {
                    label: "words".to_owned(),
                },
            ],
        }
    }

    /// A style and a typesetter to lay menus out with.
    fn style() -> (Style, Typesetter) {
        (
            Style::built_in(),
            Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH),
        )
    }

    /// Returns the item highlighted in menu `level`.
    fn hovered(stack: &MenuStack, level: usize) -> Option<usize> {
        stack.menus[level].panel.content().hovered()
    }

    #[test]
    fn typing_highlights_the_item_found_and_keeps_only_letters_that_find_one() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::with_search(MenuSearch::ItemStart);
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        let mut key = |stack: &mut MenuStack, key| stack.key(key, (&style, &mut typesetter));
        key(&mut stack, MenuKey::Type('M'));
        assert_eq!(hovered(&stack, 0), Some(2), "More, in any letter case");
        key(&mut stack, MenuKey::Type('x'));
        assert_eq!(stack.search, "M", "x finds nothing, so it is not kept");
        key(&mut stack, MenuKey::Erase);
        key(&mut stack, MenuKey::Type('r'));
        assert_eq!(hovered(&stack, 0), Some(0), "Reload");
        key(&mut stack, MenuKey::Type('w'));
        assert_eq!(hovered(&stack, 0), Some(0), "inert words are never found");
        let mut anywhere = MenuStack::with_search(MenuSearch::Anywhere);
        anywhere.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        anywhere.key(MenuKey::Type('o'), (&style, &mut typesetter));
        assert_eq!(hovered(&anywhere, 0), Some(0), "Reload has an o");
    }

    #[test]
    fn keys_move_past_what_cannot_be_chosen_open_submenus_and_choose() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        assert!(stack.is_open());
        let mut key = |stack: &mut MenuStack, key| stack.key(key, (&style, &mut typesetter));
        assert_eq!(key(&mut stack, MenuKey::Down), None);
        assert_eq!(hovered(&stack, 0), Some(0), "the first item");
        key(&mut stack, MenuKey::Down);
        assert_eq!(hovered(&stack, 0), Some(2), "past the separator");
        key(&mut stack, MenuKey::Down);
        assert_eq!(hovered(&stack, 0), Some(0), "past the inert text, around");
        key(&mut stack, MenuKey::Up);
        assert_eq!(hovered(&stack, 0), Some(2));
        key(&mut stack, MenuKey::Forward);
        assert_eq!(stack.menus.len(), 2, "the submenu opened");
        key(&mut stack, MenuKey::Back);
        assert_eq!(stack.menus.len(), 1, "and closed");
        key(&mut stack, MenuKey::Choose);
        key(&mut stack, MenuKey::Down);
        let chosen = key(&mut stack, MenuKey::Choose);
        assert_eq!(
            chosen,
            Some(MenuChoice::Run {
                command: Command::Run(Action::Exit),
                window: None
            })
        );
        assert!(!stack.is_open(), "choosing closes every menu");
    }

    #[test]
    fn menus_stay_on_the_output_and_submenus_open_beside_their_item() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (1915, 1075).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        let outer = stack.menus[0].geometry.outer;
        assert!(
            bounds().contains_rect(outer),
            "{outer:?} is moved onto the output"
        );
        stack.key(MenuKey::Down, (&style, &mut typesetter));
        stack.key(MenuKey::Down, (&style, &mut typesetter));
        stack.key(MenuKey::Forward, (&style, &mut typesetter));
        let submenu = stack.menus[1].geometry.outer;
        assert!(
            submenu.loc.x + submenu.size.w <= outer.loc.x + 1,
            "no room on the right: it opens on the left: {submenu:?} of {outer:?}"
        );
        let parent_row = stack.menus[0].geometry.panel.loc.y
            + i32::try_from(stack.menus[0].panel.content().row_top(2)).expect("small");
        assert_eq!(
            stack.menus[1].geometry.panel.loc.y, parent_row,
            "its first item beside its item"
        );
    }

    #[test]
    fn a_press_outside_closes_every_menu_and_one_inside_is_the_menus() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        let corner = stack.menus[0].geometry.panel.loc.to_f64();
        let inside: Point<f64, Logical> = (corner.x + 5.0, corner.y + 5.0).into();
        assert!(stack.press(inside));
        assert!(stack.is_open());
        assert!(!stack.press((1000.0, 1000.0).into()));
        assert!(!stack.is_open());
    }

    #[test]
    fn releasing_on_an_item_chooses_it() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        let menu = &stack.menus[0];
        let corner = menu.geometry.panel.loc.to_f64();
        let first_row: Point<f64, Logical> = (
            corner.x + 10.0,
            corner.y + f64::from(menu.panel.content().row_top(0)) + 5.0,
        )
            .into();
        // The panel lays out as it paints.
        stack.prepare(&style, &mut typesetter);
        let chosen = stack.release(first_row, (&style, &mut typesetter), false);
        assert_eq!(
            chosen,
            Some(MenuChoice::Run {
                command: Command::Run(Action::Reload),
                window: None
            })
        );
    }

    #[test]
    fn choosing_a_style_or_layout_leaves_the_menus_open_and_anything_else_closes_them() {
        let (style, mut typesetter) = style();
        for (action, stays_open) in [
            (Action::SetStyle("a".to_owned()), true),
            (Action::SetLayout("a".to_owned()), true),
            (Action::Reload, false),
        ] {
            let mut stack = MenuStack::default();
            let menu = MenuModel {
                title: None,
                items: vec![choice("Item", action.clone())],
            };
            stack.open(menu, (100, 100).into(), bounds(), (&style, &mut typesetter));
            stack.key(MenuKey::Down, (&style, &mut typesetter));
            let chosen = stack.key(MenuKey::Choose, (&style, &mut typesetter));
            assert!(chosen.is_some(), "{action:?} is chosen");
            assert_eq!(stack.is_open(), stays_open, "{action:?}");
        }
    }

    #[test]
    fn choosing_with_keep_open_leaves_the_menus_as_they_were() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        let at = (100, 100).into();
        stack.open(model(), at, bounds(), (&style, &mut typesetter));
        stack.mark_opened_by(MenuRequest::Root, None);
        stack.prepare(&style, &mut typesetter);
        let menu = &stack.menus[0];
        let corner = menu.geometry.panel.loc.to_f64();
        let first_row: Point<f64, Logical> = (
            corner.x + 10.0,
            corner.y + f64::from(menu.panel.content().row_top(0)) + 5.0,
        )
            .into();
        let chosen = stack.release(first_row, (&style, &mut typesetter), true);
        assert!(chosen.is_some(), "the item is chosen");
        assert!(stack.is_open(), "and the menus stay");
        let snapshot = stack.snapshot().expect("opened by a request");
        assert_eq!(snapshot.at, at);
        assert_eq!(
            snapshot.path,
            [Some(0)],
            "the chosen item stays highlighted"
        );
    }

    #[test]
    fn restoring_a_snapshot_reopens_the_submenus_it_had_open() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        stack.mark_opened_by(MenuRequest::Root, None);
        stack.restore(
            &[Some(2), Some(0)],
            &[],
            bounds(),
            (&style, &mut typesetter),
        );
        assert_eq!(stack.menus.len(), 2, "the submenu is open");
        assert_eq!(
            stack.snapshot().expect("a request").path,
            [Some(2), Some(0)]
        );
    }

    #[test]
    fn a_list_in_a_submenu_is_titled_with_its_items_label() {
        let config = Config::built_in();
        let output = OutputArea {
            whole: bounds(),
            usable: bounds(),
        };
        let manager = WindowManager::new(
            &config.settings,
            OutputAreas::new(output, Vec::new()),
            config.style.metrics(),
        );
        let root = MenuBuilder::new(&config.menus, &manager, None)
            .build(Menus::ROOT)
            .expect("built in");
        let titled: Vec<(&str, Option<&str>)> = root
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Submenu { label, menu, .. } => {
                    Some((label.as_str(), menu.title.as_deref()))
                }
                MenuItem::Choice { .. } | MenuItem::Inert { .. } | MenuItem::Separator => None,
            })
            .collect();
        assert!(!titled.is_empty(), "the root menu has lists in submenus");
        for (label, title) in titled {
            assert_eq!(title, Some(label), "the submenu {label} shows its name");
        }
    }

    #[test]
    fn the_built_in_menus_build_with_the_workspaces_listed() {
        let config = Config::built_in();
        let output = OutputArea {
            whole: bounds(),
            usable: bounds(),
        };
        let manager = WindowManager::new(
            &config.settings,
            OutputAreas::new(output, Vec::new()),
            config.style.metrics(),
        );
        let builder = MenuBuilder::new(&config.menus, &manager, None);
        let workspace = builder.build(Menus::WORKSPACE).expect("built in");
        let marks: Vec<Option<bool>> = workspace
            .items
            .iter()
            .take(4)
            .map(|item| match item {
                MenuItem::Choice { mark, .. } => *mark,
                MenuItem::Submenu { .. } | MenuItem::Inert { .. } | MenuItem::Separator => None,
            })
            .collect();
        assert_eq!(
            marks,
            [Some(true), Some(false), Some(false), Some(false)],
            "the four workspaces in place, the first shown"
        );
        assert!(
            builder.build(Menus::WINDOW).is_some_and(|menu| menu
                .items
                .iter()
                .all(|item| !matches!(item, MenuItem::Choice { mark: Some(_), .. }))),
            "without a window, the window menu has no toggles"
        );
    }

    #[test]
    fn the_root_menus_vaht_submenu_is_generated() {
        let config = Config::built_in();
        let output = OutputArea {
            whole: bounds(),
            usable: bounds(),
        };
        let manager = WindowManager::new(
            &config.settings,
            OutputAreas::new(output, Vec::new()),
            config.style.metrics(),
        );
        let builder = MenuBuilder::new(&config.menus, &manager, None);
        let root = builder.build(Menus::ROOT).expect("built in");
        let Some(MenuItem::Submenu { label, menu, .. }) = root.items.last() else {
            panic!("the last item opens vaht's menu");
        };
        assert_eq!(label, "vaht");
        let labels: Vec<&str> = menu
            .items
            .iter()
            .map(|item| match item {
                MenuItem::Choice { label, .. }
                | MenuItem::Submenu { label, .. }
                | MenuItem::Inert { label } => label.as_str(),
                MenuItem::Separator => "-",
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Styles",
                "Style Options",
                "Layouts",
                "Position",
                "-",
                "Advanced Configuration...",
                "vaht Command",
                "Reload Config",
                "Save Config",
                "-",
                "Exit"
            ]
        );
    }

    /// A point on the title of the menu opened at (100, 100) by `model()`.
    fn on_the_title() -> Point<f64, Logical> {
        (130.0, 108.0).into()
    }

    #[test]
    fn a_menu_is_moved_by_holding_its_title_and_chooses_nothing_when_let_go() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        let before = stack.menus[0].geometry.outer.loc;
        assert!(stack.press(on_the_title()), "the title is on the menu");
        assert!(stack.drag_to((330.0, 308.0).into(), bounds()));
        let after = stack.menus[0].geometry.outer.loc;
        assert_eq!(
            (after.x - before.x, after.y - before.y),
            (200, 200),
            "the menu goes where the pointer does"
        );
        assert!(
            stack.menus[0].geometry.covers((330.0, 308.0).into()),
            "its parts moved with it"
        );
        assert_eq!(
            stack.release((330.0, 308.0).into(), (&style, &mut typesetter), false),
            None,
            "letting go of a title chooses nothing"
        );
        assert!(stack.is_open(), "and the menu stays");
        assert!(
            !stack.drag_to((600.0, 600.0).into(), bounds()),
            "the move is over"
        );
    }

    #[test]
    fn a_menu_moved_stays_on_the_output_and_an_item_is_not_a_handle() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        assert!(stack.press(on_the_title()));
        stack.drag_to((-500.0, -500.0).into(), bounds());
        let corner = stack.menus[0].geometry.outer.loc;
        assert_eq!((corner.x, corner.y), (0, 0), "no further than the output");
        stack.release((0.0, 0.0).into(), (&style, &mut typesetter), false);
        // A press on an item does not start a move.
        let item = {
            let outer = stack.menus[0].geometry.outer;
            Point::<f64, Logical>::from((
                f64::from(outer.loc.x) + 20.0,
                f64::from(outer.loc.y) + 40.0,
            ))
        };
        assert!(stack.press(item));
        assert!(!stack.drag_to((500.0, 500.0).into(), bounds()));
    }

    #[test]
    fn a_moved_menu_is_put_back_where_it_was_moved_when_the_menus_open_again() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        stack.mark_opened_by(MenuRequest::Root, None);
        stack.press(on_the_title());
        stack.drag_to((430.0, 408.0).into(), bounds());
        stack.release((430.0, 408.0).into(), (&style, &mut typesetter), false);
        let moved_to = stack.menus[0].geometry.outer.loc;
        let snapshot = stack.snapshot().expect("opened by a request");
        // The menus are made again where they first opened, then restored.
        stack.open(model(), snapshot.at, bounds(), (&style, &mut typesetter));
        assert_ne!(stack.menus[0].geometry.outer.loc, moved_to);
        stack.restore(
            &snapshot.path,
            &snapshot.moved,
            snapshot.bounds,
            (&style, &mut typesetter),
        );
        assert_eq!(stack.menus[0].geometry.outer.loc, moved_to);
    }

    #[test]
    fn a_menu_is_carried_from_one_output_to_the_next_and_kept_there() {
        let (style, mut typesetter) = style();
        let mut stack = MenuStack::default();
        stack.open(
            model(),
            (100, 100).into(),
            bounds(),
            (&style, &mut typesetter),
        );
        stack.mark_opened_by(MenuRequest::Root, None);
        // A second output to the right of the first.
        let next: Area = Rectangle::new((1920, 0).into(), (1920, 1080).into());
        assert!(stack.press(on_the_title()));
        assert!(stack.drag_to((2400.0, 300.0).into(), next));
        let moved_to = stack.menus[0].geometry.outer.loc;
        assert_eq!(
            (moved_to.x, moved_to.y),
            (2400 - 30, 300 - 8),
            "the menu is where the pointer is, on the other output"
        );
        stack.release((2400.0, 300.0).into(), (&style, &mut typesetter), false);
        // Opened again after a choice, it is still there, and a submenu fits
        // on the output it is on.
        let snapshot = stack.snapshot().expect("opened by a request");
        stack.open(model(), snapshot.at, bounds(), (&style, &mut typesetter));
        stack.restore(
            &snapshot.path,
            &snapshot.moved,
            snapshot.bounds,
            (&style, &mut typesetter),
        );
        assert_eq!(stack.menus[0].geometry.outer.loc, moved_to);
        assert_eq!(stack.bounds, next);
    }
}
