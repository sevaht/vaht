//! Fluxbox styles to vaht styles (02 §10).
//!
//! [`StyleFile`] reads a style as Fluxbox does (fluxland's `style.c` is the
//! reference): each vaht element in each state gets what the style gives
//! the Fluxbox resources behind it, through the same fallback chains
//! (`window.label.focus.font`, then `window.font`). The vaht style is then
//! written as the fewest layers that give every element exactly that: a
//! value most elements share goes in `[default]`, one a state shares in
//! `[state.<state>]`, and so on down to `[<element>.<state>]`. Colors keep
//! the names the style used.
//!
//! What Fluxbox draws that vaht does not yet (pixmaps, interlacing, some
//! gradients, menus, rounded corners) is reported, as is every style line
//! nothing used.

mod database;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};
use vaht_ui::{Color, Corner, StyleElement, WidgetState};

use self::database::StyleDatabase;
use crate::report::{Reason, Report};

/// A property of an element's style in a vaht style file, as the converter writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, strum::EnumIter)]
enum Property {
    /// `texture`.
    Texture,
    /// `font`.
    Font,
    /// `text_effect`.
    TextEffect,
    /// `text_color`.
    TextColor,
    /// `glyph_color`.
    GlyphColor,
    /// `glyph_images`.
    GlyphImages,
    /// `justify`.
    Justify,
    /// `padding`.
    Padding,
    /// `spacing`.
    Spacing,
    /// `border`.
    Border,
    /// `height`.
    Height,
    /// `corner_radius`.
    CornerRadius,
    /// `corners`.
    Corners,
    /// `bullet`.
    Bullet,
    /// `bullet_position`.
    BulletPosition,
    /// `alpha`.
    Alpha,
}

impl Property {
    /// Returns whether the property is an element's own geometry (its
    /// padding, border and bar height), written only for the element, never
    /// as a default or for a state, so no other element takes it on.
    const fn is_own_geometry(self) -> bool {
        matches!(
            self,
            Self::Padding
                | Self::Spacing
                | Self::Border
                | Self::Height
                | Self::CornerRadius
                | Self::Corners
                | Self::Bullet
                | Self::BulletPosition
                | Self::Alpha
        )
    }

    /// Returns the property's key in vaht style files.
    const fn key(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Font => "font",
            Self::TextEffect => "text_effect",
            Self::TextColor => "text_color",
            Self::GlyphColor => "glyph_color",
            Self::GlyphImages => "glyph_images",
            Self::Justify => "justify",
            Self::Padding => "padding",
            Self::Spacing => "spacing",
            Self::Border => "border",
            Self::Height => "height",
            Self::CornerRadius => "corner_radius",
            Self::Corners => "corners",
            Self::Bullet => "bullet",
            Self::BulletPosition => "bullet_position",
            Self::Alpha => "alpha",
        }
    }
}

/// A property's value as written in TOML, compared by its text.
#[derive(Debug, Clone)]
struct Written(Value);

impl Written {
    /// Returns a texture with its bevel the other way: raised for sunken,
    /// sunken for raised.
    fn with_bevel_reversed(&self) -> Self {
        let mut value = self.0.clone();
        if let Some(texture) = value.as_inline_table_mut() {
            let reversed = match texture.get("bevel").and_then(Value::as_str) {
                Some("raised") => Some("sunken"),
                Some("sunken") => Some("raised"),
                _ => None,
            };
            if let Some(bevel) = reversed {
                texture.insert("bevel", Value::from(bevel));
            }
        }
        Self(value)
    }

    /// Returns the text the value is written as.
    fn text(&self) -> String {
        self.0.to_string()
    }
}

impl PartialEq for Written {
    fn eq(&self, other: &Self) -> bool {
        self.text() == other.text()
    }
}

/// What the style gives one element in one state: a value per property the
/// element uses. A property it has no use for is absent, and any layer's
/// value suits it.
type Cell = BTreeMap<Property, Written>;

/// Where a texture comes from.
#[derive(Debug, Clone)]
enum TextureSource {
    /// Its own value.
    Own(Written),
    /// Its parent's (Fluxbox's `ParentRelative`).
    Parent,
}

/// The Fluxbox resources behind one vaht element in one state.
#[derive(Debug, Clone, Copy)]
struct Sources {
    /// The texture's resource names, the first the style has winning.
    texture: &'static [&'static str],
    /// The element whose texture `ParentRelative` means.
    parent: Option<StyleElement>,
    /// The font's names.
    font: Option<&'static [&'static str]>,
    /// The text color's names.
    text_color: Option<&'static [&'static str]>,
    /// The picture color's names.
    glyph_color: Option<&'static [&'static str]>,
    /// The justification's names.
    justify: Option<&'static [&'static str]>,
    /// The space vaht keeps around the element's content, which Fluxbox
    /// implies by its own drawing rather than says: top, right, bottom,
    /// left.
    padding: Option<[u32; 4]>,
    /// Whether the element has no texture of its own, and what is behind
    /// it shows: a menu's items, on the frame.
    shows_parent: bool,
    /// Whether Fluxbox styles say nothing of the element, so it keeps what
    /// vaht's built-in style gives it: a dialog's line.
    is_built_in: bool,
}

