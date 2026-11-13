//! Finding icons by name, and applications' icons by app id.
//!
//! [`IconStore`] looks in the XDG data directories, as the icon theme and
//! desktop entry specifications describe, keeping to what every system
//! has: the `hicolor` theme (each `NxN` size's `apps`, then `scalable`)
//! and the old `pixmaps` directory. An app id finds its desktop entry by
//! file name (exactly, in any case, or as the last part of a reverse-DNS
//! name) or by its `StartupWMClass`; the entry's `Icon` names the icon,
//! and without an entry the app id itself is tried as a name. Lookups are
//! cached, misses too, until [`IconStore::forget`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::Icon;

/// Finds and keeps icons, from the XDG data directories.
#[derive(Debug, Default)]
pub struct IconStore {
    /// Where to look, most important first.
    data_directories: Vec<PathBuf>,
    /// Icons found (or not) by name and size.
    by_name: HashMap<(String, u32), Option<Icon>>,
    /// Icon names found (or not) by app id.
    names_by_app_id: HashMap<String, Option<String>>,
}

impl IconStore {
    /// The data directories when `XDG_DATA_DIRS` names none, as the XDG
    /// base directory specification says.
    const DEFAULT_DATA_DIRECTORIES: [&str; 2] = ["/usr/local/share", "/usr/share"];

    /// Creates a store looking in `data_directories`, most important first.
    #[must_use]
    pub fn new(data_directories: Vec<PathBuf>) -> Self {
        Self {
            data_directories,
            ..Self::default()
        }
    }

