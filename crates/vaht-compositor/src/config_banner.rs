//! The notice, on every output, that the configuration has problems.
//!
//! vaht goes on with what it can use of a configuration that is not all
//! valid (02 §2), so the one thing that says something was left out must be
//! impossible to miss: a red banner along the top of every output naming
//! what was left out and what was used instead. It stays until the files are
//! fixed; a click hides it (until the problems change). It is drawn from the
//! built-in style, never the user's, so a broken style cannot hide it.

use smithay::utils::{Logical, Point, Rectangle, Scale};
use vaht_ui::{Panel, Style, StyleElement, Typesetter, WidgetState};

use crate::config::ConfigProblem;
use crate::scene::{Outline, PanelTexture, SceneElement, SceneRenderer};
use crate::tooltip::Lines;
use crate::window_manager::Area;

/// The configuration's problems and the banner that names them: what it says
/// and where, on each output.
#[derive(Debug)]
pub(crate) struct ConfigBanner {
    /// What could not be used of the configuration files, and was left out:
    /// empty when all of it is in effect.
    problems: Vec<ConfigProblem>,
    /// Whether a press on the banner is held, so its release is not a click
    /// on what is beneath.
    is_held: bool,
    /// The built-in style, which draws it.
    style: Style,
    /// The problems and outputs the banners shown were made for.
    built_for: Option<(Vec<ConfigProblem>, Vec<Area>)>,
    /// One banner on each output, while there is something to say.
    shown: Vec<Shown>,
    /// The problems the user hid the banner for.
    hidden_for: Option<Vec<ConfigProblem>>,
}

/// The banner on one output.
#[derive(Debug)]
struct Shown {
    /// Where it is, its border included.
    area: Area,
    /// How wide its border is.
    border: i32,
    /// Its text's pixels.
    texture: PanelTexture,
    /// Its border.
    outline: Outline,
}

impl Default for ConfigBanner {
    fn default() -> Self {
        Self {
            problems: Vec::new(),
            is_held: false,
            style: Style::built_in(),
            built_for: None,
            shown: Vec::new(),
            hidden_for: None,
        }
    }
}

impl ConfigBanner {
    /// How far from the top and the sides of its output the banner is.
    const MARGIN: i32 = 12;

    /// The widest the banner is, so a long line is cut rather than a
    /// screen-wide bar.
    const WIDEST: i32 = 1100;

    /// The most problems named; the rest are counted.
    const MOST_NAMED: usize = 6;

    /// Returns the problems the configuration has.
    pub(crate) fn problems(&self) -> &[ConfigProblem] {
        &self.problems
    }

    /// Takes the `problems` the configuration has now.
    pub(crate) fn set_problems(&mut self, problems: Vec<ConfigProblem>) {
        self.problems = problems;
    }

    /// Brings the banners up to date with the problems the configuration
    /// has and the `outputs` there are.
    pub(crate) fn prepare(&mut self, outputs: &[Area], typesetter: &mut Typesetter) {
        let problems = self.problems.clone();
        let problems = problems.as_slice();
        if problems.is_empty() {
            // Fixed: the next problem is news again.
            self.hidden_for = None;
        }
        if problems.is_empty() || self.hidden_for.as_deref() == Some(problems) {
            self.shown.clear();
            self.built_for = None;
            return;
        }
        let wanted = (problems.to_vec(), outputs.to_vec());
        if self.built_for.as_ref() == Some(&wanted) {
            return;
        }
        self.shown = outputs
            .iter()
            .map(|output| self.make(problems, *output, typesetter))
            .collect();
        self.built_for = Some(wanted);
    }

    /// Returns what the banner says: a line of summary, a line for each of
    /// the first problems, the number left, and how to be rid of it.
    fn lines(problems: &[ConfigProblem]) -> Vec<String> {
        let count = problems.len();
        let mut lines = vec![format!(
            "vaht: {count} problem{} in your configuration; what could be used is in effect",
            if count == 1 { "" } else { "s" }
        )];
        lines.extend(
            problems
                .iter()
                .take(Self::MOST_NAMED)
                .map(|problem| format!("- {}", problem.brief())),
        );
        if count > Self::MOST_NAMED {
            lines.push(format!(
                "...and {} more: vaht config files lists them all",
                count - Self::MOST_NAMED
            ));
        }
        lines.push("Fix the files and vaht reloads them. Click here to hide this.".to_owned());
        lines
    }

