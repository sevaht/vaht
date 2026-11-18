//! A rectangle vaht draws itself: one type for frames, taskbars and menus.
//!
//! A [`Panel`] owns its [`PanelContent`] and does everything else: it lays
//! the content's widgets out, paints them with the style, finds the widget
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
use crate::style::{Style, WidgetState};
use crate::text::Typesetter;
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
    /// The button the pointer is on.
    hovered: Option<Content::Hit>,
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
            hovered: None,
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

    /// Returns where the widget reporting `hit` is, in the panel's pixels,
    /// as last laid out.
    pub fn area_of(&self, hit: Content::Hit) -> Option<Rect>
    where
        Content::Hit: PartialEq,
    {
        self.layout
            .as_ref()?
            .placed()
            .iter()
            .find(|placed| placed.hit == Some(hit))
            .map(|placed| placed.rect)
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

    /// Notes where the pointer is, at `point` in the panel (none: not on it);
    /// returns whether the button it is on changed, which needs painting
    /// again.
    pub fn hover(&mut self, point: Option<Point>) -> bool {
        let hit = point.and_then(|point| self.button_hit_at(point));
        if hit == self.hovered {
            return false;
        }
        self.hovered = hit;
        self.invalidate();
        true
    }

    /// Returns what clicking at `point` would report, if it is a button.
    fn button_hit_at(&self, point: Point) -> Option<Content::Hit> {
        let layout = self.layout.as_ref()?;
        let hit = layout.hit_at(point)?;
        layout
            .placed()
            .iter()
            .any(|placed| placed.is_button && placed.hit == Some(hit))
            .then_some(hit)
    }

    /// Lets go without clicking anything.
    pub fn cancel_press(&mut self) {
        self.set_pressed(None);
    }

    /// Lays out and paints the panel if anything changed; returns whether it
    /// did.
    pub fn paint(&mut self, style: &Style, typesetter: &mut Typesetter) -> bool {
        if !self.is_dirty {
            return false;
        }
        self.is_dirty = false;
        let layout = Self::lay_out(&self.content, self.size, style, typesetter);
        self.pixels = Pixmap::new(self.size.width, self.size.height);
        if let Some(pixels) = &mut self.pixels {
            let base = self.content.state();
            let (pressed, hovered) = (self.pressed, self.hovered);
            Canvas::new(pixels).paint(&layout, style, typesetter, |placed| {
                let is_button = placed.is_button && placed.hit.is_some();
                if is_button && placed.hit == pressed {
                    (WidgetState::Pressed, false)
                } else {
                    (
                        placed.state.unwrap_or(base),
                        is_button && placed.hit == hovered,
                    )
                }
            });
            self.corners.cut(pixels);
        }
        self.layout = Some(layout);
        self.generation += 1;
        true
    }

    /// Returns where each widget of `content` goes in a panel of `size`,
    /// as `style` styles it: what painting lays out, and what content can
    /// try before settling on its shape.
    pub fn lay_out(
        content: &Content,
        size: Size,
        style: &Style,
        typesetter: &mut Typesetter,
    ) -> Layout<Content::Hit> {
        let root = content.build();
        let mut fit_width = |text: &str, element| {
            // The fitted width is the text's, plus the element's padding in
            // the content's normal state.
            let style = style.element_style(element, content.state());
            typesetter.text_width(text, &style.text.font) + style.padding.horizontal()
        };
        Layout::new(&root, Rect::from_size(size), &mut fit_width)
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
    use crate::style::StyleElement;
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
                StyleElement::Titlebar,
                vec![
                    Widget::text(StyleElement::Label, self.title.clone())
                        .with_size(crate::widget::SizePolicy::Fill { weight: 1 })
                        .with_hit(TitlebarHit::Label),
                    Widget::glyph(StyleElement::Button, Glyph::Close)
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
        assert!(panel.paint(&Style::built_in(), typesetter));
        panel
    }

    /// The test titlebar, painted in `style`.
    fn painted_titlebar_in(style: &Style, typesetter: &mut Typesetter) -> Panel<Titlebar> {
        let mut panel = Panel::new(
            Titlebar {
                title: "kitty: ~/scm".to_owned(),
                is_focused: true,
            },
            Size::new(200, 20),
        );
        assert!(panel.paint(style, typesetter));
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
        let style = Style::built_in();
        let mut panel = painted_titlebar(&mut typesetter);
        let generation = panel.generation();
        assert!(!panel.paint(&style, &mut typesetter), "nothing changed");
        assert_eq!(panel.generation(), generation);
        panel.update(|titlebar| titlebar.is_focused = false);
        assert!(panel.paint(&style, &mut typesetter));
        assert_eq!(panel.generation(), generation + 1);
    }

    #[test]
    fn pressing_a_button_repaints_it_in_the_pressed_style() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let style = Style::built_in();
        let mut panel = painted_titlebar(&mut typesetter);
        // A corner of the button, away from its picture.
        let before = pixel(&panel, 181, 18);
        panel.press(Point::new(190, 10));
        assert!(panel.paint(&style, &mut typesetter));
        assert_ne!(pixel(&panel, 181, 18), before);
    }

    #[test]
    fn a_button_under_the_pointer_takes_what_its_hovered_section_sets() {
        let layers = crate::style::StyleLayers::from_toml(
            "[window.button]\ntexture = { solid = \"black\" }\n\
             [window.button.hovered]\ntexture = { solid = \"red\" }\n",
        )
        .expect("a valid style");
        let style = Style::resolve(&layers);
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = painted_titlebar_in(&style, &mut typesetter);
        assert_eq!(pixel(&panel, 185, 3), (0, 0, 0));
        assert!(panel.hover(Some(Point::new(190, 10))), "the button changed");
        assert!(panel.paint(&style, &mut typesetter));
        assert_eq!(pixel(&panel, 185, 3), (255, 0, 0), "hovered");
        assert!(
            !panel.hover(Some(Point::new(191, 11))),
            "still the same button"
        );
        assert!(panel.hover(None));
        panel.paint(&style, &mut typesetter);
        assert_eq!(pixel(&panel, 185, 3), (0, 0, 0), "and back");
    }

    #[test]
    fn a_style_without_a_hovered_section_looks_the_same_hovered_or_not() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let style = Style::built_in();
        let mut panel = painted_titlebar(&mut typesetter);
        let before = panel.pixels().expect("painted").clone();
        panel.hover(Some(Point::new(190, 10)));
        panel.paint(&style, &mut typesetter);
        assert_eq!(panel.pixels().expect("painted"), &before);
    }

    #[test]
    fn hovering_a_plain_hit_region_is_not_hovering_a_button() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = painted_titlebar(&mut typesetter);
        assert!(
            !panel.hover(Some(Point::new(20, 10))),
            "the label is no button"
        );
    }

    #[test]
    fn pressing_a_plain_hit_region_does_not_change_its_look() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let style = Style::built_in();
        let mut panel = painted_titlebar(&mut typesetter);
        let before = panel.pixels().expect("painted").clone();
        assert_eq!(panel.press(Point::new(20, 10)), Some(TitlebarHit::Label));
        panel.paint(&style, &mut typesetter);
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
                StyleElement::Taskbar,
                vec![
                    Widget::fill(StyleElement::WindowListEntry),
                    Widget::fill(StyleElement::WindowListEntry).with_state(WidgetState::Unfocused),
                ],
            )
        }

        fn state(&self) -> WidgetState {
            WidgetState::Focused
        }
    }

    #[test]
    fn a_widget_may_be_drawn_in_a_state_of_its_own() {
        let layers = crate::style::StyleLayers::from_toml(
            "[state.focused]\ntexture = { solid = \"red\" }\n\
             [state.unfocused]\ntexture = { solid = \"blue\" }\n",
        )
        .expect("a valid style");
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = Panel::new(Entries, Size::new(100, 10));
        assert!(panel.paint(&Style::resolve(&layers), &mut typesetter));
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

    /// A dialog's line being edited.
    struct Line(crate::widget::TextField);

    impl PanelContent for Line {
        type Hit = ();

        fn build(&self) -> Widget<()> {
            Widget::field(StyleElement::DialogField, self.0.clone())
        }

        fn state(&self) -> WidgetState {
            WidgetState::Focused
        }
    }

    /// Returns the columns of a painted 100 by 20 line with any dark pixel.
    fn dark_columns(field: crate::widget::TextField) -> Vec<u32> {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = Panel::new(Line(field), Size::new(100, 20));
        assert!(panel.paint(&Style::built_in(), &mut typesetter));
        let pixels = panel.pixels().expect("painted");
        (0..100)
            .filter(|x| {
                (0..20)
                    .filter_map(|y| pixels.pixel(*x, y))
                    .any(|pixel| pixel.red() < 128)
            })
            .collect()
    }

    #[test]
    fn an_empty_field_shows_its_cursor_at_the_start() {
        let columns = dark_columns(crate::widget::TextField::default());
        assert_eq!(columns, [2], "only the cursor, inside the padding");
    }

    #[test]
    fn a_long_field_scrolls_to_keep_its_cursor_in_view() {
        let text = "x".repeat(60);
        let at_end = dark_columns(crate::widget::TextField::new(text.clone(), text.len()));
        assert_eq!(at_end.last(), Some(&97), "the cursor at the right edge");
        let at_start = dark_columns(crate::widget::TextField::new(text, 0));
        assert_eq!(at_start.first(), Some(&2), "the cursor at the left edge");
    }

    /// One item-high row with a check on the left.
    struct Checked;

    impl PanelContent for Checked {
        type Hit = ();

        fn build(&self) -> Widget<()> {
            Widget::text(StyleElement::MenuItem, "x")
                .with_mark_on(Glyph::Check, crate::widget::MarkSide::Left)
        }

        fn state(&self) -> WidgetState {
            WidgetState::Focused
        }
    }

    #[test]
    fn a_mark_goes_on_its_side_in_the_glyph_color() {
        let layers = crate::style::StyleLayers::from_toml(
            "[default]\ntexture = { solid = \"black\" }\nglyph_color = \"red\"\n",
        )
        .expect("a valid style");
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = Panel::new(Checked, Size::new(100, 20));
        assert!(panel.paint(&Style::resolve(&layers), &mut typesetter));
        let red_at = |x| {
            let pixel = panel
                .pixels()
                .and_then(|pixels| pixels.pixel(x, 10))
                .expect("inside the panel");
            pixel.red() == 255 && pixel.green() == 0
        };
        assert!(red_at(10), "the check, in the left square");
        assert!(!red_at(90), "nothing at the right end");
    }

    /// A taskbar arrow button, 20 pixels square.
    struct Arrow;

    impl PanelContent for Arrow {
        type Hit = ();

        fn build(&self) -> Widget<()> {
            Widget::glyph(StyleElement::TaskbarButton, Glyph::ArrowLeft)
        }

        fn state(&self) -> WidgetState {
            WidgetState::Unfocused
        }
    }

    #[test]
    fn a_taskbar_arrow_is_two_fifths_of_its_button_as_fluxbox_draws_it() {
        let layers = crate::style::StyleLayers::from_toml(
            "[default]\ntexture = { solid = \"black\" }\nglyph_color = \"white\"\n",
        )
        .expect("a valid style");
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = Panel::new(Arrow, Size::new(20, 20));
        assert!(panel.paint(&Style::resolve(&layers), &mut typesetter));
        let pixels = panel.pixels().expect("painted");
        // Any coverage at all: the arrow is smoothed at its edges.
        let lit = |x, y| pixels.pixel(x, y).is_some_and(|pixel| pixel.red() > 0);
        let columns: Vec<u32> = (0..20).filter(|&x| (0..20).any(|y| lit(x, y))).collect();
        let rows: Vec<u32> = (0..20).filter(|&y| (0..20).any(|x| lit(x, y))).collect();
        // Measured on Fluxbox: columns 6 to 13, eight rows, tip at the left.
        assert_eq!(
            columns,
            (6..14).collect::<Vec<_>>(),
            "eight wide from the tip"
        );
        assert_eq!(rows.len(), 8, "eight tall: {rows:?}");
    }

    /// A close button, 20 pixels square, drawn in whatever state is asked.
    struct CloseButton(WidgetState);

    impl PanelContent for CloseButton {
        type Hit = ();

        fn build(&self) -> Widget<()> {
            Widget::glyph(StyleElement::Button, Glyph::Close)
        }

        fn state(&self) -> WidgetState {
            self.0
        }
    }

    #[test]
    fn a_button_picture_from_the_style_replaces_the_glyph_by_state() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        for (name, color) in [("up.xpm", "#ff0000"), ("down.xpm", "#0000ff")] {
            let xpm = format!("\"1 1 1 1\",\n\"a\tc {color}\",\n\"a\"");
            std::fs::write(directory.path().join(name), xpm).expect("written");
        }
        let layers = crate::style::StyleLayers::from_toml_in(
            "[window.button]\ntexture = { solid = \"black\" }\n\
             glyph_images = { close = \"up.xpm\" }\n\
             [window.button.pressed]\nglyph_images = { close = \"down.xpm\" }\n",
            Some(directory.path()),
        )
        .expect("a valid style");
        let style = Style::resolve(&layers);
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut center = |state| {
            let mut panel = Panel::new(CloseButton(state), Size::new(20, 20));
            assert!(panel.paint(&style, &mut typesetter));
            let pixel = panel
                .pixels()
                .and_then(|pixels| pixels.pixel(10, 10))
                .expect("inside the panel");
            (pixel.red(), pixel.blue())
        };
        assert_eq!(
            center(WidgetState::Focused),
            (255, 0),
            "the picture, not an X"
        );
        assert_eq!(
            center(WidgetState::Pressed),
            (0, 255),
            "a state has its own"
        );
    }

    /// A pair of pixels, one black and one white, to tile.
    const TWO_PIXELS: &str = "\"2 1 2 1\",\n\"a\tc #000000\",\n\"b\tc #ffffff\",\n\"ab\"";

    /// An element of the titlebar button's texture alone.
    struct Plain;

    impl PanelContent for Plain {
        type Hit = ();

        fn build(&self) -> Widget<()> {
            Widget::glyph(StyleElement::Button, Glyph::Minimize)
        }

        fn state(&self) -> WidgetState {
            WidgetState::Focused
        }
    }

    #[test]
    fn a_tiled_image_repeats_at_its_own_size_and_a_stretched_one_scales() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join("two.xpm"), TWO_PIXELS).expect("written");
        let paint = |fit: &str| {
            let layers = crate::style::StyleLayers::from_toml_in(
                &format!(
                    "[window.button]\ntexture = {{ image = \"two.xpm\", fit = \"{fit}\" }}\n\
                     glyph_color = \"#00000000\"\n"
                ),
                Some(directory.path()),
            )
            .expect("a valid style");
            let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
            let mut panel = Panel::new(Plain, Size::new(8, 2));
            assert!(panel.paint(&Style::resolve(&layers), &mut typesetter));
            let pixels = panel.pixels().expect("painted");
            (0..8)
                .map(|x| pixels.pixel(x, 0).expect("inside").red())
                .collect::<Vec<_>>()
        };
        assert_eq!(paint("tile"), [0, 255, 0, 255, 0, 255, 0, 255], "repeated");
        let stretched = paint("stretch");
        assert!(
            stretched[0] < 64 && stretched[7] > 192,
            "scaled across: {stretched:?}"
        );
    }

    #[test]
    fn the_stuck_and_menu_glyphs_are_drawn() {
        let layers = crate::style::StyleLayers::from_toml(
            "[default]\ntexture = { solid = \"black\" }\nglyph_color = \"white\"\n",
        )
        .expect("a valid style");
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        for glyph in [Glyph::Stuck, Glyph::Menu] {
            struct One(Glyph);
            impl PanelContent for One {
                type Hit = ();
                fn build(&self) -> Widget<()> {
                    Widget::glyph(StyleElement::Button, self.0)
                }
                fn state(&self) -> WidgetState {
                    WidgetState::Focused
                }
            }
            let mut panel = Panel::new(One(glyph), Size::new(20, 20));
            assert!(panel.paint(&Style::resolve(&layers), &mut typesetter));
            let pixels = panel.pixels().expect("painted");
            let lit = (0..20)
                .flat_map(|y| (0..20).map(move |x| (x, y)))
                .any(|(x, y)| pixels.pixel(x, y).is_some_and(|pixel| pixel.red() > 128));
            assert!(lit, "{glyph:?} draws something");
        }
    }

    /// Two stacked rows on a gradient background, each with or without a background
    /// of its own.
    struct Stacked {
        rows_have_background: bool,
    }

    impl PanelContent for Stacked {
        type Hit = TitlebarHit;

        fn build(&self) -> Widget<TitlebarHit> {
            let row = || {
                let row = Widget::row(StyleElement::Label, Vec::new());
                if self.rows_have_background {
                    row
                } else {
                    row.without_background()
                }
            };
            Widget::column(StyleElement::Taskbar, vec![row(), row()])
        }

        fn state(&self) -> WidgetState {
            WidgetState::Focused
        }
    }

    /// Returns the colors at the top of a stack's first and second rows
    /// (each 20 pixels tall).
    fn tops_of_two_rows(rows_have_background: bool) -> ((u8, u8, u8), (u8, u8, u8)) {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = Panel::new(
            Stacked {
                rows_have_background,
            },
            Size::new(40, 40),
        );
        assert!(panel.paint(&Style::built_in(), &mut typesetter));
        let top = |y| {
            let pixel = panel
                .pixels()
                .and_then(|pixels| pixels.pixel(20, y))
                .expect("a pixel inside the panel");
            (pixel.red(), pixel.green(), pixel.blue())
        };
        (top(1), top(21))
    }

    #[test]
    fn a_widget_with_a_background_starts_the_gradient_again() {
        let (first, second) = tops_of_two_rows(true);
        assert_eq!(first, second, "each row has a gradient of its own");
    }

    #[test]
    fn a_widget_without_a_background_shows_its_parents_unbroken() {
        let (first, second) = tops_of_two_rows(false);
        assert_ne!(first, second, "one gradient runs behind both rows");
    }

    /// A panel that is one glyph.
    struct OneGlyph(Glyph);

    impl PanelContent for OneGlyph {
        type Hit = ();

        fn build(&self) -> Widget<()> {
            Widget::glyph(StyleElement::Button, self.0)
        }

        fn state(&self) -> WidgetState {
            WidgetState::Focused
        }
    }

    /// Returns which pixels of a 40 by 40 panel `glyph` marks, as rows of
    /// text: `#` where it shows.
    fn drawing_of(glyph: Glyph) -> Vec<String> {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut panel = Panel::new(OneGlyph(glyph), Size::new(40, 40));
        assert!(panel.paint(&Style::built_in(), &mut typesetter));
        let pixels = panel.pixels().expect("a painted panel");
        // The glyph is white on the style's grey gradient.
        (0..40)
            .map(|y| {
                (0..40)
                    .map(|x| {
                        let pixel = pixels.pixel(x, y).expect("inside");
                        if pixel.red() > 200 { '#' } else { '.' }
                    })
                    .collect()
            })
            .collect()
    }

    /// Returns whether the three by three pixels at each of the four corners
    /// of `drawing`'s footprint (its marked pixels' bounds) have a mark:
    /// top left, top right, bottom left, bottom right.
    fn corners_marked(drawing: &[String]) -> [bool; 4] {
        let marks: Vec<(usize, usize)> = drawing
            .iter()
            .enumerate()
            .flat_map(|(y, row)| {
                row.chars()
                    .enumerate()
                    .filter(|(_, mark)| *mark == '#')
                    .map(move |(x, _)| (x, y))
            })
            .collect();
        let (left, right) = (
            marks
                .iter()
                .map(|&(column, _)| column)
                .min()
                .expect("marks"),
            marks
                .iter()
                .map(|&(column, _)| column)
                .max()
                .expect("marks"),
        );
        let (top, bottom) = (
            marks.iter().map(|&(_, row)| row).min().expect("marks"),
            marks.iter().map(|&(_, row)| row).max().expect("marks"),
        );
        let near = |corner_x: usize, corner_y: usize| {
            marks
                .iter()
                .any(|(x, y)| x.abs_diff(corner_x) < 3 && y.abs_diff(corner_y) < 3)
        };
        [
            near(left, top),
            near(right, top),
            near(left, bottom),
            near(right, bottom),
        ]
    }

    #[test]
    fn the_restore_glyph_is_two_offset_boxes_unlike_the_maximize_box() {
        // One box has all four corners; two boxes, the back one up and to the
        // right, leave the top left and the bottom right of the footprint
        // empty.
        assert_eq!(corners_marked(&drawing_of(Glyph::Maximize)), [true; 4]);
        assert_eq!(
            corners_marked(&drawing_of(Glyph::Restore)),
            [false, true, true, false]
        );
    }
}