impl Sources {
    /// Sources with only a texture.
    const fn texture(texture: &'static [&'static str]) -> Self {
        Self {
            texture,
            parent: None,
            font: None,
            text_color: None,
            glyph_color: None,
            justify: None,
            padding: None,
            shows_parent: false,
            is_built_in: false,
        }
    }

    /// Returns the resources behind `element` in `state`. Where Fluxbox
    /// has no such state (a pressed label), the focused one stands in.
    #[expect(
        clippy::too_many_lines,
        reason = "one table of every element's resources"
    )]
    fn of(element: StyleElement, state: WidgetState) -> Self {
        use WidgetState::{Disabled, Focused, Hovered, Pressed, Unfocused};
        if matches!(
            element,
            StyleElement::MenuTitle | StyleElement::MenuFrame | StyleElement::MenuItem
        ) {
            return Self::of_menu(element, state);
        }
        // Only menus have these states in Fluxbox; elsewhere the focused one
        // stands in.
        let state = match state {
            Hovered | Disabled => Focused,
            Focused | Unfocused | Pressed => state,
        };
        let window = |focus: &'static [&'static str], unfocus: &'static [&'static str]| match state
        {
            Focused | Pressed | Hovered | Disabled => focus,
            Unfocused => unfocus,
        };
        match element {
            StyleElement::Titlebar => {
                Self::texture(window(&["window.title.focus"], &["window.title.unfocus"]))
            }
            StyleElement::Label => Self {
                texture: window(
                    &["window.label.focus", "toolbar.windowLabel"],
                    &["window.label.unfocus", "toolbar.windowLabel"],
                ),
                parent: Some(StyleElement::Titlebar),
                font: Some(window(
                    &["window.label.focus.font", "window.font"],
                    &["window.label.unfocus.font", "window.font"],
                )),
                text_color: Some(window(
                    &["window.label.focus.textColor"],
                    &["window.label.unfocus.textColor"],
                )),
                justify: Some(window(
                    &[
                        "window.label.focus.justify",
                        "window.label.justify",
                        "window.justify",
                    ],
                    &[
                        "window.label.unfocus.justify",
                        "window.label.justify",
                        "window.justify",
                    ],
                )),
                padding: Some([0, 4, 0, 4]),
                ..Self::texture(&[])
            },
            StyleElement::Button => Self {
                texture: match state {
                    Focused | Hovered | Disabled => &["window.button.focus"],
                    Unfocused => &["window.button.unfocus"],
                    Pressed => &["window.button.pressed"],
                },
                parent: Some(StyleElement::Titlebar),
                glyph_color: Some(match state {
                    Focused | Hovered | Disabled => &["window.button.focus.picColor"],
                    Unfocused => &["window.button.unfocus.picColor"],
                    Pressed => &[
                        "window.button.pressed.picColor",
                        "window.button.focus.picColor",
                    ],
                }),
                padding: Some([5, 5, 5, 5]),
                ..Self::texture(&[])
            },
            StyleElement::Handle => {
                Self::texture(window(&["window.handle.focus"], &["window.handle.unfocus"]))
            }
            StyleElement::Grip => {
                Self::texture(window(&["window.grip.focus"], &["window.grip.unfocus"]))
            }
            StyleElement::Border => Self::texture(&[]),
            StyleElement::Taskbar => Self::texture(&["toolbar"]),
            StyleElement::TaskbarButton => Self {
                texture: match state {
                    Pressed => &["toolbar.button.pressed"],
                    Focused | Unfocused | Hovered | Disabled => {
                        &["toolbar.button", "toolbar.clock"]
                    }
                },
                parent: Some(StyleElement::Taskbar),
                glyph_color: Some(match state {
                    Pressed => &[
                        "toolbar.button.pressed.picColor",
                        "toolbar.button.picColor",
                        "toolbar.clock.picColor",
                        "toolbar.clock.textColor",
                        "toolbar.textColor",
                    ],
                    Focused | Unfocused | Hovered | Disabled => &[
                        "toolbar.button.picColor",
                        "toolbar.clock.picColor",
                        "toolbar.clock.textColor",
                        "toolbar.textColor",
                    ],
                }),
                // The arrow is two fifths of the button (Fluxbox's
                // toolbar.button.scale, 250), drawn in the whole button.
                padding: Some([0, 0, 0, 0]),
                ..Self::texture(&[])
            },
            StyleElement::TaskbarWorkspace => Self {
                texture: &["toolbar.workspace", "toolbar.label"],
                parent: Some(StyleElement::Taskbar),
                font: Some(&["toolbar.workspace.font", "toolbar.font"]),
                text_color: Some(&[
                    "toolbar.workspace.textColor",
                    "toolbar.label.textColor",
                    "toolbar.textColor",
                ]),
                justify: Some(&["toolbar.workspace.justify", "toolbar.justify"]),
                // Fluxbox's workspace name tool: the widest name plus 2.
                padding: Some([0, 1, 0, 1]),
                ..Self::texture(&[])
            },
            StyleElement::TaskbarClock => Self {
                texture: &["toolbar.clock"],
                parent: Some(StyleElement::Taskbar),
                font: Some(&["toolbar.clock.font", "toolbar.font"]),
                text_color: Some(&["toolbar.clock.textColor", "toolbar.textColor"]),
                justify: Some(&["toolbar.clock.justify", "toolbar.justify"]),
                padding: Some([0, 8, 0, 8]),
                ..Self::texture(&[])
            },
            StyleElement::WindowListEntry => Self {
                texture: match state {
                    Focused | Pressed | Hovered | Disabled => {
                        &["toolbar.iconbar.focused", "toolbar.windowLabel"]
                    }
                    Unfocused => &["toolbar.iconbar.unfocused", "toolbar.windowLabel"],
                },
                parent: Some(StyleElement::Taskbar),
                font: Some(match state {
                    Focused | Pressed | Hovered | Disabled => &[
                        "toolbar.iconbar.focused.font",
                        "window.font",
                        "toolbar.font",
                    ],
                    Unfocused => &[
                        "toolbar.iconbar.unfocused.font",
                        "window.font",
                        "toolbar.font",
                    ],
                }),
                text_color: Some(match state {
                    Focused | Pressed | Hovered | Disabled => &[
                        "toolbar.iconbar.focused.textColor",
                        "toolbar.windowLabel.textColor",
                        "toolbar.textColor",
                    ],
                    Unfocused => &[
                        "toolbar.iconbar.unfocused.textColor",
                        "toolbar.windowLabel.textColor",
                        "toolbar.textColor",
                    ],
                }),
                justify: Some(match state {
                    Focused | Pressed | Hovered | Disabled => &[
                        "toolbar.iconbar.focused.justify",
                        "toolbar.iconbar.justify",
                        "window.justify",
                    ],
                    Unfocused => &[
                        "toolbar.iconbar.unfocused.justify",
                        "toolbar.iconbar.justify",
                        "window.justify",
                    ],
                }),
                // Half of iconbar.iconTextPadding (10 by default) each side
                // of the title, as Fluxbox lays an entry out.
                padding: Some([0, 5, 0, 5]),
                ..Self::texture(&[])
            },
            StyleElement::MenuTitle | StyleElement::MenuFrame | StyleElement::MenuItem => {
                Self::of_menu(element, state)
            }
            StyleElement::DialogField
            | StyleElement::Wireframe
            | StyleElement::Tooltip
            | StyleElement::Warning => Self {
                is_built_in: true,
                ..Self::texture(&[])
            },
            StyleElement::WindowListEmpty => Self {
                texture: &[
                    "toolbar.iconbar.empty",
                    "toolbar.iconbar",
                    "toolbar.windowLabel",
                    "toolbar",
                ],
                parent: Some(StyleElement::Taskbar),
                ..Self::texture(&[])
            },
        }
    }

    /// Returns the resources behind a menu `element` in `state`, as
    /// fluxland's `load_menu` reads them: `menu.hilite` for the hovered
    /// item, `menu.frame.disableColor` for a disabled one, and the frame
    /// showing behind the others.
    fn of_menu(element: StyleElement, state: WidgetState) -> Self {
        let is_hovered = state == WidgetState::Hovered;
        match element {
            StyleElement::MenuTitle => Self {
                font: Some(&["menu.title.font"]),
                text_color: Some(&["menu.title.textColor"]),
                justify: Some(&["menu.title.justify"]),
                ..Self::texture(&["menu.title"])
            },
            StyleElement::MenuItem if is_hovered => Self {
                font: Some(&["menu.hilite.font", "menu.frame.font"]),
                text_color: Some(&["menu.hilite.textColor"]),
                // A chosen toggle's square is the highlight's color.
                glyph_color: Some(&["menu.hilite.color"]),
                justify: Some(&["menu.hilite.justify", "menu.frame.justify"]),
                parent: Some(StyleElement::MenuFrame),
                ..Self::texture(&["menu.hilite"])
            },
            StyleElement::MenuItem => Self {
                font: Some(&["menu.frame.font"]),
                glyph_color: Some(&["menu.hilite.color"]),
                text_color: Some(if state == WidgetState::Disabled {
                    &["menu.frame.disableColor"]
                } else {
                    &["menu.frame.textColor"]
                }),
                justify: Some(&["menu.frame.justify"]),
                shows_parent: true,
                ..Self::texture(&[])
            },
            StyleElement::MenuFrame
            | StyleElement::Titlebar
            | StyleElement::Label
            | StyleElement::Button
            | StyleElement::Handle
            | StyleElement::Grip
            | StyleElement::Border
            | StyleElement::Taskbar
            | StyleElement::TaskbarButton
            | StyleElement::TaskbarWorkspace
            | StyleElement::TaskbarClock
            | StyleElement::WindowListEntry
            | StyleElement::WindowListEmpty
            | StyleElement::DialogField
            | StyleElement::Wireframe
            | StyleElement::Tooltip
            | StyleElement::Warning => Self {
                font: Some(&["menu.frame.font"]),
                text_color: Some(&["menu.frame.textColor"]),
                justify: Some(&["menu.frame.justify"]),
                ..Self::texture(&["menu.frame"])
            },
        }
    }
}

/// The style's lines that vaht leaves out on purpose or cannot use yet, by
/// how their names start, with why.
const UNUSED_REASONS: [(&str, Reason, &str); 6] = [
    ("menu", Reason::NotYetInVaht, "not in vaht's menus yet"),
    ("slit", Reason::Dropped, "the slit is gone"),
    ("window.tab", Reason::Dropped, "tabs are gone"),
    (
        "background",
        Reason::Dropped,
        "styles cannot set the wallpaper",
    ),
    (
        "rootcommand",
        Reason::Dropped,
        "styles cannot set the wallpaper",
    ),
    (
        "toolbar.shaped",
        Reason::NotYetInVaht,
        "a taskbar narrower than its edge, whose corners could be rounded",
    ),
];

/// A Fluxbox style being converted.
#[derive(Debug)]
pub(crate) struct StyleFile {
    /// Its lines.
    database: StyleDatabase,
    /// The name the converted style gets: the directory of its images.
    name: String,
    /// The images its textures use, found beside it, as the converted style
    /// names them.
    images: Vec<StyleImage>,
}

/// An image a converted style draws with, to be written beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StyleImage {
    /// Its path in the styles directory, as the style names it:
    /// `<style>/<file>`.
    pub(crate) name: String,
    /// The XPM text.
    pub(crate) text: String,
}

impl StyleFile {
    /// The radius that rounds a corner as Fluxbox's corner cut does: its
    /// 8-by-8 shape cuts 14 pixels away, as much as a circle of radius 8
    /// leaves outside it (8² · (1 − π/4) ≈ 13.7).
    const FLUXBOX_CORNER_RADIUS: u32 = 8;

    /// Resources whose value asks for what vaht draws anyway: each name,
    /// and that value.
    const WHAT_VAHT_DRAWS: [(&str, &str); 4] = [
        // The taskbar's corners are square.
        ("toolbar.shaped", "false"),
        // No bevel inside frames, the taskbar or menus.
        ("window.bevelWidth", "0"),
        // The window list's entries have no outline.
        ("toolbar.iconbar.borderWidth", "0"),
        // Text has no effect unless a vaht style gives it one.
        ("window.font.effect", "none"),
    ];

