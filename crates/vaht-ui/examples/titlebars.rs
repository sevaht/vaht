//! Renders the built-in style's titlebars to a PNG, to look at while working
//! on styles: `cargo run -p vaht-ui --example titlebars titlebars.png`.

use std::process::ExitCode;

use tiny_skia::{Pixmap, PixmapPaint, Transform};
use vaht_ui::{
    Glyph, Panel, PanelContent, Point, Size, SizePolicy, Style, StyleElement, Typesetter, Widget,
    WidgetState,
};

/// The size of each titlebar.
const SIZE: Size = Size::new(360, 20);

/// The titlebar's buttons, as clicks report them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    /// The minimize button.
    Minimize,
    /// The close button.
    Close,
}

/// A window title and whether the window is focused.
struct Titlebar {
    /// The text shown.
    title: &'static str,
    /// Whether to draw the focused style.
    is_focused: bool,
}

impl PanelContent for Titlebar {
    type Hit = Button;

    fn build(&self) -> Widget<Button> {
        Widget::row(
            StyleElement::Titlebar,
            vec![
                Widget::text(StyleElement::Label, self.title)
                    .with_size(SizePolicy::Fill { weight: 1 }),
                Widget::glyph(StyleElement::Button, Glyph::Minimize).with_button(Button::Minimize),
                Widget::glyph(StyleElement::Button, Glyph::Close).with_button(Button::Close),
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

#[expect(clippy::print_stderr, reason = "an example reports its own errors")]
fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: titlebars <output.png>");
        return ExitCode::FAILURE;
    };
    let style = Style::built_in();
    let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
    let mut focused = Panel::new(
        Titlebar {
            title: "kitty: ~/scm/personal/production/vaht",
            is_focused: true,
        },
        SIZE,
    );
    let mut unfocused = Panel::new(
        Titlebar {
            title: "brave: vaht design",
            is_focused: false,
        },
        SIZE,
    );
    let mut pressed = Panel::new(
        Titlebar {
            title: "close button held down",
            is_focused: true,
        },
        SIZE,
    );
    for panel in [&mut focused, &mut unfocused, &mut pressed] {
        panel.paint(&style, &mut typesetter);
    }
    // Hold the close button: its rectangle is the last 20 pixels.
    let close = Point::new(i32::try_from(SIZE.width).expect("small") - 10, 10);
    pressed.press(close);
    pressed.paint(&style, &mut typesetter);

    let Some(mut sheet) = Pixmap::new(SIZE.width, SIZE.height * 3) else {
        return ExitCode::FAILURE;
    };
    for (row, panel) in [focused.pixels(), unfocused.pixels(), pressed.pixels()]
        .into_iter()
        .enumerate()
    {
        let Some(pixels) = panel else {
            return ExitCode::FAILURE;
        };
        let top =
            i32::try_from(row).expect("three rows") * i32::try_from(SIZE.height).expect("small");
        sheet.draw_pixmap(
            0,
            top,
            pixels.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    match sheet.save_png(&path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("could not write {path}: {error}");
            ExitCode::FAILURE
        }
    }
}
