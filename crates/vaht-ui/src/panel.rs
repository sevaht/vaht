//! A rectangle vaht draws itself: one type for frames, taskbars and menus.
//!
//! A [`Panel`] owns its [`PanelContent`] and does everything else: it lays
//! the content's widgets out, paints them with the theme, finds the widget
//! under a point, and tracks which one the pointer holds down. Content types
//! only describe what they show (01 §6). The panel repaints only when
//! something changed; [`Panel::generation`] tells a compositor when the
//! pixels are new. A panel's corners can be rounded ([`Panel::set_corners`]):
//! what lies outside them is clear.

use tiny_skia::Pixmap;

use crate::canvas::Canvas;
use crate::corner::CornerRadii;
use crate::geometry::{Point, Rect, Size};
use crate::layout::Layout;
use crate::text::Typesetter;
use crate::theme::{Theme, WidgetState};
use crate::widget::Widget;

/// What a panel shows; implemented by window frames, the taskbar, menus.
pub trait PanelContent {
    /// What clicking one of the content's widgets reports.
    type Hit: Copy + Eq + std::fmt::Debug;

    /// Builds the widget tree for the content's current state.
    fn build(&self) -> Widget<Self::Hit>;

    /// Returns the state every widget is drawn in unless it is pressed.
    fn state(&self) -> WidgetState;
}

/// A panel: content, its layout, its pixels, and the pointer's grip on it.
#[derive(Debug)]
pub struct Panel<Content: PanelContent> {
    /// What it shows.
    content: Content,
    /// Its size in pixels.
    size: Size,
    /// How round its corners are.
    corners: CornerRadii,
    /// Where each widget is; `None` until the next paint lays it out.
    layout: Option<Layout<Content::Hit>>,
    /// The painted pixels; `None` while the panel has no area.
    pixels: Option<Pixmap>,
    /// The widget the pointer is holding down.
    pressed: Option<Content::Hit>,
    /// Whether the pixels are out of date.
    is_dirty: bool,
    /// Counts repaints, so a compositor knows when to upload again.
    generation: u64,
}

impl<Content: PanelContent> Panel<Content> {
    /// Creates a panel of `size` showing `content`.
    pub const fn new(content: Content, size: Size) -> Self {
        Self {
            content,
            size,
            corners: CornerRadii::SQUARE,
            layout: None,
            pixels: None,
            pressed: None,
            is_dirty: true,
            generation: 0,
        }
    }

    /// Returns what the panel shows.
    pub const fn content(&self) -> &Content {
        &self.content
    }

    /// Returns the panel's size.
    pub const fn size(&self) -> Size {
        self.size
    }

    /// Returns the painted pixels, if the panel has been painted and has an
    /// area.
    pub const fn pixels(&self) -> Option<&Pixmap> {
        self.pixels.as_ref()
    }

    /// Returns a number that changes whenever the pixels change.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns what clicking at `point` would report, as of the last paint.
    pub fn hit_at(&self, point: Point) -> Option<Content::Hit> {
        self.layout.as_ref()?.hit_at(point)
    }

    /// Returns the widget the pointer is holding down.
    pub const fn pressed(&self) -> Option<Content::Hit> {
        self.pressed
    }

    /// Changes the content; the panel is laid out and painted again.
    pub fn update(&mut self, change: impl FnOnce(&mut Content)) {
        change(&mut self.content);
        self.invalidate();
    }

    /// Changes the content with `change`, which returns whether it changed
    /// anything; only then is the panel laid out and painted again. Returns
    /// what `change` did.
    pub fn update_if(&mut self, change: impl FnOnce(&mut Content) -> bool) -> bool {
        let changed = change(&mut self.content);
        if changed {
            self.invalidate();
        }
        changed
    }

    /// Resizes the panel.
    pub fn resize(&mut self, size: Size) {
        if size != self.size {
            self.size = size;
            self.invalidate();
        }
    }

    /// Rounds the panel's corners by `corners`.
    pub fn set_corners(&mut self, corners: CornerRadii) {
        if corners != self.corners {
            self.corners = corners;
            self.invalidate();
        }
    }

