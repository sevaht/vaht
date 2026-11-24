//! Converts a Fluxbox setup to vaht's configuration (02 §11).
//!
//! [`FluxboxSetup::read`] reads a Fluxbox configuration directory (`~/.fluxbox`,
//! or my fluxland one: the format is the same), following the file paths its
//! `init` names. [`FluxboxSetup::convert`] turns it into vaht's files, and
//! [`Conversion::write`] writes them, never over existing files unless asked,
//! and checks the result as `vaht config check` would. The source is never changed.
//! Every line that is not converted is in the [`Report`], with why.
//!
//! [`convert_style`] converts a Fluxbox style on its own (colors, fonts,
//! textures: nothing else of a setup), and [`find_styles`] finds the styles in
//! a directory of them.
//!
//! Converted today: `init` to `settings.toml`, `keys` to `bindings.toml`,
//! `apps` to `rules.toml`, `menu` and `windowmenu` to `menus.toml`, and the
//! style `init` names to `styles/<name>.toml`, which `settings.toml` then
//! chooses. The wallpaper `startup` sets becomes `[wallpaper]`; the rest of
//! `startup` is reported: its commands belong in `session.autostart`.

mod apps;
mod command;
mod fold;
mod input;
mod keys;
mod menu;
mod outputs;
mod pattern;
mod presentation;
mod report;
mod resources;
mod settings;
mod style;
mod wallpaper;

use std::io;
use std::path::{Path, PathBuf};

pub use fold::fold_variants;
pub use pattern::RUN_DIALOG_APP_ID;
pub use report::{Note, Reason, Report};

use crate::apps::AppsFile;
use crate::keys::KeysFile;
use crate::menu::MenuFiles;
use crate::resources::Resources;
use crate::settings::InitFile;
use crate::style::{StyleFile, StyleImage};
use crate::wallpaper::StartupWallpaper;

/// A Fluxbox configuration directory, read.
#[derive(Debug)]
pub struct FluxboxSetup {
    /// The directory.
    directory: PathBuf,
    /// The `init` resources.
    init: Resources,
    /// The `keys` file: where, and what it says.
    keys: Option<(PathBuf, String)>,
    /// The `apps` file: where, and what it says.
    apps: Option<(PathBuf, String)>,
    /// The style `init` names: where, and what it says.
    style: Option<(PathBuf, String)>,
    /// The root menu: where, and what it says.
    menu: Option<(PathBuf, String)>,
    /// The window menu: where, and what it says.
    window_menu: Option<(PathBuf, String)>,
    /// The `startup` script: where, and what it says.
    startup: Option<(PathBuf, String)>,
}

impl FluxboxSetup {
    /// Reads the setup in `directory`.
    ///
    /// # Errors
    ///
    /// Returns an error if `directory` has no `init`, or a file cannot be
    /// read.
    pub fn read(directory: &Path) -> Result<Self, ImportError> {
        let init_path = directory.join("init");
        let init_text = std::fs::read_to_string(&init_path).map_err(|error| ImportError::Read {
            path: init_path.clone(),
            error,
        })?;
        let mut init = Resources::parse(&init_path, &init_text);
        // A style is a file, or a directory holding theme.cfg.
        let style_path = init
            .value("session.styleFile")
            .map(|path| directory.join(Self::expand_home(path)))
            .map(|path| {
                if path.is_dir() {
                    path.join("theme.cfg")
                } else {
                    path
                }
            });
        let mut file = |resource: &str, default: &str| {
            init.value(resource).map_or_else(
                || directory.join(default),
                |path| directory.join(Self::expand_home(path)),
            )
        };
        let keys_path = file("session.keyFile", "keys");
        let apps_path = file("session.appsFile", "apps");
        let menu_path = file("session.menuFile", "menu");
        let window_menu_path = file("session.screen0.windowMenu", "windowmenu");
        Ok(Self {
            directory: directory.to_path_buf(),
            init,
            keys: Self::read_optional(keys_path)?,
            apps: Self::read_optional(apps_path)?,
            style: style_path.map(Self::read_optional).transpose()?.flatten(),
            menu: Self::read_optional(menu_path)?,
            window_menu: Self::read_optional(window_menu_path)?,
            startup: Self::read_optional(directory.join("startup"))?,
        })
    }