    /// Converts the style `text`, read from `path`.
    pub(crate) fn new(path: &Path, text: &str) -> Self {
        Self {
            database: StyleDatabase::parse(path, text),
            name: crate::style_name(path),
            images: Vec::new(),
        }
    }

    /// Converts the style to a vaht style file, noting in `report` what it
    /// leaves out.
    pub(crate) fn convert(mut self, report: &mut Report) -> (DocumentMut, Vec<StyleImage>) {
        let cells = self.cells(report);
        self.report_unused(report);
        (Layers::fitting(&cells).document(), self.images)
    }

    /// Returns what the style gives every element in every state.
    fn cells(&mut self, report: &mut Report) -> Vec<(StyleElement, WidgetState, Cell)> {
        let mut cells = Vec::new();
        let mut tooltip: Option<Cell> = None;
        let mut textures: HashMap<(StyleElement, WidgetState), Written> = HashMap::new();
        // Parents first, so ParentRelative finds their textures.
        // The warning has no Fluxbox source, and vaht draws it from the
        // built-in style whatever the style says, so a style does not carry it.
        let mut elements: Vec<StyleElement> = <StyleElement as strum::IntoEnumIterator>::iter()
            .filter(|element| *element != StyleElement::Warning)
            .collect();
        elements
            .sort_by_key(|element| Sources::of(*element, WidgetState::Focused).parent.is_some());
        for element in elements {
            for state in <WidgetState as strum::IntoEnumIterator>::iter() {
                let sources = Sources::of(element, state);
                if element == StyleElement::Tooltip {
                    // The same in every state, read (and noted) once.
                    let cell = match &tooltip {
                        Some(cell) => Clone::clone(cell),
                        None => self.tooltip_cell(report),
                    };
                    tooltip = Some(cell.clone());
                    cells.push((element, state, cell));
                    continue;
                }
                if sources.is_built_in {
                    cells.push((element, state, Self::built_in_cell(element)));
                    continue;
                }
                let mut cell = Cell::new();
                if sources.shows_parent {
                    cell.insert(Property::Texture, Written(Value::from("parent")));
                }
                if !sources.texture.is_empty() {
                    // Without its own, a pressed taskbar button is the
                    // button with its bevel the other way, as in Fluxbox.
                    let is_unstyled_press = element == StyleElement::TaskbarButton
                        && state == WidgetState::Pressed
                        && self.database.first_given(sources.texture).is_none();
                    let texture = if is_unstyled_press {
                        textures
                            .get(&(element, WidgetState::Focused))
                            .map(Written::with_bevel_reversed)
                    } else {
                        match self.texture(sources.texture, report) {
                            TextureSource::Own(texture) => Some(texture),
                            TextureSource::Parent => sources
                                .parent
                                .and_then(|parent| textures.get(&(parent, state)).cloned()),
                        }
                    };
                    if let Some(texture) = texture {
                        textures.insert((element, state), texture.clone());
                        cell.insert(Property::Texture, texture);
                    }
                }
                if let Some(names) = sources.font {
                    self.add_font(&mut cell, names, report);
                }
                let white = || "white".to_owned();
                if let Some(names) = sources.text_color {
                    let color = self.color_chain(names, report).unwrap_or_else(white);
                    cell.insert(Property::TextColor, Written(Value::from(color)));
                }
                if let Some(names) = sources.glyph_color {
                    let color = self.color_chain(names, report).unwrap_or_else(white);
                    cell.insert(Property::GlyphColor, Written(Value::from(color)));
                }
                if element == StyleElement::Button {
                    self.add_glyph_images(&mut cell, state);
                }
                if element == StyleElement::MenuItem {
                    self.add_menu_images(&mut cell, state);
                }
                if let Some(names) = sources.justify {
                    let justify = self.justify(names);
                    cell.insert(Property::Justify, Written(Value::from(justify)));
                }
                if let Some(padding) = sources.padding {
                    let array: Array = padding.iter().map(|side| i64::from(*side)).collect();
                    cell.insert(Property::Padding, Written(Value::Array(array)));
                }
                self.add_sizes(&mut cell, element, state, report);
                if element == StyleElement::MenuItem {
                    self.add_bullet(&mut cell);
                }
                cells.push((element, state, cell));
            }
        }
        for (name, value) in Self::WHAT_VAHT_DRAWS {
            self.database.accept(name, value);
        }
        cells
    }

    /// Adds to `cell` the padding and spacing of `element` that Fluxbox's
    /// bevel width asks, for a bar.
    fn add_bevel(&mut self, cell: &mut Cell, element: StyleElement) {
        // Fluxbox's bevel width keeps a bar's parts that far from its edges and
        // from each other: vaht's padding and spacing of the bar.
        let bevel = if element == StyleElement::Titlebar {
            self.number(&["window.bevelWidth"])
        } else if element == StyleElement::Taskbar {
            self.number(&["toolbar.bevelWidth"])
        } else {
            None
        };
        if let Some(bevel) = bevel.filter(|bevel| *bevel > 0) {
            let sides: Array = [bevel; 4].into_iter().map(i64::from).collect();
            cell.insert(Property::Padding, Written(Value::Array(sides)));
            cell.insert(Property::Spacing, Written(Value::from(i64::from(bevel))));
        }
    }

    /// Adds to `cell` the padding of a titlebar button that leaves its
    /// picture the size Fluxbox draws it, about ten pixels, whatever the
    /// button's size: the titlebar's height less its bevel at both ends.
    fn add_button_padding(&mut self, cell: &mut Cell, element: StyleElement) {
        /// How big Fluxbox draws a titlebar button's picture, in pixels.
        const PICTURE: u32 = 10;
        if element != StyleElement::Button {
            return;
        }
        let height = self
            .number(&["window.title.height"])
            .filter(|height| *height > 0)
            .unwrap_or(vaht_ui::Style::DEFAULT_BAR_HEIGHT);
        let bevel = self.number(&["window.bevelWidth"]).unwrap_or(0);
        let side = height.saturating_sub(2 * bevel);
        let padding = i64::from(side.saturating_sub(PICTURE) / 2);
        let sides: Array = [padding; 4].into_iter().collect();
        cell.insert(Property::Padding, Written(Value::Array(sides)));
    }

    /// Adds to `cell` the border of `element` in `state`, where it has one.
    fn add_border(
        &mut self,
        cell: &mut Cell,
        element: StyleElement,
        state: WidgetState,
        report: &mut Report,
    ) {
        let border: Option<(&[&str], &[&str])> = match (element, state) {
            (StyleElement::Border, WidgetState::Focused | WidgetState::Pressed) => Some((
                &["window.focus.borderWidth", "window.borderWidth"],
                &["window.focus.borderColor", "window.borderColor"],
            )),
            (StyleElement::Border, WidgetState::Unfocused) => Some((
                &["window.unfocus.borderWidth", "window.borderWidth"],
                &["window.unfocus.borderColor", "window.borderColor"],
            )),
            (StyleElement::Taskbar, _) => {
                Some((&["toolbar.borderWidth"], &["toolbar.borderColor"]))
            }
            (StyleElement::WindowListEntry, WidgetState::Unfocused) => Some((
                &[
                    "toolbar.iconbar.unfocused.borderWidth",
                    "toolbar.iconbar.borderWidth",
                ],
                &[
                    "toolbar.iconbar.unfocused.borderColor",
                    "toolbar.iconbar.borderColor",
                ],
            )),
            (StyleElement::WindowListEntry, _) => Some((
                &[
                    "toolbar.iconbar.focused.borderWidth",
                    "toolbar.iconbar.borderWidth",
                ],
                &[
                    "toolbar.iconbar.focused.borderColor",
                    "toolbar.iconbar.borderColor",
                ],
            )),
            (StyleElement::MenuFrame, _) => Some((&["menu.borderWidth"], &["menu.borderColor"])),
            _ => None,
        };
        if let Some((widths, colors)) = border {
            let width = self.number(widths).unwrap_or(0);
            // Most window-list entries have no line, and need not say so: unless
            // the other state's has one, which this state then does without.
            if width == 0
                && element == StyleElement::WindowListEntry
                && !self.window_list_has_a_border()
            {
                return;
            }
            let color = self
                .color_chain(colors, report)
                .unwrap_or_else(|| "black".to_owned());
            let mut border = InlineTable::new();
            border.insert("width", Value::from(i64::from(width)));
            border.insert("color", Value::from(color));
            cell.insert(Property::Border, Written(Value::InlineTable(border)));
        }
    }

    /// Returns whether the style gives window-list entries a border in any
    /// state.
    fn window_list_has_a_border(&mut self) -> bool {
        [
            [
                "toolbar.iconbar.focused.borderWidth",
                "toolbar.iconbar.borderWidth",
            ],
            [
                "toolbar.iconbar.unfocused.borderWidth",
                "toolbar.iconbar.borderWidth",
            ],
        ]
        .iter()
        .any(|names| self.number(names).is_some_and(|width| width > 0))
    }

