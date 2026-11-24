//! The typefaces of vaht's own windows (the settings window and the dialogs):
//! the ones its default style names, so every window vaht shows looks of a
//! piece. egui's own fonts stay behind them for any character they lack.

use eframe::egui::{self, FontData, FontFamily, FontInsert, FontPriority, InsertFontFamily};
use vaht_ui::FontWeight;

/// The typeface of text, and of what is typed or fixed-width: the default
/// style's (`data/styles/default.toml`), and its monospaced companion.
const FACES: [(&str, FontFamily); 2] = [
    ("DejaVu Sans", FontFamily::Proportional),
    ("DejaVu Sans Mono", FontFamily::Monospace),
];

/// Puts the typefaces the default style uses first in egui's families, each
/// one that is installed.
pub fn install(context: &egui::Context) {
    for (name, family) in FACES {
        // One that is not installed is left out: egui's own font shows.
        let Some((bytes, index)) = vaht_ui::system_font(name, FontWeight::Normal) else {
            continue;
        };
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        context.add_font(FontInsert::new(
            name,
            data,
            vec![InsertFontFamily {
                family,
                priority: FontPriority::Highest,
            }],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How wide a line of letters is at 20 points, in `context`.
    fn width_of_text(context: &egui::Context) -> f32 {
        // A frame must run for the fonts to take effect.
        let mut output = context.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        context.fonts_mut(|fonts| {
            "Hamburgefonstiv Quickly"
                .chars()
                .map(|letter| fonts.glyph_width(&egui::FontId::proportional(20.0), letter))
                .sum()
        })
    }

    #[test]
    fn the_text_of_vahts_windows_is_set_in_the_default_styles_typeface() {
        if vaht_ui::system_font("DejaVu Sans", FontWeight::Normal).is_none() {
            return;
        }
        let context = egui::Context::default();
        let before = width_of_text(&context);
        install(&context);
        let after = width_of_text(&context);
        assert!(
            (after - before).abs() > 0.5,
            "another typeface: {before} wide, then {after}"
        );
    }
}