    /// Reads the file at `path`, if it exists.
    fn read_optional(path: PathBuf) -> Result<Option<(PathBuf, String)>, ImportError> {
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some((path, text))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ImportError::Read { path, error }),
        }
    }

    /// Converts the setup to vaht's files.
    #[must_use]
    pub fn convert(self) -> Conversion {
        let mut report = Report::default();
        let header = |source: &Path| {
            format!(
                "# Converted from {} by vaht import fluxbox.\n\n",
                source.display()
            )
        };
        let mut files = Vec::new();
        let init_path = self.init.path().to_path_buf();
        let init = InitFile::new(self.init).convert(&mut report);
        let mut settings = init.settings;
        let layout = presentation::take_layout(&mut settings);
        let mut converted_style = None;
        if let Some(outputs) = init.outputs {
            files.push(OutputFile {
                name: "outputs.toml".to_owned(),
                text: header(&init_path) + &outputs.to_string(),
            });
        }
        if let Some((path, text)) = &self.style {
            let name = style_name(path);
            let (mut vaht_style, images) = StyleFile::new(path, text).convert(&mut report);
            Self::place_alphas(&init.alphas, Some(&mut vaht_style), &init_path, &mut report);
            files.extend(images.into_iter().map(|image| OutputFile {
                name: format!("styles/{}", image.name),
                text: image.text,
            }));
            files.push(OutputFile {
                name: format!("styles/{name}.toml"),
                text: header(path) + &vaht_style.to_string(),
            });
            converted_style = Some(name);
        }
        if self.style.is_none() {
            Self::place_alphas(&init.alphas, None, &init_path, &mut report);
        }
        Self::convert_startup(self.startup.as_ref(), &mut settings, &mut report);
        Self::place_look(
            &layout,
            converted_style.as_deref(),
            &init_path,
            &mut settings,
            &mut files,
        );
        files.insert(
            0,
            OutputFile {
                name: "settings.toml".to_owned(),
                text: header(&init_path) + &settings.to_string(),
            },
        );
        if let Some((path, text)) = &self.keys {
            let mut bindings = KeysFile::new(path, text).convert(&mut report);
            if !init.switches.is_empty() {
                let mut switches = toml_edit::Table::new();
                for (switch, command, _) in init.switches {
                    switches.insert(switch, toml_edit::Item::Value(command));
                }
                bindings.insert("switches", toml_edit::Item::Table(switches));
            }
            files.push(OutputFile {
                name: "bindings.toml".to_owned(),
                text: header(path) + &bindings.to_string(),
            });
        } else {
            // bindings.toml replaces the built-in bindings whole, so it is
            // not written for the switches alone.
            for (switch, command, line) in init.switches {
                report.note(
                    &init_path,
                    line,
                    Reason::NotYetInVaht,
                    format!(
                        "{switch} = {command}: there is no keys file, so add it to \
                         [switches] in bindings.toml by hand"
                    ),
                );
            }
        }
        if let Some((path, text)) = &self.menu {
            let mut menus = MenuFiles::new();
            menus.convert_root(path, text, &mut report);
            if let Some((path, text)) = &self.window_menu {
                menus.convert_window(path, text, &mut report);
            }
            files.push(OutputFile {
                name: "menus.toml".to_owned(),
                text: header(path) + &menus.document().to_string(),
            });
        }
        if let Some((path, text)) = &self.apps {
            let rules = AppsFile::new(path, text).convert(&mut report);
            files.push(OutputFile {
                name: "rules.toml".to_owned(),
                text: header(path) + &rules.to_string(),
            });
        }
        Conversion {
            source: self.directory,
            files,
            report,
        }
    }

    /// Converts the `startup` script: its wallpaper into `settings`, the rest
    /// reported, as its commands belong in `session.autostart`.
    fn convert_startup(
        startup: Option<&(PathBuf, String)>,
        settings: &mut toml_edit::DocumentMut,
        report: &mut Report,
    ) {
        if let Some((path, text)) = startup {
            let why = match StartupWallpaper::find(text) {
                Some(wallpaper) => {
                    settings.insert("wallpaper", wallpaper.settings());
                    "the startup script, but for its wallpaper: add the rest to \
                     session.autostart if vaht should run it"
                }
                None => "the startup script: add it to session.autostart if vaht should run it",
            };
            report.note(path, 1, Reason::NotYetInVaht, why);
        }
    }

    /// Names the converted `style` and the `layout` taken from `init` (at
    /// `init_path`) in `settings`, and adds the layout's file to `files`.
    fn place_look(
        layout: &toml_edit::DocumentMut,
        style: Option<&str>,
        init_path: &Path,
        settings: &mut toml_edit::DocumentMut,
        files: &mut Vec<OutputFile>,
    ) {
        if let Some(style) = style {
            settings.insert("style", toml_edit::value(style));
        }
        if let Some((name, file)) = Self::layout_file(layout, style, init_path) {
            files.push(file);
            settings.insert("layout", toml_edit::value(name));
        }
    }

    /// Returns the layout file for the `layout` taken from `init` (at
    /// `init_path`), and the name it has, if it has anything in it. What is
    /// shown and how it is arranged go to a layout of its own, so
    /// `settings.toml` is left with how vaht behaves and another layout swaps
    /// the arrangement whole.
    fn layout_file(
        layout: &toml_edit::DocumentMut,
        style: Option<&str>,
        init_path: &Path,
    ) -> Option<(String, OutputFile)> {
        if layout.as_table().is_empty() {
            return None;
        }
        let name = style.unwrap_or("fluxbox").to_owned();
        let header = format!(
            "# Converted from {} by vaht import fluxbox.\n\n",
            init_path.display()
        );
        let file = OutputFile {
            name: format!("layouts/{name}.toml"),
            text: format!("{header}{layout}"),
        };
        Some((name, file))
    }

    /// Puts the `alphas` `init` (at `init_path`) sets into the converted
    /// `vaht_style`; without one, notes in `report` that they are left out.
    fn place_alphas(
        alphas: &[(&str, i64, usize)],
        vaht_style: Option<&mut toml_edit::DocumentMut>,
        init_path: &Path,
        report: &mut Report,
    ) {
        let Some(vaht_style) = vaht_style else {
            for (element, _, line) in alphas {
                report.note(
                    init_path,
                    *line,
                    Reason::NotYetInVaht,
                    format!(
                        "a translucent {element}: with no style converted, add `alpha` to a vaht style"
                    ),
                );
            }
            return;
        };
        for (element, alpha, _) in alphas {
            let mut table = vaht_style.as_item_mut();
            for part in element.split('.') {
                table = &mut table[part];
            }
            table["alpha"] = toml_edit::value(*alpha);
        }
    }

    /// Expands a leading `~/` to the home directory, as Fluxbox does; a
    /// relative result is then taken from the setup's directory.
    fn expand_home(path: &str) -> PathBuf {
        match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
            (Some(rest), Some(home)) => Path::new(&home).join(rest),
            (Some(_) | None, _) => PathBuf::from(path),
        }
    }
}