    /// Adds to `cell` the border and the bar height `element` has in
    /// `state`, where it has them.
    fn add_sizes(
        &mut self,
        cell: &mut Cell,
        element: StyleElement,
        state: WidgetState,
        report: &mut Report,
    ) {
        let height = match element {
            StyleElement::Titlebar => self
                .number(&["window.title.height"])
                .filter(|height| *height > 0),
            StyleElement::Handle => self.number(&["window.handleWidth"]),
            StyleElement::Taskbar => self
                .number(&["toolbar.height"])
                .filter(|height| *height > 0),
            StyleElement::MenuTitle => self
                .number(&["menu.titleHeight"])
                .filter(|height| *height > 0),
            StyleElement::MenuItem => {
                let height = self
                    .number(&["menu.itemHeight"])
                    .filter(|height| *height > 0);
                // Fluxbox keeps an item-high square each side of the text:
                // the bullet's on the right, a pixel more on the left.
                let side = height.unwrap_or(vaht_ui::Style::DEFAULT_BAR_HEIGHT);
                let padding: Array = [0, side, 0, side + 1].into_iter().map(i64::from).collect();
                cell.insert(Property::Padding, Written(Value::Array(padding)));
                height
            }
            StyleElement::MenuFrame
            | StyleElement::Label
            | StyleElement::Button
            | StyleElement::Grip
            | StyleElement::Border
            | StyleElement::TaskbarButton
            | StyleElement::TaskbarWorkspace
            | StyleElement::TaskbarClock
            | StyleElement::WindowListEntry
            | StyleElement::WindowListEmpty
            | StyleElement::DialogField
            | StyleElement::Wireframe
            | StyleElement::Tooltip
            | StyleElement::Warning => None,
        };
        if let Some(height) = height {
            cell.insert(Property::Height, Written(Value::from(i64::from(height))));
        }
        self.add_bevel(cell, element);
        self.add_button_padding(cell, element);
        self.add_border(cell, element, state, report);
        match element {
            StyleElement::Border => self.add_round_corners(cell, "window.roundCorners"),
            StyleElement::MenuFrame => self.add_round_corners(cell, "menu.roundCorners"),
            StyleElement::Titlebar
            | StyleElement::Label
            | StyleElement::Button
            | StyleElement::Handle
            | StyleElement::Grip
            | StyleElement::Taskbar
            | StyleElement::TaskbarButton
            | StyleElement::TaskbarWorkspace
            | StyleElement::TaskbarClock
            | StyleElement::WindowListEntry
            | StyleElement::WindowListEmpty
            | StyleElement::MenuTitle
            | StyleElement::MenuItem
            | StyleElement::DialogField
            | StyleElement::Wireframe
            | StyleElement::Tooltip
            | StyleElement::Warning => {}
        }
    }

    /// Returns the tooltip's style: fluxland's tray tooltip, as the
    /// built-in style has it, with what the Fluxbox style's `systray.tooltip.*`
    /// lines say instead.
    fn tooltip_cell(&mut self, report: &mut Report) -> Cell {
        let mut cell = Self::built_in_cell(StyleElement::Tooltip);
        if let Some(color) = self.color("systray.tooltip.color", report) {
            let mut texture = InlineTable::new();
            texture.insert("solid", Value::from(color));
            cell.insert(Property::Texture, Written(Value::InlineTable(texture)));
        }
        if let Some(color) = self.color("systray.tooltip.textColor", report) {
            cell.insert(Property::TextColor, Written(Value::from(color)));
        }
        if self.database.get("systray.tooltip.font").is_some() {
            self.add_font(&mut cell, &["systray.tooltip.font"], report);
        }
        if let Some(color) = self.color("systray.tooltip.borderColor", report) {
            let mut border = InlineTable::new();
            border.insert("width", Value::from(1));
            border.insert("color", Value::from(color));
            cell.insert(Property::Border, Written(Value::InlineTable(border)));
        }
        let built_in = [8, 14, 8, 13];
        let sides = ["Top", "Right", "Bottom", "Left"];
        let padding: Array = sides
            .iter()
            .zip(built_in)
            .map(|(side, default)| {
                let name = format!("systray.tooltip.padding{side}");
                i64::from(self.number(&[name.as_str()]).unwrap_or(default))
            })
            .collect();
        cell.insert(Property::Padding, Written(Value::Array(padding)));
        if let Some(radius) = self.number(&["systray.tooltip.radius"]) {
            cell.insert(
                Property::CornerRadius,
                Written(Value::from(i64::from(radius))),
            );
        }
        if let Some(alpha) = self
            .number(&["systray.tooltip.alpha"])
            .filter(|alpha| *alpha < 255)
        {
            cell.insert(
                Property::Alpha,
                Written(Value::from(i64::from(alpha.clamp(0, 255)))),
            );
        }
        cell
    }

    /// Returns what vaht's built-in style says of `element` in its own
    /// section, for an element Fluxbox styles say nothing of.
    fn built_in_cell(element: StyleElement) -> Cell {
        let vaht_style: DocumentMut = vaht_ui::Style::BUILT_IN
            .parse()
            .expect("the built-in style is valid TOML (tested in vaht-ui)");
        let section = element
            .path()
            .split('.')
            .try_fold(vaht_style.as_item(), |item, key| item.get(key));
        <Property as strum::IntoEnumIterator>::iter()
            .filter_map(|property| {
                let mut value = section?.get(property.key())?.as_value()?.clone();
                value.decor_mut().clear();
                Some((property, Written(value)))
            })
            .collect()
    }