    /// Holds down the widget at `point`, if it can be clicked; returns it.
    pub fn press(&mut self, point: Point) -> Option<Content::Hit> {
        let hit = self.hit_at(point);
        self.set_pressed(hit);
        hit
    }

    /// Lets go: returns the widget that was held, if the pointer is still on
    /// it. That is a click; letting go elsewhere cancels it.
    pub fn release(&mut self, point: Point) -> Option<Content::Hit> {
        let pressed = self.pressed;
        self.set_pressed(None);
        pressed.filter(|held| self.hit_at(point) == Some(*held))
    }

    /// Lets go without clicking anything.
    pub fn cancel_press(&mut self) {
        self.set_pressed(None);
    }

    /// Lays out and paints the panel if anything changed; returns whether it
    /// did.
    pub fn paint(&mut self, theme: &Theme, typesetter: &mut Typesetter) -> bool {
        if !self.is_dirty {
            return false;
        }
        self.is_dirty = false;
        let root = self.content.build();
        let mut fit_width = |text: &str, element| {
            // The fitted width is the text's, plus the element's padding in
            // the content's normal state.
            let style = theme.style(element, self.content.state());
            typesetter.text_width(text, &style.text.font) + style.padding.horizontal()
        };
        let layout = Layout::new(&root, Rect::from_size(self.size), &mut fit_width);
        self.pixels = Pixmap::new(self.size.width, self.size.height);
        if let Some(pixels) = &mut self.pixels {
            let base = self.content.state();
            let pressed = self.pressed;
            Canvas::new(pixels).paint(&layout, theme, typesetter, |placed| {
                if placed.is_button && placed.hit.is_some() && placed.hit == pressed {
                    WidgetState::Pressed
                } else {
                    placed.state.unwrap_or(base)
                }
            });
            self.corners.cut(pixels);
        }
        self.layout = Some(layout);
        self.generation += 1;
        true
    }

    /// Marks the panel for laying out and painting again.
    pub fn invalidate(&mut self) {
        self.is_dirty = true;
    }

    /// Holds `hit` down (or nothing), repainting if that changes anything.
    fn set_pressed(&mut self, hit: Option<Content::Hit>) {
        if hit != self.pressed {
            self.pressed = hit;
            self.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeElement;
    use crate::widget::Glyph;

    /// What clicking the test titlebar can report.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TitlebarHit {
        Label,
        Close,
    }

    /// A titlebar with a title and a close button.
    struct Titlebar {
        title: String,
        is_focused: bool,
    }

    impl PanelContent for Titlebar {
        type Hit = TitlebarHit;

        fn build(&self) -> Widget<TitlebarHit> {
            Widget::row(
                ThemeElement::Titlebar,
                vec![
                    Widget::text(ThemeElement::Label, self.title.clone())
                        .with_size(crate::widget::SizePolicy::Fill { weight: 1 })
                        .with_hit(TitlebarHit::Label),
                    Widget::glyph(ThemeElement::Button, Glyph::Close)
                        .with_button(TitlebarHit::Close),
                ],
            )
        }

        fn state(&self) -> WidgetState {
            if self.is_focused {
                WidgetState::Focused
            } else {
                WidgetState::Unfocused
            }
        }
    }

    /// A painted 200 by 20 titlebar panel.
    fn painted_titlebar(typesetter: &mut Typesetter) -> Panel<Titlebar> {
        let mut panel = Panel::new(
            Titlebar {
                title: "kitty: ~/scm".to_owned(),
                is_focused: true,
            },
            Size::new(200, 20),
        );
        assert!(panel.paint(&Theme::built_in(), typesetter));
        panel
    }

    /// The color of the pixel at `x`, `y`.
    fn pixel(panel: &Panel<Titlebar>, x: u32, y: u32) -> (u8, u8, u8) {
        let pixel = panel
            .pixels()
            .and_then(|pixels| pixels.pixel(x, y))
            .expect("the panel is painted and the pixel inside it");
        (pixel.red(), pixel.green(), pixel.blue())
    }

    #[test]
    fn a_click_reports_the_widget_pressed_and_released_on() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = painted_titlebar(&mut typesetter);
        assert_eq!(panel.press(Point::new(190, 10)), Some(TitlebarHit::Close));
        assert_eq!(panel.release(Point::new(191, 11)), Some(TitlebarHit::Close));
    }