    /// Returns `text`, cut and ended with `...` if it is wider than `width`
    /// pixels in `font`.
    fn fitted(
        text: &str,
        width: u32,
        font: &vaht_ui::text::Font,
        typesetter: &mut Typesetter,
    ) -> String {
        let text = text.replace(['\n', '\r'], " ");
        if typesetter.text_width(&text, font) <= width {
            return text;
        }
        let mut kept: Vec<char> = text.chars().collect();
        while !kept.is_empty() {
            kept.pop();
            let cut = format!("{}...", kept.iter().collect::<String>());
            if typesetter.text_width(&cut, font) <= width {
                return cut;
            }
        }
        String::new()
    }

    /// Makes the banner for `output`.
    fn make(&self, problems: &[ConfigProblem], output: Area, typesetter: &mut Typesetter) -> Shown {
        let style = &self.style;
        let warning = style.element_style(StyleElement::Warning, WidgetState::Focused);
        let border = i32::try_from(warning.border.width).unwrap_or(0);
        let outer_width = (output.size.w - 2 * Self::MARGIN).clamp(1, Self::WIDEST);
        let width = u32::try_from((outer_width - 2 * border).max(1)).unwrap_or(1);
        let text_width = width.saturating_sub(warning.padding.horizontal());
        let lines: Vec<String> = Self::lines(problems)
            .iter()
            .map(|line| Self::fitted(line, text_width, &warning.text.font, typesetter))
            .collect();
        let row_height = typesetter.line_height(&warning.text.font) + warning.padding.vertical();
        let height = u32::try_from(lines.len())
            .unwrap_or(u32::MAX)
            .saturating_mul(row_height);
        let outer_height = i32::try_from(height).unwrap_or(i32::MAX) + 2 * border;
        let x = output.loc.x + (output.size.w - outer_width) / 2;
        let area = Rectangle::new(
            (x, output.loc.y + Self::MARGIN).into(),
            (outer_width, outer_height).into(),
        );
        let corners = warning.corner_radii();
        let mut panel = Panel::new(
            Lines {
                texts: lines,
                line_height: row_height,
                element: StyleElement::Warning,
            },
            vaht_ui::Size::new(width, height),
        );
        panel.set_corners(corners.inset(warning.border.width));
        panel.paint(style, typesetter);
        let mut texture = PanelTexture::new();
        texture.upload(&panel);
        let mut outline = Outline::default();
        outline.update_as(
            Some(area),
            style,
            StyleElement::Warning,
            (corners, panel.scale()),
        );
        Shown {
            area,
            border,
            texture,
            outline,
        }
    }

    /// Handles a press at `point`: if it is on a banner, hides the banners
    /// until the problems they name change; returns whether it was, in which
    /// case the press goes no further, nor does its release.
    pub(crate) fn press(&mut self, point: Point<f64, Logical>) -> bool {
        let is_on_banner = self
            .shown
            .iter()
            .any(|shown| shown.area.to_f64().contains(point));
        if is_on_banner {
            self.hidden_for = Some(self.problems.clone());
            self.shown.clear();
            self.built_for = None;
            self.is_held = true;
        }
        is_on_banner
    }

    /// Handles a release; returns whether it ends a press the banner took.
    pub(crate) fn release(&mut self) -> bool {
        std::mem::take(&mut self.is_held)
    }

    /// Returns the elements that draw the banners, topmost first.
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: SceneRenderer,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements = Vec::new();
        for shown in &self.shown {
            elements.extend(shown.outline.render_elements(renderer, scale, 1.0));
            let inside = shown.area.loc + Point::from((shown.border, shown.border));
            elements.extend(shown.texture.render_element(renderer, inside, scale, 1.0));
        }
        elements
    }
}