    /// Adds a menu item's bullet to `cell`: `menu.bullet` (a word found
    /// anywhere in it; none by default) and its side, `menu.bullet.position`
    /// (left unless it says right), as fluxland reads them.
    fn add_bullet(&mut self, cell: &mut Cell) {
        let bullet = self
            .database
            .get("menu.bullet")
            .map(str::to_ascii_lowercase)
            .and_then(|value| {
                ["empty", "square", "triangle", "diamond"]
                    .into_iter()
                    .find(|word| value.contains(word))
            })
            .unwrap_or("empty");
        let is_right = self
            .database
            .get("menu.bullet.position")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("right"));
        cell.insert(Property::Bullet, Written(Value::from(bullet)));
        let side = if is_right { "right" } else { "left" };
        cell.insert(Property::BulletPosition, Written(Value::from(side)));
    }

    /// Adds the `resource` (`window.roundCorners`, `menu.roundCorners`) to
    /// an outline's `cell`: Fluxbox's cut, as a radius, and the corners it
    /// names when not all four.
    fn add_round_corners(&mut self, cell: &mut Cell, resource: &str) {
        let Some(names) = self.database.get(resource) else {
            return;
        };
        // Fluxbox finds each corner's name anywhere in the value, in any
        // case (fluxland's parse_round_corners).
        let names = names.to_ascii_lowercase();
        let corners: Vec<Corner> = <Corner as strum::IntoEnumIterator>::iter()
            .filter(|corner| names.contains(&corner.name().replace('-', "")))
            .collect();
        if corners.is_empty() {
            return;
        }
        cell.insert(
            Property::CornerRadius,
            Written(Value::from(i64::from(Self::FLUXBOX_CORNER_RADIUS))),
        );
        if corners.len() < 4 {
            let names: Array = corners.iter().map(|corner| corner.name()).collect();
            cell.insert(Property::Corners, Written(Value::Array(names)));
        }
    }

    /// Returns the texture the first of `names` the style has gives, with
    /// its colors: Fluxbox's `darkgray` and `white` where it gives none.
    fn texture(&mut self, names: &[&str], report: &mut Report) -> TextureSource {
        let name = self.database.first_given(names).unwrap_or(names[0]);
        let spec = self
            .database
            .get(name)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if spec.contains("parentrelative") || spec.contains("parent_relative") {
            return TextureSource::Parent;
        }
        // The image is the texture where the style asks for a pixmap, or asks
        // for nothing else (an empty texture beside a pixmap, as Neon's).
        let asks_for_an_image = spec.trim().is_empty() || spec.contains("pixmap");
        if let Some(image) = asks_for_an_image.then(|| self.pixmap_image(name)).flatten() {
            let mut texture = InlineTable::new();
            texture.insert("image", Value::from(image));
            return TextureSource::Own(Written(Value::InlineTable(texture)));
        }
        let from = self
            .color(&format!("{name}.color"), report)
            .unwrap_or_else(|| "darkgray".to_owned());
        let to = self
            .color(&format!("{name}.colorTo"), report)
            .unwrap_or_else(|| "white".to_owned());
        let mut texture = InlineTable::new();
        if spec.contains("gradient") {
            let direction = [
                ("crossdiagonal", "cross-diagonal"),
                ("diagonal", "diagonal"),
                ("horizontal", "horizontal"),
                ("vertical", "vertical"),
            ]
            .into_iter()
            .find(|(word, _)| spec.contains(word))
            .map_or_else(
                || {
                    self.note_resource(
                        name,
                        report,
                        "a pipecross, elliptic, rectangle or pyramid gradient, drawn diagonal",
                    );
                    "diagonal"
                },
                |(_, direction)| direction,
            );
            texture.insert("gradient", Value::from(direction));
            texture.insert("from", Value::from(from));
            texture.insert("to", Value::from(to));
        } else {
            if spec.contains("pixmap") && !spec.contains("solid") {
                self.note_resource(name, report, "a pixmap texture, drawn in its color");
            }
            texture.insert("solid", Value::from(from));
        }
        if spec.contains("interlaced") {
            self.note_resource(name, report, "interlacing");
        }
        if spec.contains("raised") {
            texture.insert("bevel", Value::from("raised"));
        } else if spec.contains("sunken") {
            texture.insert("bevel", Value::from("sunken"));
        }
        TextureSource::Own(Written(Value::InlineTable(texture)))
    }

    /// Adds to `cell` the font, and its effect, the first of `names` the
    /// style has gives: Fluxbox's `sans-10` if none.
    fn add_font(&mut self, cell: &mut Cell, names: &[&str], report: &mut Report) {
        let key = self.database.first_given(names);
        let spec = key
            .and_then(|key| self.database.get(key))
            .unwrap_or("sans-10")
            .to_owned();
        let Some(font) = FluxboxFont::parse(&spec) else {
            if let Some(key) = key {
                self.note_resource(key, report, "an X11 core font name");
            }
            return;
        };
        cell.insert(Property::Font, Written(Value::InlineTable(font.table())));
        let effect = key.map_or_else(
            || Value::from("none"),
            |key| self.effect(key, font.has_halo, report),
        );
        cell.insert(Property::TextEffect, Written(effect));
    }

    /// Returns the effect `key.effect` says, with its color and offsets.
    fn effect(&mut self, key: &str, has_halo: bool, report: &mut Report) -> Value {
        let effect = self
            .database
            .get(&format!("{key}.effect"))
            .map(str::to_ascii_lowercase);
        let mut table = InlineTable::new();
        match effect.as_deref() {
            Some("shadow") => {
                let color = self
                    .color(&format!("{key}.shadow.color"), report)
                    .unwrap_or_else(|| "black".to_owned());
                let x = self.number(&[&format!("{key}.shadow.x")]).unwrap_or(0);
                let y = self.number(&[&format!("{key}.shadow.y")]).unwrap_or(0);
                table.insert("shadow", Value::from(color));
                let offset: Array = [x, y].iter().map(|offset| i64::from(*offset)).collect();
                table.insert("offset", Value::Array(offset));
            }
            Some("halo") => {
                let color = self
                    .color(&format!("{key}.halo.color"), report)
                    .unwrap_or_else(|| "black".to_owned());
                table.insert("halo", Value::from(color));
            }
            _ if has_halo => {
                table.insert("halo", Value::from("black"));
            }
            _ => return Value::from("none"),
        }
        Value::InlineTable(table)
    }

    /// Returns the justification the first of `names` the style has gives:
    /// left if none.
    fn justify(&mut self, names: &[&str]) -> &'static str {
        let value = self
            .database
            .first_given(names)
            .and_then(|name| self.database.get(name))
            .map(str::to_ascii_lowercase);
        match value.as_deref() {
            Some("center" | "centre") => "center",
            Some("right") => "right",
            _ => "left",
        }
    }

    /// Returns the name the converted style gives the image the resource
    /// `name`'s `pixmap` names, once it is kept to be written beside the
    /// style: found beside the Fluxbox style or in its `pixmaps` directory,
    /// and an XPM vaht reads.
    fn pixmap_image(&mut self, name: &str) -> Option<String> {
        self.image_resource(&format!("{name}.pixmap"))
    }

    /// Adds to `cell` the pictures the style gives the titlebar buttons in
    /// `state` (`window.close.pixmap`, `.unfocus.pixmap`, `.pressed.pixmap`):
    /// each as the vaht glyphs it draws.
    fn add_glyph_images(&mut self, cell: &mut Cell, state: WidgetState) {
        // Fluxbox's buttons, and the vaht glyphs each is the picture of.
        const BUTTONS: [(&str, &[&str]); 8] = [
            ("close", &["close"]),
            ("iconify", &["minimize"]),
            ("maximize", &["maximize"]),
            ("shade", &["shade"]),
            ("unshade", &["unshade"]),
            ("stick", &["stick"]),
            ("stuck", &["stuck"]),
            ("menuicon", &["menu"]),
        ];
        let suffix = match state {
            WidgetState::Focused => "pixmap",
            WidgetState::Unfocused => "unfocus.pixmap",
            WidgetState::Pressed => "pressed.pixmap",
            WidgetState::Hovered | WidgetState::Disabled => return,
        };
        let mut images = InlineTable::new();
        for (button, glyphs) in BUTTONS {
            if let Some(image) = self.image_resource(&format!("window.{button}.{suffix}")) {
                for glyph in glyphs {
                    images.insert(*glyph, Value::from(image.clone()));
                }
            }
        }
        if !images.is_empty() {
            cell.insert(Property::GlyphImages, Written(Value::InlineTable(images)));
        }
    }

    /// Adds to `cell` the pictures the style gives a menu item's submenu
    /// bullet and check mark (`menu.submenu.pixmap`, `menu.selected.pixmap`;
    /// `menu.hilite.…` for the highlighted item).
    fn add_menu_images(&mut self, cell: &mut Cell, state: WidgetState) {
        let prefix = match state {
            WidgetState::Focused => "menu",
            WidgetState::Hovered => "menu.hilite",
            WidgetState::Unfocused | WidgetState::Pressed | WidgetState::Disabled => return,
        };
        let mut images = InlineTable::new();
        for (resource, glyph) in [("submenu", "submenu"), ("selected", "check")] {
            if let Some(image) = self.image_resource(&format!("{prefix}.{resource}.pixmap")) {
                images.insert(glyph, Value::from(image));
            }
        }
        if !images.is_empty() {
            cell.insert(Property::GlyphImages, Written(Value::InlineTable(images)));
        }
    }

    /// Returns the name the converted style gives the image the resource
    /// `resource` names (see [`StyleFile::pixmap_image`]).
    fn image_resource(&mut self, resource: &str) -> Option<String> {
        let file = self.database.get(resource)?.to_owned();
        let relative = Path::new(&file);
        // Only what is inside the style's own directory is carried along.
        if file.is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return None;
        }
        let directory = self.database.path().parent()?;
        let text = [
            directory.join("pixmaps").join(relative),
            directory.join(relative),
        ]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())?;
        vaht_ui::Image::from_xpm(&text).ok()?;
        let kept = format!("{}/{file}", self.name);
        if !self.images.iter().any(|image| image.name == kept) {
            self.images.push(StyleImage {
                name: kept.clone(),
                text,
            });
        }
        Some(kept)
    }

    /// Returns the color the first of `names` the style has gives.
    fn color_chain(&mut self, names: &[&str], report: &mut Report) -> Option<String> {
        let name = self.database.first_given(names)?;
        self.color(name, report)
    }

    /// Returns the color the resource `name` gives, as written, if vaht
    /// reads it; one it cannot read is reported.
    fn color(&mut self, name: &str, report: &mut Report) -> Option<String> {
        let value = self.database.get(name)?.to_owned();
        if value.parse::<Color>().is_ok() {
            return Some(value);
        }
        report.note(
            self.database.path(),
            self.database.line_of(name),
            Reason::Unreadable,
            format!("{name}: {value} is no color vaht reads"),
        );
        None
    }

    /// Returns the whole number the first of `names` the style has gives.
    fn number(&mut self, names: &[&str]) -> Option<u32> {
        let name = self.database.first_given(names).unwrap_or(names[0]);
        self.database.get(name)?.parse().ok()
    }

    /// Notes that what the resource `name` asks for is not in vaht yet.
    fn note_resource(&self, name: &str, report: &mut Report, what: &str) {
        report.note(
            self.database.path(),
            self.database.line_of(name),
            Reason::NotYetInVaht,
            format!("{name}: {what}"),
        );
    }

    /// Notes every line nothing used, with why.
    fn report_unused(&self, report: &mut Report) {
        for line in self.database.unused() {
            let name = line.name.to_ascii_lowercase();
            let (reason, why) = UNUSED_REASONS
                .iter()
                .find(|(prefix, ..)| name.starts_with(prefix))
                .map_or(
                    (Reason::NotYetInVaht, "not used by vaht's style"),
                    |(_, reason, why)| (*reason, *why),
                );
            report.note(
                self.database.path(),
                line.line,
                reason,
                format!("{}: {why}", line.name),
            );
        }
    }
}