/// Returns the name the vaht style converted from the style at `path` gets:
/// the style's own (its directory's, for a `theme.cfg`, and without a
/// `.style` or `.cfg` extension), with anything but letters, digits, `-` and `_` made
/// `-`; a style called `default` is `fluxbox-default`.
pub(crate) fn style_name(path: &Path) -> String {
    let named = if path.file_name().is_some_and(|name| name == "theme.cfg") {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let file_name = named
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name: String = file_name
        .strip_suffix(".style")
        .or_else(|| file_name.strip_suffix(".cfg"))
        .unwrap_or(&file_name)
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    match name.as_str() {
        "" => "imported".to_owned(),
        // vaht's own default style has this name, and a file of it would
        // replace that one.
        "default" => "fluxbox-default".to_owned(),
        _ => name,
    }
}

/// Returns whether `text` reads as a Fluxbox style: some line sets a
/// `window.`, `toolbar.` or `menu.` resource (or the toolbar's own texture).
fn looks_like_style(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        ["window.", "toolbar.", "menu.", "toolbar:", "menu.frame:"]
            .iter()
            .any(|start| line.starts_with(start))
            && line.contains(':')
    })
}

/// Finds the Fluxbox styles at `path`: the style it is (a file, or a directory
/// holding `theme.cfg`), or, for a directory of styles, every style in it,
/// searched through (a directory that is not itself a style is searched in
/// turn). A file counts as a style when it reads as one, so a `README` among
/// them is left alone. Sorted.
///
/// # Errors
///
/// Returns an error if `path`, or a directory in it, cannot be read.
pub fn find_styles(path: &Path) -> Result<Vec<PathBuf>, ImportError> {
    let metadata = std::fs::metadata(path).map_err(|error| ImportError::Read {
        path: path.to_path_buf(),
        error,
    })?;
    if metadata.is_file() {
        // Named directly: the user says it is a style.
        return Ok(vec![path.to_path_buf()]);
    }
    if path.join("theme.cfg").is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|error| ImportError::Read {
            path: path.to_path_buf(),
            error,
        })?
        .flatten()
        .map(|entry| entry.path())
        .filter(|entry| {
            !entry
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();
    entries.sort();
    let mut styles = Vec::new();
    for entry in entries {
        if entry.is_dir() {
            styles.extend(find_styles(&entry)?);
        } else if std::fs::read_to_string(&entry).is_ok_and(|text| looks_like_style(&text)) {
            styles.push(entry);
        }
    }
    Ok(styles)
}

/// A Fluxbox style, converted to a vaht style file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedStyle {
    /// The style converted: a file, or the directory holding its `theme.cfg`.
    source: PathBuf,
    /// The name the vaht style gets: `styles/<name>.toml`.
    name: String,
    /// The vaht style file.
    text: String,
    /// The images it draws with, to be written beside it.
    images: Vec<StyleImage>,
    /// What was not converted.
    report: Report,
    /// How many variants of a style this folds into one with options: one if
    /// it is a style as it was.
    variants: usize,
}