    /// Creates a store looking where the environment says: `XDG_DATA_HOME`
    /// (or `~/.local/share`), then `XDG_DATA_DIRS`.
    #[must_use]
    pub fn from_environment() -> Self {
        let variable = |name| std::env::var_os(name).filter(|value| !value.is_empty());
        let home = variable("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| variable("HOME").map(|home| PathBuf::from(home).join(".local/share")));
        let shared: Vec<PathBuf> = variable("XDG_DATA_DIRS").map_or_else(
            || {
                Self::DEFAULT_DATA_DIRECTORIES
                    .iter()
                    .map(PathBuf::from)
                    .collect()
            },
            |directories| std::env::split_paths(&directories).collect(),
        );
        Self::new(
            home.into_iter()
                .chain(shared)
                .filter(|directory| directory.is_absolute())
                .collect(),
        )
    }

    /// Returns the icon of the application `app_id`, `size` wide, if one
    /// is found.
    pub fn app_icon(&mut self, app_id: &str, size: u32) -> Option<Icon> {
        if app_id.is_empty() {
            return None;
        }
        let name = if let Some(name) = self.names_by_app_id.get(app_id) {
            name.clone()
        } else {
            let name = self.icon_name_of(app_id);
            self.names_by_app_id.insert(app_id.to_owned(), name.clone());
            name
        };
        self.named(name.as_deref().unwrap_or(app_id), size)
    }

    /// Returns the icon named `name` (or at the absolute path `name`),
    /// `size` wide, if one is found.
    pub fn named(&mut self, name: &str, size: u32) -> Option<Icon> {
        let key = (name.to_owned(), size);
        if let Some(icon) = self.by_name.get(&key) {
            return icon.clone();
        }
        let icon = self.find_file(name, size).and_then(|path| {
            Icon::load(&path, size)
                .inspect_err(|error| {
                    tracing::debug!(
                        error = error as &dyn std::error::Error,
                        "an icon was not used"
                    );
                })
                .ok()
        });
        self.by_name.insert(key, icon.clone());
        icon
    }

    /// Forgets every lookup, so icons installed since are found.
    pub fn forget(&mut self) {
        self.by_name.clear();
        self.names_by_app_id.clear();
    }

    /// Returns the file of the icon `name` best for `size`: a `hicolor`
    /// picture at least that big (the closest), then a scalable one, then
    /// a smaller one (the closest), then one in `pixmaps`.
    fn find_file(&self, name: &str, size: u32) -> Option<PathBuf> {
        let path = Path::new(name);
        if path.is_absolute() {
            return path.is_file().then(|| path.to_path_buf());
        }
        if name.contains('/') || name.is_empty() {
            return None;
        }
        let mut sized: Vec<(u32, PathBuf)> = self
            .data_directories
            .iter()
            .flat_map(|directory| Self::sized_directories(&directory.join("icons/hicolor")))
            .map(|(pixels, directory)| (pixels, directory.join(format!("apps/{name}.png"))))
            .filter(|(_, file)| file.is_file())
            .collect();
        // Big enough first, smallest first; then too small, biggest first.
        sized.sort_by_key(|(pixels, _)| {
            if *pixels >= size {
                (0, *pixels)
            } else {
                (1, u32::MAX - *pixels)
            }
        });
        let (big_enough, too_small): (Vec<_>, Vec<_>) =
            sized.into_iter().partition(|(pixels, _)| *pixels >= size);
        let within = |relative: &str| {
            self.data_directories
                .iter()
                .map(|directory| directory.join(relative))
                .find(|file| file.is_file())
        };
        big_enough
            .into_iter()
            .map(|(_, file)| file)
            .next()
            .or_else(|| within(&format!("icons/hicolor/scalable/apps/{name}.svg")))
            .or_else(|| too_small.into_iter().map(|(_, file)| file).next())
            .or_else(|| within(&format!("pixmaps/{name}.png")))
            .or_else(|| within(&format!("pixmaps/{name}.svg")))
    }

    /// Returns the `NxN` size directories of the theme at `theme`, with
    /// their size.
    fn sized_directories(theme: &Path) -> Vec<(u32, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(theme) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let (width, height) = name.to_str()?.split_once('x')?;
                let pixels: u32 = width.parse().ok()?;
                (height.parse::<u32>().ok()? == pixels).then(|| (pixels, entry.path()))
            })
            .collect()
    }

    /// Returns the icon name the desktop entry of `app_id` gives, if it
    /// has one.
    fn icon_name_of(&self, app_id: &str) -> Option<String> {
        let applications = || {
            self.data_directories
                .iter()
                .map(|directory| directory.join("applications"))
        };
        let exact = applications()
            .map(|directory| DesktopEntry::new(directory.join(format!("{app_id}.desktop"))))
            .find(|entry| entry.path.is_file());
        if let Some(entry) = exact {
            return entry.value("Icon");
        }
        applications()
            .filter_map(|directory| std::fs::read_dir(directory).ok())
            .flat_map(|entries| entries.filter_map(Result::ok))
            .map(|entry| DesktopEntry::new(entry.path()))
            .find(|entry| entry.belongs_to(app_id))
            .and_then(|entry| entry.value("Icon"))
    }
}

/// A desktop entry file.
#[derive(Debug)]
struct DesktopEntry {
    /// Where it is.
    path: PathBuf,
}