/// A font as Fluxbox styles name it: `family-size:option:…`, or
/// `pango:Family Size`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FluxboxFont {
    /// The family, as written.
    family: String,
    /// The size in points.
    size: u16,
    /// Whether it is bold.
    is_bold: bool,
    /// Whether it is italic.
    is_italic: bool,
    /// Whether `:halo` asks for a halo, as fluxland 1.x styles did.
    has_halo: bool,
}

impl FluxboxFont {
    /// Fluxbox's size when a font names none.
    const DEFAULT_SIZE: u16 = 10;

    /// Parses `spec`; `None` for an X11 core font name (`-misc-fixed-…`).
    fn parse(spec: &str) -> Option<Self> {
        let first = spec.split('|').next().unwrap_or_default().trim();
        if first.starts_with('-') {
            return None;
        }
        if let Some(pango) = first.strip_prefix("pango:") {
            // `Family [Style…] [Size]`: style words follow the family.
            let mut words: Vec<&str> = pango.split_whitespace().collect();
            let size = words
                .last()
                .and_then(|word| word.parse().ok())
                .inspect(|_| {
                    words.pop();
                })
                .unwrap_or(Self::DEFAULT_SIZE);
            let mut is_bold = false;
            let mut is_italic = false;
            while let Some(word) = words.last().map(|word| word.to_ascii_lowercase()) {
                match word.as_str() {
                    "bold" | "heavy" | "black" => is_bold = true,
                    "italic" | "oblique" => is_italic = true,
                    "normal" | "regular" | "book" | "roman" => {}
                    _ => break,
                }
                words.pop();
            }
            return Some(Self {
                family: words.join(" "),
                size,
                is_bold,
                is_italic,
                has_halo: false,
            });
        }
        let (name, options) = first.split_once(':').unwrap_or((first, ""));
        let (family, size) = match name.rsplit_once('-') {
            Some((family, size)) if !family.is_empty() && size.parse::<f32>().is_ok() => {
                let points = size
                    .parse::<f32>()
                    .unwrap_or(f32::from(Self::DEFAULT_SIZE))
                    .round();
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "font sizes are small positive numbers"
                )]
                (family, points as u16)
            }
            _ => (name, Self::DEFAULT_SIZE),
        };
        let options = options.to_ascii_lowercase();
        Some(Self {
            family: if family == "fixed" {
                "sans".to_owned()
            } else {
                family.to_owned()
            },
            size,
            is_bold: options.contains("bold"),
            is_italic: options.contains("italic") || options.contains("oblique"),
            has_halo: options.split(':').any(|option| option == "halo"),
        })
    }

    /// Returns the font as a vaht style's `font` table.
    fn table(&self) -> InlineTable {
        let mut table = InlineTable::new();
        table.insert("family", Value::from(self.family.clone()));
        table.insert("size", Value::from(i64::from(self.size)));
        if self.is_bold {
            table.insert("weight", Value::from("bold"));
        }
        if self.is_italic {
            table.insert("style", Value::from("italic"));
        }
        table
    }
}

/// A vaht style as layers: each property's value in each layer, chosen so the
/// layers give every element in every state exactly its value.
#[derive(Debug, Default)]
struct Layers {
    /// `[default]`.
    default: BTreeMap<Property, Written>,
    /// `[state.<state>]`.
    by_state: Vec<(WidgetState, BTreeMap<Property, Written>)>,
    /// `[<element>]`.
    by_element: Vec<(StyleElement, BTreeMap<Property, Written>)>,
    /// `[<element>.<state>]`.
    by_element_state: Vec<((StyleElement, WidgetState), BTreeMap<Property, Written>)>,
}

impl Layers {
    /// Returns the fewest layers that give each of `cells` its values.
    ///
    /// Property by property: the value most cells share is the default; a
    /// state, then an element, gets its own value where that spares more
    /// overrides than it costs; what is left differs per element and
    /// state.
    fn fitting(cells: &[(StyleElement, WidgetState, Cell)]) -> Self {
        let mut layers = Self::default();
        let states: Vec<WidgetState> = <WidgetState as strum::IntoEnumIterator>::iter().collect();
        let elements: Vec<StyleElement> =
            <StyleElement as strum::IntoEnumIterator>::iter().collect();
        for property in <Property as strum::IntoEnumIterator>::iter() {
            let given: Vec<(StyleElement, WidgetState, &Written)> = cells
                .iter()
                .filter_map(|(element, state, cell)| Some((*element, *state, cell.get(&property)?)))
                .collect();
            let default = if property.is_own_geometry() {
                None
            } else {
                most_common(given.iter().map(|(.., value)| *value))
            };
            if let Some(default) = &default {
                layers.default.insert(property, default.clone());
            }
            let mut state_values: HashMap<WidgetState, Written> = HashMap::new();
            for state in default.iter().flat_map(|_| &states) {
                let in_state = || {
                    given
                        .iter()
                        .filter(|(_, given_state, _)| given_state == state)
                };
                if let Some(candidate) = most_common(in_state().map(|(.., value)| *value)) {
                    let spared = in_state()
                        .filter(|(.., value)| **value == candidate)
                        .count();
                    let kept = in_state()
                        .filter(|(.., value)| Some(*value) == default.as_ref())
                        .count();
                    if spared > kept + 1 {
                        layers
                            .layer(LayerKey::State(*state))
                            .insert(property, candidate.clone());
                        state_values.insert(*state, candidate);
                    }
                }
            }
            let inherited =
                |state: WidgetState| state_values.get(&state).or(default.as_ref()).cloned();
            for element in &elements {
                let of_element: Vec<&(StyleElement, WidgetState, &Written)> = given
                    .iter()
                    .filter(|(given_element, ..)| given_element == element)
                    .collect();
                let Some(candidate) = most_common(of_element.iter().map(|(.., value)| *value))
                else {
                    continue;
                };
                let without = of_element
                    .iter()
                    .filter(|(_, state, value)| Some(*value) != inherited(*state).as_ref())
                    .count();
                let with = of_element
                    .iter()
                    .filter(|(.., value)| **value != candidate)
                    .count();
                let element_value = (with + 1 < without).then_some(candidate);
                if let Some(value) = &element_value {
                    layers
                        .layer(LayerKey::Element(*element))
                        .insert(property, value.clone());
                }
                for (_, state, value) in of_element {
                    let resolved = element_value.clone().or_else(|| inherited(*state));
                    if Some(*value) != resolved.as_ref() {
                        layers
                            .layer(LayerKey::ElementState(*element, *state))
                            .insert(property, (*value).clone());
                    }
                }
            }
        }
        layers
    }

    /// Returns the layer `key` names, adding it if it is new.
    fn layer(&mut self, key: LayerKey) -> &mut BTreeMap<Property, Written> {
        match key {
            LayerKey::State(state) => entry(&mut self.by_state, state),
            LayerKey::Element(element) => entry(&mut self.by_element, element),
            LayerKey::ElementState(element, state) => {
                entry(&mut self.by_element_state, (element, state))
            }
        }
    }

    /// Returns the layers as a vaht style file.
    fn document(&self) -> DocumentMut {
        let mut document = DocumentMut::new();
        document.insert("default", Item::Table(Self::table(&self.default)));
        if !self.by_state.is_empty() {
            let mut states = Table::new();
            states.set_implicit(true);
            for (state, values) in &self.by_state {
                states.insert(state.name(), Item::Table(Self::table(values)));
            }
            document.insert("state", Item::Table(states));
        }
        for element in <StyleElement as strum::IntoEnumIterator>::iter() {
            let own = self.by_element.iter().find(|(known, _)| *known == element);
            let by_state: Vec<_> = self
                .by_element_state
                .iter()
                .filter(|((known, _), _)| *known == element)
                .collect();
            if own.is_none() && by_state.is_empty() {
                continue;
            }
            let table = Self::element_table(&mut document, element.path());
            if let Some((_, values)) = own {
                for (property, value) in values {
                    table.insert(property.key(), Item::Value(value.0.clone()));
                }
                table.set_implicit(false);
            }
            for ((_, state), values) in by_state {
                table.insert(state.name(), Item::Table(Self::table(values)));
            }
        }
        document
    }