impl ConvertedStyle {
    /// Returns how many variants of a style this folds into one with options
    /// (one for a style as it was).
    #[must_use]
    pub const fn variants(&self) -> usize {
        self.variants
    }

    /// Returns where the style came from.
    #[must_use]
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Returns the name the vaht style gets.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the vaht style file.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns what was not converted, and why.
    #[must_use]
    pub const fn report(&self) -> &Report {
        &self.report
    }

    /// Writes the style to `<directory>/<name>.toml`, creating the directory;
    /// returns the path. An existing file is replaced only with `replace`.
    /// The style is read as vaht reads one first, so nothing invalid is
    /// written.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists and `replace` is false, the style is
    /// not valid (a bug in the conversion), or the file cannot be written.
    pub fn write(&self, directory: &Path, replace: bool) -> Result<PathBuf, ImportError> {
        let path = directory.join(format!("{}.toml", self.name));
        if !replace && path.exists() {
            return Err(ImportError::Exists(path));
        }
        std::fs::create_dir_all(directory).map_err(|error| ImportError::Write {
            path: directory.to_path_buf(),
            error,
        })?;
        for image in &self.images {
            let image_path = directory.join(&image.name);
            if let Some(parent) = image_path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| ImportError::Write {
                    path: parent.to_path_buf(),
                    error,
                })?;
            }
            std::fs::write(&image_path, &image.text).map_err(|error| ImportError::Write {
                path: image_path.clone(),
                error,
            })?;
        }
        // Read as vaht reads it, images and all, before it is written.
        vaht_ui::style::StyleLayers::from_toml_in(&self.text, Some(directory)).map_err(
            |error| ImportError::InvalidStyle {
                name: self.name.clone(),
                message: error.to_string(),
            },
        )?;
        std::fs::write(&path, &self.text).map_err(|error| ImportError::Write {
            path: path.clone(),
            error,
        })?;
        Ok(path)
    }
}