impl DesktopEntry {
    /// The entry at `path`.
    const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns whether the entry is the application `app_id`'s: by name,
    /// in any case or as a reverse-DNS name's last part, or by its
    /// `StartupWMClass`, which X11 applications' entries give.
    fn belongs_to(&self, app_id: &str) -> bool {
        let Some(stem) = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".desktop"))
        else {
            return false;
        };
        let last = stem.rsplit('.').next().unwrap_or(stem);
        stem.eq_ignore_ascii_case(app_id)
            || last.eq_ignore_ascii_case(app_id)
            || self
                .value("StartupWMClass")
                .is_some_and(|class| class.eq_ignore_ascii_case(app_id))
    }

    /// Returns the value of `key` in the `[Desktop Entry]` group.
    fn value(&self, key: &str) -> Option<String> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let mut is_in_entry = false;
        for line in text.lines() {
            if line.starts_with('[') {
                is_in_entry = line.trim_end() == "[Desktop Entry]";
            } else if is_in_entry
                && let Some((name, value)) = line.split_once('=')
                && name.trim_end() == key
            {
                return Some(value.trim().to_owned());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::Color;

    /// A data directory to fill.
    struct DataDirectory(tempfile::TempDir);

    impl DataDirectory {
        /// An empty one.
        fn new() -> Self {
            Self(tempfile::tempdir().expect("a temporary directory"))
        }

        /// Writes a PNG `pixels` wide at `relative`, colored `color`.
        fn picture(&self, relative: &str, pixels: u32, color: Color) {
            let path = self.0.path().join(relative);
            std::fs::create_dir_all(path.parent().expect("in a directory")).expect("created");
            let mut pixmap = Pixmap::new(pixels, pixels).expect("an area");
            pixmap.fill(color.to_skia());
            pixmap.save_png(&path).expect("written");
        }

        /// Writes `text` at `relative`.
        fn file(&self, relative: &str, text: &str) {
            let path = self.0.path().join(relative);
            std::fs::create_dir_all(path.parent().expect("in a directory")).expect("created");
            std::fs::write(path, text).expect("written");
        }

        /// A store looking only here.
        fn store(&self) -> IconStore {
            IconStore::new(vec![self.0.path().to_path_buf()])
        }
    }

    /// Returns the color of the middle of `icon`.
    fn middle(icon: &Icon) -> (u8, u8, u8) {
        let size = icon.size();
        let pixel = icon
            .pixels()
            .pixel(size / 2, size / 2)
            .expect("inside")
            .demultiply();
        (pixel.red(), pixel.green(), pixel.blue())
    }

    #[test]
    fn the_closest_size_at_least_as_big_is_chosen() {
        let data = DataDirectory::new();
        data.picture(
            "icons/hicolor/16x16/apps/editor.png",
            16,
            Color::rgb(255, 0, 0),
        );
        data.picture(
            "icons/hicolor/32x32/apps/editor.png",
            32,
            Color::rgb(0, 255, 0),
        );
        data.picture(
            "icons/hicolor/64x64/apps/editor.png",
            64,
            Color::rgb(0, 0, 255),
        );
        let mut store = data.store();
        let icon = store.named("editor", 18).expect("found");
        assert_eq!(icon.size(), 18);
        assert_eq!(middle(&icon), (0, 255, 0), "32 is the closest above 18");
        let big = store.named("editor", 100).expect("found");
        assert_eq!(middle(&big), (0, 0, 255), "none is that big: the biggest");
    }

    #[test]
    fn an_app_id_finds_its_icon_through_its_desktop_entry() {
        let data = DataDirectory::new();
        data.picture(
            "icons/hicolor/48x48/apps/utilities-terminal.png",
            48,
            Color::rgb(255, 0, 0),
        );
        data.file(
            "applications/org.example.Terminal.desktop",
            "[Desktop Entry]\nName=Terminal\nIcon=utilities-terminal\n\
             [Desktop Action new]\nIcon=other\n",
        );
        data.file(
            "applications/legacy.desktop",
            "[Desktop Entry]\nIcon=utilities-terminal\nStartupWMClass=XTerm\n",
        );
        let mut store = data.store();
        for app_id in ["org.example.Terminal", "terminal", "xterm"] {
            assert!(store.app_icon(app_id, 20).is_some(), "{app_id}");
        }
        assert!(store.app_icon("nothing", 20).is_none());
    }

    #[test]
    fn without_an_entry_the_app_id_is_tried_as_a_name() {
        let data = DataDirectory::new();
        data.picture("pixmaps/kitty.png", 32, Color::rgb(255, 0, 0));
        assert!(data.store().app_icon("kitty", 20).is_some());
    }

    #[test]
    fn lookups_are_kept_until_forgotten() {
        let data = DataDirectory::new();
        let mut store = data.store();
        assert!(store.named("late", 20).is_none());
        data.picture("pixmaps/late.png", 32, Color::rgb(255, 0, 0));
        assert!(store.named("late", 20).is_none(), "the miss is kept");
        store.forget();
        assert!(store.named("late", 20).is_some(), "found once forgotten");
    }
}