    /// Returns the table at the dotted `path` of `document`, made implicit
    /// (shown only through its children) where it is new.
    fn element_table<'document>(
        document: &'document mut DocumentMut,
        path: &str,
    ) -> &'document mut Table {
        let mut table = document.as_table_mut();
        for part in path.split('.') {
            let item = table.entry(part).or_insert_with(|| {
                let mut new = Table::new();
                new.set_implicit(true);
                Item::Table(new)
            });
            table = item.as_table_mut().expect("vaht style paths name tables");
        }
        table
    }

    /// Returns `values` as a table.
    fn table(values: &BTreeMap<Property, Written>) -> Table {
        let mut table = Table::new();
        for (property, value) in values {
            table.insert(property.key(), Item::Value(value.0.clone()));
        }
        table
    }
}

/// Which layer.
#[derive(Debug, Clone, Copy)]
enum LayerKey {
    /// `[state.<state>]`.
    State(WidgetState),
    /// `[<element>]`.
    Element(StyleElement),
    /// `[<element>.<state>]`.
    ElementState(StyleElement, WidgetState),
}

/// Returns the values kept for `key` in `layers`, adding them if new.
fn entry<Key: PartialEq + Copy>(
    layers: &mut Vec<(Key, BTreeMap<Property, Written>)>,
    key: Key,
) -> &mut BTreeMap<Property, Written> {
    let position = layers
        .iter()
        .position(|(known, _)| *known == key)
        .unwrap_or_else(|| {
            layers.push((key, BTreeMap::new()));
            layers.len() - 1
        });
    &mut layers[position].1
}