/// Converts the Fluxbox style at `path` (a file, or a directory holding
/// `theme.cfg`) to a vaht style, and nothing else of a setup: its colors,
/// fonts, textures and sizes. Every line not converted is in the report.
///
/// # Errors
///
/// Returns an error if the style cannot be read.
pub fn convert_style(path: &Path) -> Result<ConvertedStyle, ImportError> {
    let file = if path.is_dir() {
        path.join("theme.cfg")
    } else {
        path.to_path_buf()
    };
    let text = std::fs::read_to_string(&file).map_err(|error| ImportError::Read {
        path: file.clone(),
        error,
    })?;
    let mut report = Report::default();
    let (converted, images) = StyleFile::new(&file, &text).convert(&mut report);
    Ok(ConvertedStyle {
        source: path.to_path_buf(),
        name: style_name(&file),
        text: format!(
            "# Converted from {} by vaht import fluxbox-style.\n\n{converted}",
            path.display()
        ),
        images,
        report,
        variants: 1,
    })
}

/// One file of vaht's configuration, converted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    /// Its path in the configuration directory, such as `settings.toml` or
    /// `styles/nacitar.toml`.
    pub name: String,
    /// Its contents.
    pub text: String,
}

/// A converted setup, ready to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    /// The directory converted.
    source: PathBuf,
    /// The files.
    files: Vec<OutputFile>,
    /// What was not converted.
    report: Report,
}

impl Conversion {
    /// Returns the converted files.
    #[must_use]
    pub fn files(&self) -> &[OutputFile] {
        &self.files
    }

    /// Returns what was not converted, and why.
    #[must_use]
    pub const fn report(&self) -> &Report {
        &self.report
    }

    /// Writes the files into `directory`, creating it if needed. Existing
    /// files are replaced only with `replace`. The result is then read as
    /// vaht reads it.
    ///
    /// # Errors
    ///
    /// Returns an error if a file exists and `replace` is false, a file
    /// cannot be written, the target is the source, or vaht would not
    /// accept the result.
    pub fn write(&self, directory: &Path, replace: bool) -> Result<(), ImportError> {
        if directory == self.source {
            return Err(ImportError::SameDirectory(directory.to_path_buf()));
        }
        if !replace
            && let Some(existing) = self
                .files
                .iter()
                .map(|file| directory.join(&file.name))
                .find(|path| path.exists())
        {
            return Err(ImportError::Exists(existing));
        }
        std::fs::create_dir_all(directory).map_err(|error| ImportError::Write {
            path: directory.to_path_buf(),
            error,
        })?;
        for file in &self.files {
            let path = directory.join(&file.name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| ImportError::Write {
                    path: parent.to_path_buf(),
                    error,
                })?;
            }
            std::fs::write(&path, &file.text)
                .map_err(|error| ImportError::Write { path, error })?;
        }
        vaht_compositor::Config::load(directory).map_err(ImportError::Invalid)?;
        Ok(())
    }
}