    #[test]
    fn releasing_elsewhere_cancels_the_click() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = painted_titlebar(&mut typesetter);
        panel.press(Point::new(190, 10));
        assert_eq!(panel.release(Point::new(20, 10)), None);
        assert_eq!(panel.pressed(), None);
    }

    #[test]
    fn painting_happens_only_after_a_change() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let theme = Theme::built_in();
        let mut panel = painted_titlebar(&mut typesetter);
        let generation = panel.generation();
        assert!(!panel.paint(&theme, &mut typesetter), "nothing changed");
        assert_eq!(panel.generation(), generation);
        panel.update(|titlebar| titlebar.is_focused = false);
        assert!(panel.paint(&theme, &mut typesetter));
        assert_eq!(panel.generation(), generation + 1);
    }

    #[test]
    fn pressing_a_button_repaints_it_in_the_pressed_style() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let theme = Theme::built_in();
        let mut panel = painted_titlebar(&mut typesetter);
        // A corner of the button, away from its picture.
        let before = pixel(&panel, 181, 18);
        panel.press(Point::new(190, 10));
        assert!(panel.paint(&theme, &mut typesetter));
        assert_ne!(pixel(&panel, 181, 18), before);
    }

    #[test]
    fn pressing_a_plain_hit_region_does_not_change_its_look() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let theme = Theme::built_in();
        let mut panel = painted_titlebar(&mut typesetter);
        let before = panel.pixels().expect("painted").clone();
        assert_eq!(panel.press(Point::new(20, 10)), Some(TitlebarHit::Label));
        panel.paint(&theme, &mut typesetter);
        assert!(
            panel.pixels().expect("painted").data() == before.data(),
            "the title looks the same while held"
        );
    }

    #[test]
    fn the_focused_titlebar_is_lighter_at_the_top_than_the_bottom() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let panel = painted_titlebar(&mut typesetter);
        let (top, _, _) = pixel(&panel, 2, 1);
        let (bottom, _, _) = pixel(&panel, 2, 18);
        assert!(
            top > bottom,
            "the gradient runs light to dark: {top} vs {bottom}"
        );
    }

    #[test]
    fn the_title_is_drawn_white_with_a_dark_halo() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let panel = painted_titlebar(&mut typesetter);
        let pixels = panel.pixels().expect("painted");
        let label_pixels = (40..140).flat_map(|x| (4..16).map(move |y| (x, y)));
        let colors: Vec<_> = label_pixels
            .filter_map(|(x, y)| pixels.pixel(x, y))
            .map(|pixel| (pixel.red(), pixel.green(), pixel.blue()))
            .collect();
        assert!(colors.contains(&(255, 255, 255)), "white letters");
        assert!(colors.iter().any(|&(red, _, _)| red < 20), "a black halo");
    }

    /// Two fills side by side, the right one drawn unfocused in a focused
    /// panel.
    struct Entries;

    impl PanelContent for Entries {
        type Hit = ();

        fn build(&self) -> Widget<()> {
            Widget::row(
                ThemeElement::Taskbar,
                vec![
                    Widget::fill(ThemeElement::WindowListEntry),
                    Widget::fill(ThemeElement::WindowListEntry).with_state(WidgetState::Unfocused),
                ],
            )
        }

        fn state(&self) -> WidgetState {
            WidgetState::Focused
        }
    }

    #[test]
    fn a_widget_may_be_drawn_in_a_state_of_its_own() {
        let layers = crate::theme::ThemeLayers::from_toml(
            "[state.focused]\ntexture = { solid = \"red\" }\n\
             [state.unfocused]\ntexture = { solid = \"blue\" }\n",
        )
        .expect("a valid theme");
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = Panel::new(Entries, Size::new(100, 10));
        assert!(panel.paint(&Theme::resolve(&layers), &mut typesetter));
        let color = |x| {
            let pixel = panel
                .pixels()
                .and_then(|pixels| pixels.pixel(x, 5))
                .expect("inside the panel");
            (pixel.red(), pixel.green(), pixel.blue())
        };
        assert_eq!(color(25), (255, 0, 0));
        assert_eq!(color(75), (0, 0, 255));
    }
}