/// Returns the value most of `values` share; the first of equals.
fn most_common<'value>(values: impl Iterator<Item = &'value Written>) -> Option<Written> {
    let mut counts: Vec<(Written, usize)> = Vec::new();
    for value in values {
        match counts.iter_mut().find(|(known, _)| known == value) {
            Some((_, count)) => *count += 1,
            None => counts.push((value.clone(), 1)),
        }
    }
    let highest = counts.iter().map(|(_, count)| *count).max()?;
    counts
        .into_iter()
        .find(|(_, count)| *count == highest)
        .map(|(value, _)| value)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use vaht_ui::Style;
    use vaht_ui::style::StyleLayers;

    use super::*;

    /// My style's lines that vaht's style uses, and a pixmap and a
    /// parent-relative texture to show those cases.
    const STYLE: &str = "\
*.font:\t\t\t\tdejavu sans-8:bold
*.font.effect:\t\t\thalo
*.font.halo.color:\t\tblack
*.justify:\t\t\t\tcenter
window.borderWidth:\t\t\t1
toolbar.height:\t\t\t\t20
window.title.height:\t\t\t20
handleWidth:\t\t\t\t\t1
*.color:\t\t\t\tgrey30
*.colorTo:\t\t\t\tgrey5
toolbar*.focused.color:\t\tgrey55
window*.focus.color:\t\tgrey55
*.textColor:\t\t\twhite
*.picColor:\t\t\t\twhite
toolbar*.unfocused.textColor:\tgrey55
window*.unfocus.textColor:\t\tgrey55
window*.unfocus.picColor:\t\tgrey55
window.handle.focus.color:\t\twhite
window.handle.focus.colorTo:\tgrey60
*.pressed.color:\tgrey20
*.pressed.colorTo:\tgrey40
*.borderColor: grey60
*.button.pressed: sunken gradient crossdiagonal
toolbar:\t\t\tgradient vertical
toolbar.clock:\t\tparentrelative
toolbar.workspace:\tpixmap
toolbar.iconbar.focused:\traised gradient vertical
toolbar.iconbar.unfocused:\tsunken gradient vertical
window.label.focus:\t\t\tgradient vertical
window.label.unfocus:\t\tgradient vertical
window.handle.focus:\t\traised gradient vertical
menu.frame.color:\tblack
menu.bullet:\tsquare
";

    /// Converts `style`, returning what each element gets, the vaht style
    /// written, and the report.
    fn convert(style: &str) -> (Vec<(StyleElement, WidgetState, Cell)>, String, Report) {
        let mut report = Report::default();
        let mut file = StyleFile::new(Path::new("style"), style);
        let cells = file.cells(&mut report);
        file.report_unused(&mut report);
        let vaht_style = Layers::fitting(&cells).document().to_string();
        (cells, vaht_style, report)
    }

    /// Returns `cell`'s properties as a vaht style file's table for `element`.
    fn as_element_table(element: StyleElement, cell: &Cell) -> String {
        cell.iter().fold(
            format!("[{}]\n", element.path()),
            |text, (property, value)| format!("{text}{} = {}\n", property.key(), value.text()),
        )
    }

    #[test]
    fn the_theme_gives_every_element_in_every_state_what_the_style_does() {
        let (cells, vaht_style, _) = convert(STYLE);
        let vaht_style =
            Style::resolve(&StyleLayers::from_toml(&vaht_style).expect("a valid style"));
        for (element, state, cell) in &cells {
            let expected = Style::resolve(
                &StyleLayers::from_toml(&as_element_table(*element, cell)).expect("a valid cell"),
            );
            let (got, want) = (
                vaht_style.element_style(*element, *state),
                expected.element_style(*element, *state),
            );
            for property in cell.keys() {
                let matches = match property {
                    Property::Texture => got.texture == want.texture,
                    Property::Font => got.text.font == want.text.font,
                    Property::TextEffect => got.text.effect == want.text.effect,
                    Property::TextColor => got.text.color == want.text.color,
                    Property::GlyphColor => got.glyph_color == want.glyph_color,
                    Property::GlyphImages => got.glyph_images == want.glyph_images,
                    Property::Justify => got.text.justify == want.text.justify,
                    Property::Padding => got.padding == want.padding,
                    Property::Spacing => got.spacing == want.spacing,
                    Property::Border => got.border == want.border,
                    Property::Height => got.height == want.height,
                    Property::CornerRadius => got.corner_radius == want.corner_radius,
                    Property::Corners => got.rounded_corners == want.rounded_corners,
                    Property::Bullet => got.bullet == want.bullet,
                    Property::BulletPosition => got.bullet_side == want.bullet_side,
                    Property::Alpha => got.alpha == want.alpha,
                };
                assert!(
                    matches,
                    "{} {} {}",
                    element.path(),
                    state.name(),
                    property.key()
                );
            }
        }
    }

    #[test]
    fn a_dialog_looks_as_in_the_built_in_theme() {
        let (_, vaht_style, _) = convert(STYLE);
        let vaht_style =
            Style::resolve(&StyleLayers::from_toml(&vaht_style).expect("a valid style"));
        let built_in = Style::built_in();
        let (got, want) = (
            vaht_style.element_style(StyleElement::DialogField, WidgetState::Focused),
            built_in.element_style(StyleElement::DialogField, WidgetState::Focused),
        );
        assert_eq!(got.texture, want.texture);
        assert_eq!(got.text, want.text);
        assert_eq!(got.padding, want.padding);
        let wireframe = |vaht_style: &Style| {
            vaht_style
                .element_style(StyleElement::Wireframe, WidgetState::Focused)
                .border
        };
        assert_eq!(wireframe(&vaht_style), wireframe(&built_in));
    }

    #[test]
    fn the_style_reads_as_fluxbox_reads_it() {
        let (cells, _, _) = convert(STYLE);
        let value = |element, state, property| {
            cells
                .iter()
                .find(|(known, known_state, _)| *known == element && *known_state == state)
                .and_then(|(.., cell)| cell.get(&property))
                .map(Written::text)
                .unwrap_or_default()
        };
        assert_eq!(
            value(StyleElement::Label, WidgetState::Focused, Property::Texture),
            r#"{ gradient = "vertical", from = "grey55", to = "grey5" }"#,
            "window*.focus.color beats *.color"
        );
        assert_eq!(
            value(
                StyleElement::TaskbarClock,
                WidgetState::Focused,
                Property::Texture
            ),
            value(
                StyleElement::Taskbar,
                WidgetState::Focused,
                Property::Texture
            ),
            "parentrelative takes the taskbar's"
        );
        assert_eq!(
            value(
                StyleElement::WindowListEntry,
                WidgetState::Unfocused,
                Property::TextColor
            ),
            r#""grey55""#
        );
        assert_eq!(
            value(StyleElement::Label, WidgetState::Focused, Property::Font),
            r#"{ family = "dejavu sans", size = 8, weight = "bold" }"#
        );
    }

    #[test]
    fn a_pressed_taskbar_button_without_a_texture_reverses_the_buttons_bevel() {
        let (cells, _, _) = convert("toolbar.button: raised gradient vertical\n");
        let texture = |state| {
            cells
                .iter()
                .find(|(element, known, _)| {
                    *element == StyleElement::TaskbarButton && *known == state
                })
                .and_then(|(.., cell)| cell.get(&Property::Texture))
                .map(Written::text)
                .unwrap_or_default()
        };
        assert!(texture(WidgetState::Focused).contains("raised"));
        assert!(texture(WidgetState::Pressed).contains("sunken"));
    }

    #[test]
    fn what_vaht_cannot_draw_or_use_is_reported() {
        let (_, _, report) = convert(STYLE);
        let notes = report.to_string();
        assert!(
            notes.contains("toolbar.workspace: a pixmap texture"),
            "{notes}"
        );
        assert!(
            !notes.contains("menu.bullet"),
            "the bullet converts: {notes}"
        );
    }

    #[test]
    fn style_lines_asking_for_what_vaht_draws_need_nothing() {
        let (_, _, report) = convert(
            "toolbar.shaped: false\n*.bevelWidth: 0\ntoolbar.iconbar.borderWidth: 0\n\
             window.font.effect: none\n",
        );
        assert!(report.notes().is_empty(), "{report}");
        let (_, _, report) = convert("toolbar.shaped: true\n");
        assert!(report.to_string().contains("toolbar.shaped"), "{report}");
    }

    #[test]
    fn a_menu_bullet_converts_with_its_side_and_the_check_color() {
        let (_, vaht_style, _) = convert(
            "menu.bullet: Triangle\nmenu.bullet.position: right\nmenu.hilite.color: #112233\n",
        );
        let resolved = Style::resolve(&StyleLayers::from_toml(&vaht_style).expect("a valid style"));
        let metrics = resolved.metrics().menu;
        assert_eq!(
            metrics.bullet,
            vaht_ui::style::Bullet::Triangle,
            "{vaht_style}"
        );
        assert_eq!(metrics.bullet_side, vaht_ui::MarkSide::Right);
        let item = resolved.element_style(StyleElement::MenuItem, WidgetState::Focused);
        assert_eq!(item.glyph_color, Color::rgb(0x11, 0x22, 0x33));
        let (_, plain, _) = convert("menu.frame.textColor: white\n");
        let resolved = Style::resolve(&StyleLayers::from_toml(&plain).expect("a valid style"));
        assert_eq!(
            resolved.metrics().menu.bullet,
            vaht_ui::style::Bullet::Empty,
            "Fluxbox shows no bullet unless the style says"
        );
    }

    #[rstest]
    #[case::top_corners("TopLeft TopRight", Some("[\"top-left\", \"top-right\"]"))]
    #[case::every_corner("TopLeft TopRight BottomLeft BottomRight", None)]
    fn round_corners_round_the_frame_s_outline(
        #[case] corners: &str,
        #[case] expected_corners: Option<&str>,
    ) {
        let style = format!("window.borderWidth: 1\nwindow.roundCorners: {corners}\n");
        let (cells, vaht_style, report) = convert(&style);
        let border = cells
            .iter()
            .find(|(element, state, _)| {
                *element == StyleElement::Border && *state == WidgetState::Focused
            })
            .map(|(.., cell)| cell)
            .expect("the outline converts");
        assert_eq!(
            border.get(&Property::CornerRadius).map(Written::text),
            Some("8".to_owned())
        );
        assert_eq!(
            border.get(&Property::Corners).map(Written::text),
            expected_corners.map(str::to_owned),
            "all four go without saying"
        );
        let resolved = Style::resolve(&StyleLayers::from_toml(&vaht_style).expect("a valid style"));
        assert!(
            !resolved.metrics().frame_corners.is_square(),
            "{vaht_style}"
        );
        assert!(!report.to_string().contains("roundCorners"), "{report}");
    }

    #[test]
    fn square_corners_add_nothing() {
        let (cells, _, _) = convert("window.roundCorners: none\n");
        // A tooltip is rounded as fluxland's are, whatever the style.
        assert!(
            cells
                .iter()
                .filter(|(element, ..)| *element != StyleElement::Tooltip)
                .all(|(.., cell)| !cell.contains_key(&Property::CornerRadius))
        );
    }

    #[rstest]
    #[case::fluxbox("dejavu sans-8:bold", Some(("dejavu sans", 8, true, false)))]
    #[case::italic("Sans-9.5:italic", Some(("Sans", 10, false, true)))]
    #[case::no_size("monospace", Some(("monospace", 10, false, false)))]
    #[case::pango("pango:DejaVu Sans Bold Italic 11", Some(("DejaVu Sans", 11, true, true)))]
    #[case::pango_without_size("pango:Cantarell", Some(("Cantarell", 10, false, false)))]
    #[case::x11_core("-misc-fixed-medium-r-normal--13-*", None)]
    fn fonts_read_as_fluxbox_writes_them(
        #[case] spec: &str,
        #[case] expected: Option<(&str, u16, bool, bool)>,
    ) {
        let font = FluxboxFont::parse(spec)
            .map(|font| (font.family, font.size, font.is_bold, font.is_italic));
        assert_eq!(
            font,
            expected.map(|(family, size, bold, italic)| (family.to_owned(), size, bold, italic))
        );
    }

    #[test]
    fn a_fallback_the_style_states_beats_a_wildcard_for_the_first_name() {
        // LemonSpace: `*textColor` would answer for the workspace's own name
        // and hide the label's, which the style states.
        let (_, vaht_style, _) = convert(
            "*textColor: rgb:c/c/c\n\
             toolbar.label: flat solid\ntoolbar.label.color: yellow\n\
             toolbar.label.textColor: black\n",
        );
        let document: DocumentMut = vaht_style.parse().expect("TOML");
        assert_eq!(
            document["taskbar"]["workspace"]["text_color"].as_str(),
            Some("black"),
            "{vaht_style}"
        );
    }

    #[test]
    fn a_pressed_taskbar_button_has_its_own_arrow_color() {
        let (_, vaht_style, _) = convert(
            "toolbar.button: flat solid\ntoolbar.button.color: grey\n\
             toolbar.button.picColor: white\ntoolbar.button.pressed: flat solid\n\
             toolbar.button.pressed.color: grey\ntoolbar.button.pressed.picColor: red\n",
        );
        let document: DocumentMut = vaht_style.parse().expect("TOML");
        assert_eq!(
            document["taskbar"]["button"]["pressed"]["glyph_color"].as_str(),
            Some("red"),
            "{vaht_style}"
        );
    }

    #[test]
    fn a_bevel_width_is_the_bars_padding_and_spacing() {
        let (_, vaht_style, _) = convert(
            "window.bevelWidth: 3\ntoolbar.bevelWidth: 2\n\
             window.title.focus: flat solid\nwindow.title.focus.color: grey\n",
        );
        let document: DocumentMut = vaht_style.parse().expect("TOML");
        assert_eq!(
            document["window"]["titlebar"]["padding"].to_string().trim(),
            "[3, 3, 3, 3]"
        );
        assert_eq!(
            document["window"]["titlebar"]["spacing"].as_integer(),
            Some(3)
        );
        assert_eq!(document["taskbar"]["spacing"].as_integer(), Some(2));
    }

    #[test]
    fn the_iconbars_border_is_the_window_list_entries() {
        let (_, vaht_style, _) = convert(
            "toolbar.iconbar.focused: flat solid\ntoolbar.iconbar.focused.color: grey\n\
             toolbar.iconbar.focused.borderWidth: 2\n\
             toolbar.iconbar.focused.borderColor: red\n\
             toolbar.iconbar.unfocused: flat solid\ntoolbar.iconbar.unfocused.color: grey\n\
             toolbar.iconbar.unfocused.borderWidth: 0\n",
        );
        let document: DocumentMut = vaht_style.parse().expect("TOML");
        let entry = &document["taskbar"]["window-list"]["entry"];
        assert_eq!(
            entry["border"]["width"].as_integer(),
            Some(2),
            "{vaht_style}"
        );
        assert_eq!(entry["border"]["color"].as_str(), Some("red"));
        assert_eq!(
            entry["unfocused"]["border"]["width"].as_integer(),
            Some(0),
            "an unfocused entry with no line says so, since the focused one has one: {vaht_style}"
        );
    }

    #[test]
    fn a_buttons_padding_leaves_its_picture_ten_pixels_whatever_its_size() {
        let padding = |style: &str| {
            let (_, vaht_style, _) = convert(style);
            let document: DocumentMut = vaht_style.parse().expect("TOML");
            document["window"]["button"]["padding"]
                .to_string()
                .trim()
                .to_owned()
        };
        // 20 high, no bevel: a 20 pixel button, 5 around its picture.
        assert_eq!(padding("window.title.height: 20\n"), "[5, 5, 5, 5]");
        // Sage: 24 high with a bevel of 5 is a 14 pixel button.
        assert_eq!(
            padding("window.title.height: 24\nwindow.bevelWidth: 5\n"),
            "[2, 2, 2, 2]"
        );
        assert_eq!(
            padding("window.title.height: 8\n"),
            "[0, 0, 0, 0]",
            "a button smaller than its picture has no padding"
        );
    }
}