/// Why an import failed.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// A source file could not be read.
    #[error("could not read {}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        error: io::Error,
    },
    /// A converted file could not be written.
    #[error("could not write {}", path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        error: io::Error,
    },
    /// A file to write exists already.
    #[error("{} exists; pass --force to replace it", .0.display())]
    Exists(PathBuf),
    /// The target directory is the source directory.
    #[error("{} is the setup being converted; choose another directory", .0.display())]
    SameDirectory(PathBuf),
    /// The written configuration is not valid: a bug in the conversion.
    #[error("the converted configuration is not valid (a bug in vaht import)")]
    Invalid(#[source] vaht_compositor::ConfigError),
    /// A converted style is not valid: a bug in the conversion.
    #[error("the converted style {name} is not valid (a bug in vaht import): {message}")]
    InvalidStyle {
        /// The style's name.
        name: String,
        /// What is wrong.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setup_converts_and_writes_a_valid_configuration() {
        let source = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            source.path().join("init"),
            "session.screen0.windowPlacement: CascadePlacement\nsession.keyFile: keys\n",
        )
        .expect("written");
        std::fs::write(
            source.path().join("keys"),
            "Mod4 T :Exec kitty\nOnDesktop Mouse3 :RootMenu\n",
        )
        .expect("written");
        std::fs::write(
            source.path().join("menu"),
            "[begin] (vaht)\n[exec] (kitty) {kitty}\n[submenu] (More)\n[exit] (Exit)\n[end]\n[end]\n",
        )
        .expect("written");
        let conversion = FluxboxSetup::read(source.path())
            .expect("readable")
            .convert();
        let target = tempfile::tempdir().expect("a temporary directory");
        conversion
            .write(target.path(), false)
            .expect("written and valid");
        assert!(
            conversion.write(target.path(), false).is_err(),
            "existing files are kept"
        );
        conversion
            .write(target.path(), true)
            .expect("replaced on request");
        let bindings =
            std::fs::read_to_string(target.path().join("bindings.toml")).expect("written");
        assert!(
            bindings.contains("\"Super+T\" = \"exec kitty\""),
            "{bindings}"
        );
        assert!(bindings.contains("Right = \"root-menu\""), "{bindings}");
        let menus = std::fs::read_to_string(target.path().join("menus.toml")).expect("written");
        assert!(menus.contains("submenu = \"more\""), "{menus}");
        assert!(
            conversion.report().notes().is_empty(),
            "{}",
            conversion.report()
        );
    }

    #[test]
    fn switch_commands_become_switch_bindings() {
        let source = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            source.path().join("init"),
            "session.switch.lidClose: Exec swaylock\nsession.switch.tabletModeOn: Exec onboard\n",
        )
        .expect("written");
        std::fs::write(source.path().join("keys"), "Mod4 T :Exec kitty\n").expect("written");
        let conversion = FluxboxSetup::read(source.path())
            .expect("readable")
            .convert();
        let target = tempfile::tempdir().expect("a temporary directory");
        conversion
            .write(target.path(), false)
            .expect("written and valid");
        let bindings =
            std::fs::read_to_string(target.path().join("bindings.toml")).expect("written");
        assert!(
            bindings.contains(
                "[switches]\nlid-close = \"exec swaylock\"\ntablet-mode-on = \"exec onboard\"\n"
            ),
            "{bindings}"
        );
        assert!(
            conversion.report().notes().is_empty(),
            "{}",
            conversion.report()
        );
    }

    #[test]
    fn switch_commands_without_a_keys_file_are_reported() {
        let source = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            source.path().join("init"),
            "session.switch.lidClose: Exec swaylock\n",
        )
        .expect("written");
        let conversion = FluxboxSetup::read(source.path())
            .expect("readable")
            .convert();
        assert!(
            conversion
                .files()
                .iter()
                .all(|file| file.name != "bindings.toml"),
            "the built-in bindings are kept"
        );
        assert_eq!(
            conversion.report().notes().len(),
            1,
            "{}",
            conversion.report()
        );
    }

    #[test]
    fn the_wallpaper_startup_sets_becomes_the_wallpaper_setting() {
        let source = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(source.path().join("init"), "").expect("written");
        std::fs::write(
            source.path().join("startup"),
            "swaybg -m tile -i ~/sky.png &\nexec fluxbox\n",
        )
        .expect("written");
        let conversion = FluxboxSetup::read(source.path())
            .expect("readable")
            .convert();
        let settings = &conversion.files()[0].text;
        assert!(
            settings.contains("[wallpaper]\nimage = \"~/sky.png\"\nfit = \"tile\"\n"),
            "{settings}"
        );
        let target = tempfile::tempdir().expect("a temporary directory");
        conversion.write(target.path(), false).expect("valid");
    }

    /// A small Fluxbox style.
    const SAMPLE_STYLE: &str = "toolbar: Raised Gradient Vertical\n\
        toolbar.color: grey30\ntoolbar.colorTo: grey5\n\
        window.title.focus: Raised Gradient Vertical\n\
        window.title.focus.color: grey55\nwindow.title.focus.colorTo: grey5\n\
        window.label.focus.textColor: white\n\
        menu.frame: Flat Solid\nmenu.frame.color: black\nmenu.frame.textColor: white\n";

    #[test]
    fn a_style_is_named_for_its_file_or_directory() {
        let name = |path: &str| style_name(Path::new(path));
        assert_eq!(name("/styles/Artwiz"), "Artwiz");
        assert_eq!(
            name("/styles/wave.style"),
            "wave",
            "the extension is not part of the name"
        );
        assert_eq!(
            name("/styles/green tea/theme.cfg"),
            "green-tea",
            "a directory's name"
        );
        assert_eq!(name("/styles/bloe"), "bloe");
        assert_eq!(
            name("/styles/default"),
            "fluxbox-default",
            "not vaht's own default style's name"
        );
        assert_eq!(name("/"), "imported");
    }

    #[test]
    fn a_style_converts_on_its_own_and_is_written_without_replacing() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let source = directory.path().join("Mine");
        std::fs::write(&source, SAMPLE_STYLE).expect("written");
        let converted = convert_style(&source).expect("converts");
        assert_eq!(converted.name(), "Mine");
        assert!(
            converted.text().starts_with("# Converted from"),
            "{}",
            converted.text()
        );
        let styles = directory.path().join("out");
        let written = converted.write(&styles, false).expect("written");
        assert_eq!(written, styles.join("Mine.toml"));
        assert!(
            vaht_ui::style::StyleLayers::from_toml(
                &std::fs::read_to_string(&written).expect("read")
            )
            .is_ok(),
            "what is written is a style vaht reads"
        );
        assert!(matches!(
            converted.write(&styles, false),
            Err(ImportError::Exists(_))
        ));
        converted.write(&styles, true).expect("replaced on request");
    }

    #[test]
    fn a_style_directory_is_read_from_its_theme_cfg() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let style = directory.path().join("arch");
        std::fs::create_dir(&style).expect("a directory");
        std::fs::write(style.join("theme.cfg"), SAMPLE_STYLE).expect("written");
        let converted = convert_style(&style).expect("converts");
        assert_eq!(converted.name(), "arch");
    }

    #[test]
    fn a_style_that_cannot_be_read_is_an_error() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        assert!(matches!(
            convert_style(&directory.path().join("missing")),
            Err(ImportError::Read { .. })
        ));
    }

    #[test]
    fn a_directory_of_styles_is_searched_for_the_files_that_read_as_styles() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path();
        std::fs::write(root.join("Alpha"), SAMPLE_STYLE).expect("written");
        std::fs::write(
            root.join("README"),
            "Styles for fluxbox.\nSee the licenses.\n",
        )
        .expect("written");
        std::fs::write(root.join(".hidden"), SAMPLE_STYLE).expect("written");
        std::fs::create_dir(root.join("community")).expect("a directory");
        std::fs::write(root.join("community").join("Beta.style"), SAMPLE_STYLE).expect("written");
        std::fs::create_dir(root.join("gamma")).expect("a directory");
        std::fs::write(root.join("gamma").join("theme.cfg"), SAMPLE_STYLE).expect("written");
        std::fs::create_dir(root.join("gamma").join("pixmaps")).expect("a directory");
        let found: Vec<String> = find_styles(root)
            .expect("searched")
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .expect("inside")
                    .display()
                    .to_string()
            })
            .collect();
        assert_eq!(found, ["Alpha", "community/Beta.style", "gamma"]);
    }

    #[test]
    fn a_style_named_directly_is_one_even_if_it_does_not_read_as_a_style() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let file = directory.path().join("odd");
        std::fs::write(&file, "nothing here\n").expect("written");
        assert_eq!(find_styles(&file).expect("found"), [file]);
    }
}
