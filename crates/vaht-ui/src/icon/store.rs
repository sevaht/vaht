//! Finding icons by name, and applications' icons by app id.
//!
//! [`IconStore`] looks the way desktops do, as the icon theme and desktop
//! entry specifications describe, and then somewhat further, so a window
//! that has an icon anywhere on the system shows it:
//!
//! - **Where:** the directory an application brings its own icons in (a tray
//!   item's `IconThemePath`), then the icon themes in `~/.icons` and each XDG
//!   data directory's `icons` (and the Flatpak and Snap ones, which a session
//!   not started through a login shell may lack in `XDG_DATA_DIRS`), then the
//!   `pixmaps` directories.
//! - **Which theme:** the one named (the GTK settings', by default), the
//!   themes it `Inherits`, `hicolor`, and then every other theme installed:
//!   an application's icon often lives in only one theme.
//! - **Which file:** a theme's `index.theme` says its directories and their
//!   sizes; a theme without one is read as `hicolor` is laid out (`NxN` and
//!   `scalable`, each with `apps`, `status`, `devices` and `panel`). The size
//!   asked for is matched exactly, else a scalable picture (drawn at the size,
//!   sharper than any picture resized), else the closest bigger picture, else
//!   the closest smaller.
//! - **Which format:** PNG, SVG (and gzipped SVG), XPM, and the other raster
//!   formats [`crate::raster`] reads, in `pixmaps` and an application's own
//!   directory; themes hold PNG, SVG and XPM.
//! - **Which name:** as written, without an extension it was given, and in
//!   lower case (a class `Firefox` has an icon `firefox`).
//! - **Which entry:** an app id finds its desktop entry by file name (exactly,
//!   in any case, or as the last part of a reverse-DNS name), by its
//!   `StartupWMClass`, by the program its `Exec` runs, or by its `Name`, in
//!   that order, in the entries of every data directory and the directories
//!   in them. The entry's `Icon` names the icon, and without one the app id
//!   itself is tried as a name.
//!
//! Lookups are cached, misses too, until [`IconStore::forget`].

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::Icon;

/// The file name extensions of the pictures vaht reads as icons, in the order
/// they are tried.
const EXTENSIONS: [&str; 10] = [
    "png", "svg", "svgz", "xpm", "ico", "webp", "jpg", "jpeg", "bmp", "gif",
];

/// The extensions an icon theme's directories hold pictures in.
const THEME_EXTENSIONS: [&str; 4] = ["png", "svg", "svgz", "xpm"];

/// One directory of an icon theme, and the sizes it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ThemeDirectory {
    /// Where it is.
    path: PathBuf,
    /// The size its pictures are drawn for.
    size: u32,
    /// The sizes a scalable directory's pictures may be drawn at, if it is
    /// one; none for a directory of pictures of one size.
    scalable_range: Option<(u32, u32)>,
}

impl ThemeDirectory {
    /// Returns how well the directory suits `size`, best first: a picture
    /// exactly that size, then a scalable one, then the closest bigger
    /// picture, then the closest smaller.
    fn rank(&self, size: u32) -> (u8, u32) {
        if self.scalable_range.is_some() {
            return (1, 0);
        }
        match self.size.cmp(&size) {
            std::cmp::Ordering::Equal => (0, 0),
            std::cmp::Ordering::Greater => (2, self.size - size),
            std::cmp::Ordering::Less => (3, size - self.size),
        }
    }
}

/// A desktop entry's keys that find its icon.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct EntryInfo {
    /// The file's name without `.desktop`.
    stem: String,
    /// The `Icon`.
    icon: Option<String>,
    /// The `StartupWMClass`, which X11 applications' entries give.
    window_class: Option<String>,
    /// The name of the program `Exec` runs.
    program: Option<String>,
    /// Whether `Exec` runs it with nothing but field codes (`%U`) after it:
    /// the application itself, not one of its parts (a web app a browser
    /// installed runs the browser with `--app-id=...`).
    is_plain: bool,
    /// The `Name`.
    name: Option<String>,
}

/// Finds and keeps icons, from the XDG data directories.
#[derive(Debug, Default)]
pub struct IconStore {
    /// Where to look, most important first.
    data_directories: Vec<PathBuf>,
    /// The directories holding icon themes, most important first.
    theme_roots: Vec<PathBuf>,
    /// The icon theme to prefer, if one is named.
    theme: Option<String>,
    /// Icons found (or not) by name, size and the application's own
    /// directory.
    by_name: HashMap<(String, u32, Option<PathBuf>), Option<Icon>>,
    /// Icon names found (or not) by app id.
    names_by_app_id: HashMap<String, Option<String>>,
    /// The themes to look in, in order; read when first needed.
    themes: Option<Vec<String>>,
    /// Each theme's directories, by theme.
    theme_directories: HashMap<String, Vec<ThemeDirectory>>,
    /// The desktop entries; read when first needed.
    entries: Option<Vec<EntryInfo>>,
}

impl IconStore {
    /// The theme directories icons are looked for in: applications', and
    /// the status icons tray items show.
    const CONTEXTS: [&str; 4] = ["apps", "status", "devices", "panel"];

    /// The data directories when `XDG_DATA_DIRS` names none, as the XDG
    /// base directory specification says.
    const DEFAULT_DATA_DIRECTORIES: [&str; 2] = ["/usr/local/share", "/usr/share"];

    /// Where Flatpak and Snap put the entries and icons they export, which a
    /// session's `XDG_DATA_DIRS` does not always name.
    const EXTRA_DATA_DIRECTORIES: [&str; 2] =
        ["/var/lib/flatpak/exports/share", "/var/lib/snapd/desktop"];

    /// Creates a store looking in `data_directories`, most important first.
    #[must_use]
    pub fn new(data_directories: Vec<PathBuf>) -> Self {
        let theme_roots = data_directories
            .iter()
            .map(|directory| directory.join("icons"))
            .collect();
        Self {
            data_directories,
            theme_roots,
            ..Self::default()
        }
    }

    /// Creates a store looking where the environment says: `XDG_DATA_HOME`
    /// (or `~/.local/share`), then `XDG_DATA_DIRS`, and the icon theme the
    /// GTK settings name.
    #[must_use]
    pub fn from_environment() -> Self {
        let variable = |name| std::env::var_os(name).filter(|value| !value.is_empty());
        let user_home = variable("HOME").map(PathBuf::from);
        let home = variable("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| user_home.as_ref().map(|home| home.join(".local/share")));
        let shared: Vec<PathBuf> = variable("XDG_DATA_DIRS").map_or_else(
            || {
                Self::DEFAULT_DATA_DIRECTORIES
                    .iter()
                    .map(PathBuf::from)
                    .collect()
            },
            |directories| std::env::split_paths(&directories).collect(),
        );
        let mut directories: Vec<PathBuf> = Vec::new();
        let flatpak = user_home
            .iter()
            .map(|home| home.join(".local/share/flatpak/exports/share"))
            .chain(Self::EXTRA_DATA_DIRECTORIES.iter().map(PathBuf::from));
        for directory in home.into_iter().chain(shared).chain(flatpak) {
            if directory.is_absolute() && !directories.contains(&directory) {
                directories.push(directory);
            }
        }
        let mut store = Self::new(directories);
        // The icon directory older programs expect in the home directory.
        if let Some(home) = &user_home {
            store.theme_roots.insert(0, home.join(".icons"));
        }
        let config = variable("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| user_home.map(|home| home.join(".config")));
        store.theme = config.as_deref().and_then(Self::gtk_icon_theme);
        store
    }

    /// Prefers the icon theme `name`, or none to go back to the GTK
    /// settings' (as [`IconStore::from_environment`] found them).
    pub fn set_theme(&mut self, name: Option<String>) {
        self.theme = name;
        self.forget();
    }

    /// Returns the icon theme the GTK settings in `config` (`~/.config`)
    /// name, if they name one.
    fn gtk_icon_theme(config: &Path) -> Option<String> {
        ["gtk-3.0", "gtk-4.0"].iter().find_map(|version| {
            let text = std::fs::read_to_string(config.join(version).join("settings.ini")).ok()?;
            text.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "gtk-icon-theme-name")
                    .then(|| value.trim().to_owned())
                    .filter(|name| !name.is_empty())
            })
        })
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
        // The entry's icon, else the app id as a name; and a reverse-DNS app
        // id's last part, which is how such applications often name theirs.
        let last = app_id.rsplit('.').next().unwrap_or(app_id);
        name.iter()
            .map(String::as_str)
            .chain([app_id, last])
            .find_map(|name| self.named(name, size))
    }

    /// Returns the icon named `name` (or at the absolute path `name`),
    /// `size` wide, if one is found.
    pub fn named(&mut self, name: &str, size: u32) -> Option<Icon> {
        self.named_in(name, size, None)
    }

    /// Returns the icon named `name`, `size` wide, looking first in
    /// `own_directory`, the icons an application brings along (a tray
    /// item's `IconThemePath`): its own files, or a theme inside it.
    pub fn named_in(
        &mut self,
        name: &str,
        size: u32,
        own_directory: Option<&Path>,
    ) -> Option<Icon> {
        let key = (name.to_owned(), size, own_directory.map(Path::to_path_buf));
        if let Some(icon) = self.by_name.get(&key) {
            return icon.clone();
        }
        let icon = self.find_file(name, size, own_directory).and_then(|path| {
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
        self.themes = None;
        self.theme_directories.clear();
        self.entries = None;
    }

    /// Returns `name` without a picture's extension it was given (`foo.png`
    /// is the icon `foo`), and in lower case: the names to look for.
    fn name_candidates(name: &str) -> Vec<String> {
        let stem = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| {
                EXTENSIONS
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(extension))
            })
            .and_then(|_| name.rsplit_once('.').map(|(stem, _)| stem))
            .unwrap_or(name);
        let mut names = vec![stem.to_owned()];
        let lower = stem.to_lowercase();
        if lower != stem {
            names.push(lower);
        }
        names
    }

    /// Returns the file of the icon `name` best for `size`, from
    /// `own_directory` first, then from the themes, then from `pixmaps`.
    fn find_file(
        &mut self,
        name: &str,
        size: u32,
        own_directory: Option<&Path>,
    ) -> Option<PathBuf> {
        let path = Path::new(name);
        if path.is_absolute() {
            return path.is_file().then(|| path.to_path_buf());
        }
        if name.contains('/') || name.is_empty() {
            return None;
        }
        let own = own_directory.filter(|directory| directory.is_absolute());
        for candidate in Self::name_candidates(name) {
            if let Some(file) = self.find_candidate(&candidate, size, own) {
                return Some(file);
            }
        }
        None
    }

    /// Returns the file of the icon `name` (one name to look for) best for
    /// `size`.
    fn find_candidate(&mut self, name: &str, size: u32, own: Option<&Path>) -> Option<PathBuf> {
        let in_directory = |directory: &Path, extensions: &[&str]| {
            extensions
                .iter()
                .map(|extension| directory.join(format!("{name}.{extension}")))
                .find(|file| file.is_file())
        };
        if let Some(directory) = own
            && let Some(file) = in_directory(directory, &EXTENSIONS)
        {
            return Some(file);
        }
        if let Some(directory) = own {
            let own_themes = [directory.join("hicolor"), directory.to_path_buf()];
            if let Some(file) = own_themes.iter().find_map(|theme| {
                let directories = Self::guessed_directories(theme);
                Self::best_in(&directories, name, size)
            }) {
                return Some(file);
            }
        }
        for theme in self.theme_names() {
            let directories = self.directories_of(&theme);
            if let Some(file) = Self::best_in(&directories, name, size) {
                return Some(file);
            }
        }
        self.data_directories
            .iter()
            .find_map(|directory| in_directory(&directory.join("pixmaps"), &EXTENSIONS))
    }

    /// Returns the themes to look in, in order: the one named, what it
    /// inherits, `hicolor`, and every other theme installed.
    fn theme_names(&mut self) -> Vec<String> {
        if let Some(names) = &self.themes {
            return names.clone();
        }
        let mut names: Vec<String> = Vec::new();
        let mut queue: Vec<String> = self.theme.iter().cloned().collect();
        queue.push("hicolor".to_owned());
        while !queue.is_empty() {
            let name = queue.remove(0);
            if names.contains(&name) {
                continue;
            }
            let inherited = self.index_of(&name).map(|index| index.inherits);
            names.push(name);
            queue.splice(0..0, inherited.unwrap_or_default());
        }
        // Every other theme there is, by name.
        let others: BTreeSet<String> = self
            .theme_roots
            .iter()
            .filter_map(|root| std::fs::read_dir(root).ok())
            .flat_map(|entries| entries.filter_map(Result::ok))
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        let fresh: Vec<String> = others
            .into_iter()
            .filter(|name| !names.contains(name))
            .collect();
        names.extend(fresh);
        self.themes = Some(names.clone());
        names
    }

    /// Returns where the theme `name` is: each root it has a directory in.
    fn theme_paths(&self, name: &str) -> Vec<PathBuf> {
        self.theme_roots
            .iter()
            .map(|root| root.join(name))
            .filter(|path| path.is_dir())
            .collect()
    }

    /// Returns what the first `index.theme` of the theme `name` says.
    fn index_of(&self, name: &str) -> Option<ThemeIndex> {
        self.theme_paths(name).iter().find_map(|path| {
            let text = std::fs::read_to_string(path.join("index.theme")).ok()?;
            Some(ThemeIndex::parse(&text))
        })
    }

    /// Returns the directories of the theme `name`, as its `index.theme`
    /// lists them, or, for a theme without one, as `hicolor` is laid out.
    fn directories_of(&mut self, name: &str) -> Vec<ThemeDirectory> {
        if let Some(directories) = self.theme_directories.get(name) {
            return directories.clone();
        }
        let paths = self.theme_paths(name);
        let index = self.index_of(name);
        let directories: Vec<ThemeDirectory> = paths
            .iter()
            .flat_map(|path| match &index {
                Some(index) if !index.directories.is_empty() => index
                    .directories
                    .iter()
                    .map(|listed| ThemeDirectory {
                        path: path.join(&listed.relative),
                        size: listed.size,
                        scalable_range: listed.scalable_range,
                    })
                    .collect::<Vec<_>>(),
                Some(_) | None => Self::guessed_directories(path),
            })
            .collect();
        self.theme_directories
            .insert(name.to_owned(), directories.clone());
        directories
    }

    /// Returns the directories of the theme at `theme` laid out as `hicolor`
    /// is: `NxN/<context>/` and `scalable/<context>/`.
    fn guessed_directories(theme: &Path) -> Vec<ThemeDirectory> {
        let Ok(entries) = std::fs::read_dir(theme) else {
            return Vec::new();
        };
        let mut directories = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let is_scalable = name == "scalable";
            let pixels = name.split_once('x').and_then(|(width, height)| {
                let pixels: u32 = width.parse().ok()?;
                (height.parse::<u32>().ok()? == pixels).then_some(pixels)
            });
            if !is_scalable && pixels.is_none() {
                continue;
            }
            for context in Self::CONTEXTS {
                directories.push(ThemeDirectory {
                    path: entry.path().join(context),
                    size: pixels.unwrap_or(0),
                    scalable_range: is_scalable.then_some((1, u32::MAX)),
                });
            }
        }
        directories
    }

    /// Returns the file of the icon `name` best for `size` in `directories`:
    /// a picture exactly that big, then a scalable one, then the closest
    /// bigger picture, then the closest smaller.
    fn best_in(directories: &[ThemeDirectory], name: &str, size: u32) -> Option<PathBuf> {
        let mut ranked: Vec<&ThemeDirectory> = directories.iter().collect();
        ranked.sort_by_key(|directory| directory.rank(size));
        ranked.into_iter().find_map(|directory| {
            THEME_EXTENSIONS
                .iter()
                .map(|extension| directory.path.join(format!("{name}.{extension}")))
                .find(|file| file.is_file())
        })
    }

    /// Returns the icon name the desktop entry of `app_id` gives, if it
    /// has one.
    fn icon_name_of(&mut self, app_id: &str) -> Option<String> {
        let entries = self.entries.get_or_insert_with(|| {
            let mut entries = Vec::new();
            for directory in &self.data_directories {
                Self::read_entries(&directory.join("applications"), 3, &mut entries);
            }
            entries
        });
        let last = |stem: &str| stem.rsplit('.').next().unwrap_or(stem).to_owned();
        let same = |text: &Option<String>| {
            text.as_deref()
                .is_some_and(|text| text.eq_ignore_ascii_case(app_id))
        };
        let passes: [&dyn Fn(&EntryInfo) -> bool; 5] = [
            &|entry| entry.stem == app_id,
            &|entry| {
                entry.stem.eq_ignore_ascii_case(app_id)
                    || last(&entry.stem).eq_ignore_ascii_case(app_id)
            },
            &|entry| same(&entry.window_class),
            &|entry| same(&entry.program),
            &|entry| same(&entry.name),
        ];
        // Of entries that match, one that is the application itself comes
        // before one that runs it with arguments (a browser's web apps run
        // the browser too).
        passes.iter().find_map(|matches| {
            let found = || {
                entries
                    .iter()
                    .filter(|entry| matches(entry) && entry.icon.is_some())
            };
            found()
                .find(|entry| entry.is_plain)
                .or_else(|| found().next())
                .and_then(|entry| entry.icon.clone())
        })
    }

    /// Reads the desktop entries in `directory`, and in the directories in
    /// it (an entry may be in a subdirectory), `depth` levels down, into
    /// `entries`, sorted by name so the result does not depend on the
    /// order the file system lists them in.
    fn read_entries(directory: &Path, depth: u32, entries: &mut Vec<EntryInfo>) {
        let Ok(listing) = std::fs::read_dir(directory) else {
            return;
        };
        let mut paths: Vec<PathBuf> = listing
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                if depth > 0 {
                    Self::read_entries(&path, depth - 1, entries);
                }
            } else if let Some(entry) = EntryInfo::read(&path) {
                entries.push(entry);
            }
        }
    }
}

impl EntryInfo {
    /// Reads the desktop entry at `path`, if it is one.
    fn read(path: &Path) -> Option<Self> {
        let stem = path.file_name()?.to_str()?.strip_suffix(".desktop")?;
        let text = std::fs::read_to_string(path).ok()?;
        let mut entry = Self {
            stem: stem.to_owned(),
            ..Self::default()
        };
        let mut is_in_entry = false;
        for line in text.lines() {
            if line.starts_with('[') {
                is_in_entry = line.trim_end() == "[Desktop Entry]";
            } else if is_in_entry && let Some((key, value)) = line.split_once('=') {
                let value = value.trim();
                match key.trim_end() {
                    "Icon" => entry.icon = Some(value.to_owned()),
                    "StartupWMClass" => entry.window_class = Some(value.to_owned()),
                    "Name" => entry.name = Some(value.to_owned()),
                    "Exec" => {
                        entry.program = Self::program_of(value);
                        entry.is_plain = Self::is_plain(value);
                    }
                    _ => {}
                }
            }
        }
        Some(entry)
    }

    /// Returns whether the `Exec` line `exec` gives its program no arguments
    /// but field codes.
    fn is_plain(exec: &str) -> bool {
        let mut words = exec.split_whitespace();
        words
            .find(|word| *word != "env" && !word.contains('=') && !word.starts_with('%'))
            .is_some()
            && words.all(|word| word.starts_with('%'))
    }

    /// Returns the name of the program the `Exec` line `exec` runs: the first
    /// word that is not an environment assignment or the `env` command,
    /// without its directory.
    fn program_of(exec: &str) -> Option<String> {
        exec.split_whitespace()
            .find(|word| *word != "env" && !word.contains('=') && !word.starts_with('%'))
            .map(|word| {
                word.trim_matches('"')
                    .rsplit('/')
                    .next()
                    .unwrap_or(word)
                    .to_owned()
            })
            .filter(|name| !name.is_empty())
    }
}

/// What a theme's `index.theme` says.
#[derive(Debug, Default)]
struct ThemeIndex {
    /// The themes it inherits, in order.
    inherits: Vec<String>,
    /// Its directories: the path in the theme, and the size, and, for a
    /// scalable one, the sizes it may be drawn at. Directories drawn for a
    /// higher resolution screen (`Scale` above 1) are left out.
    directories: Vec<IndexedDirectory>,
}

/// A directory an `index.theme` lists.
#[derive(Debug)]
struct IndexedDirectory {
    /// Its path in the theme.
    relative: String,
    /// The size its pictures are drawn for.
    size: u32,
    /// For a scalable one, the sizes it may be drawn at.
    scalable_range: Option<(u32, u32)>,
}

impl ThemeIndex {
    /// Reads `text`, an `index.theme`.
    fn parse(text: &str) -> Self {
        let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                sections.push((name.to_owned(), HashMap::new()));
            } else if let Some((key, value)) = line.split_once('=')
                && let Some((_, keys)) = sections.last_mut()
            {
                keys.insert(key.trim().to_owned(), value.trim().to_owned());
            }
        }
        let list = |text: Option<&String>| -> Vec<String> {
            text.map(|text| {
                text.split(',')
                    .map(|part| part.trim().to_owned())
                    .filter(|part| !part.is_empty())
                    .collect()
            })
            .unwrap_or_default()
        };
        let mut index = Self::default();
        let Some((_, theme)) = sections.iter().find(|(name, _)| name == "Icon Theme") else {
            return index;
        };
        index.inherits = list(theme.get("Inherits"));
        for relative in list(theme.get("Directories")) {
            let Some((_, keys)) = sections.iter().find(|(name, _)| *name == relative) else {
                continue;
            };
            let number = |key: &str| keys.get(key).and_then(|value| value.parse::<u32>().ok());
            if number("Scale").is_some_and(|scale| scale > 1) {
                continue;
            }
            let Some(size) = number("Size") else {
                continue;
            };
            let scalable_range =
                (keys.get("Type").map(String::as_str) == Some("Scalable")).then(|| {
                    (
                        number("MinSize").unwrap_or(size),
                        number("MaxSize").unwrap_or(size),
                    )
                });
            index.directories.push(IndexedDirectory {
                relative,
                size,
                scalable_range,
            });
        }
        index
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use std::fmt::Write as _;

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
    fn a_scalable_icon_beats_resizing_a_picture_but_not_an_exact_size() {
        let data = DataDirectory::new();
        data.picture(
            "icons/hicolor/16x16/apps/editor.png",
            16,
            Color::rgb(255, 0, 0),
        );
        data.picture(
            "icons/hicolor/256x256/apps/editor.png",
            256,
            Color::rgb(0, 255, 0),
        );
        data.file(
            "icons/hicolor/scalable/apps/editor.svg",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">\
             <rect width=\"10\" height=\"10\" fill=\"#0000ff\"/></svg>",
        );
        let mut store = data.store();
        let drawn = store.named("editor", 18).expect("found");
        assert_eq!(drawn.size(), 18);
        assert_eq!(middle(&drawn), (0, 0, 255), "the SVG, drawn at 18");
        let exact = store.named("editor", 16).expect("found");
        assert_eq!(middle(&exact), (255, 0, 0), "a picture exactly 16");
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
    fn an_xpm_in_pixmaps_is_an_icon_found_through_the_desktop_entry() {
        // How xterm installs its icon: an XPM in pixmaps, named by an entry
        // that matches the window's class.
        let data = DataDirectory::new();
        data.file(
            "pixmaps/xterm-color_48x48.xpm",
            "/* XPM */\nstatic char *icon[] = {\n\"2 2 2 1\",\n\"a c #ff0000\",\n\"b c None\",\n\
             \"ab\",\n\"ba\"\n};\n",
        );
        data.file(
            "applications/xterm.desktop",
            "[Desktop Entry]\nIcon=xterm-color_48x48\nStartupWMClass=XTerm\n",
        );
        let icon = data
            .store()
            .app_icon("XTerm", 20)
            .expect("the XPM is found and read");
        let pixel = icon.pixels().pixel(2, 2).expect("inside");
        assert_eq!(
            (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
            (255, 0, 0, 255),
            "the red half, opaque"
        );
        let corner = icon.pixels().pixel(17, 2).expect("inside");
        assert_eq!(corner.alpha(), 0, "the None half is clear");
    }

    #[test]
    fn without_an_entry_the_app_id_is_tried_as_a_name() {
        let data = DataDirectory::new();
        data.picture("pixmaps/kitty.png", 32, Color::rgb(255, 0, 0));
        assert!(data.store().app_icon("kitty", 20).is_some());
    }

    #[test]
    fn an_application_s_own_icons_come_first_and_status_icons_are_found() {
        let data = DataDirectory::new();
        data.picture(
            "icons/hicolor/22x22/status/network.png",
            22,
            Color::rgb(255, 0, 0),
        );
        let own = DataDirectory::new();
        own.picture("network.png", 22, Color::rgb(0, 0, 255));
        let mut store = data.store();
        let shared = store
            .named("network", 22)
            .expect("found among status icons");
        assert_eq!(middle(&shared), (255, 0, 0));
        let mine = store
            .named_in("network", 22, Some(own.0.path()))
            .expect("found in its own directory");
        assert_eq!(middle(&mine), (0, 0, 255));
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

    /// An `index.theme` listing `directories` (path, size, scalable), and
    /// inheriting `inherits`.
    fn index(inherits: &str, directories: &[(&str, u32, bool)]) -> String {
        let mut text = format!(
            "[Icon Theme]\nName=T\nInherits={inherits}\nDirectories={}\n",
            directories
                .iter()
                .map(|(path, ..)| *path)
                .collect::<Vec<_>>()
                .join(",")
        );
        for (path, size, is_scalable) in directories {
            let kind = if *is_scalable { "Scalable" } else { "Fixed" };
            writeln!(text, "[{path}]\nSize={size}\nType={kind}").expect("a string takes text");
        }
        text
    }

    #[test]
    fn a_theme_the_settings_name_is_looked_in_with_what_it_inherits() {
        let data = DataDirectory::new();
        data.file(
            "icons/Fancy/index.theme",
            &index("Base", &[("big", 48, false)]),
        );
        data.file(
            "icons/Base/index.theme",
            &index("", &[("apps/32", 32, false)]),
        );
        data.picture("icons/Base/apps/32/editor.png", 32, Color::rgb(255, 0, 0));
        data.picture("icons/Fancy/big/viewer.png", 48, Color::rgb(0, 255, 0));
        let mut store = data.store();
        store.set_theme(Some("Fancy".to_owned()));
        assert_eq!(
            middle(&store.named("viewer", 20).expect("the theme's own")),
            (0, 255, 0)
        );
        assert_eq!(
            middle(&store.named("editor", 20).expect("inherited")),
            (255, 0, 0)
        );
    }

    #[test]
    fn an_icon_only_some_other_installed_theme_has_is_still_found() {
        let data = DataDirectory::new();
        data.file(
            "icons/Elsewhere/index.theme",
            &index("", &[("apps/48", 48, false)]),
        );
        data.picture(
            "icons/Elsewhere/apps/48/tool.png",
            48,
            Color::rgb(0, 0, 255),
        );
        let mut store = data.store();
        assert_eq!(
            middle(&store.named("tool", 20).expect("found in any theme")),
            (0, 0, 255)
        );
    }

    #[test]
    fn a_themes_index_decides_where_its_pictures_are() {
        // Not the hicolor layout: sizes in directories named otherwise.
        let data = DataDirectory::new();
        data.file(
            "icons/Odd/index.theme",
            &index("", &[("small", 16, false), ("large", 64, false)]),
        );
        data.picture("icons/Odd/small/x.png", 16, Color::rgb(255, 0, 0));
        data.picture("icons/Odd/large/x.png", 64, Color::rgb(0, 255, 0));
        let mut store = data.store();
        store.set_theme(Some("Odd".to_owned()));
        assert_eq!(middle(&store.named("x", 16).expect("found")), (255, 0, 0));
        assert_eq!(middle(&store.named("x", 40).expect("found")), (0, 255, 0));
    }

    #[test]
    fn an_icon_is_found_without_the_extension_it_was_given_and_in_lower_case() {
        let data = DataDirectory::new();
        data.picture("pixmaps/firefox.png", 32, Color::rgb(255, 0, 0));
        let mut store = data.store();
        assert!(store.named("firefox.png", 20).is_some(), "an extension");
        assert!(store.named("Firefox", 20).is_some(), "a class in capitals");
        assert!(store.app_icon("Firefox", 20).is_some(), "as an app id");
    }

    #[test]
    fn pictures_of_other_formats_are_read_from_pixmaps() {
        let data = DataDirectory::new();
        data.picture("pixmaps/a.png", 16, Color::rgb(255, 0, 0));
        let png = std::fs::read(data.0.path().join("pixmaps/a.png")).expect("read");
        // The same picture in a format picked by its contents, not its name.
        std::fs::write(data.0.path().join("pixmaps/b.ico"), &png).expect("written");
        std::fs::write(data.0.path().join("pixmaps/c.jpeg"), &png).expect("written");
        let mut store = data.store();
        assert!(store.named("b", 16).is_some(), "an .ico");
        assert!(store.named("c", 16).is_some(), "a .jpeg");
    }

    #[test]
    fn an_entry_is_found_by_the_program_it_runs_and_by_its_name() {
        let data = DataDirectory::new();
        data.picture("pixmaps/shot.png", 32, Color::rgb(255, 0, 0));
        data.picture("pixmaps/pad.png", 32, Color::rgb(0, 255, 0));
        data.file(
            "applications/screenshooter.desktop",
            "[Desktop Entry]\nName=Screenshot Tool\nExec=env GDK_BACKEND=x11 /usr/bin/flameshot %U\nIcon=shot\n",
        );
        data.file(
            "applications/notes.desktop",
            "[Desktop Entry]\nName=Scratch Pad\nExec=notes\nIcon=pad\n",
        );
        let mut store = data.store();
        assert_eq!(
            middle(&store.app_icon("flameshot", 20).expect("by Exec")),
            (255, 0, 0)
        );
        assert_eq!(
            middle(&store.app_icon("scratch pad", 20).expect("by Name")),
            (0, 255, 0)
        );
    }

    #[test]
    fn the_application_comes_before_the_things_run_with_its_program() {
        // A browser's installed web apps run the browser with arguments, and
        // may be listed first; the browser's own entry is the plain one.
        let data = DataDirectory::new();
        data.picture("pixmaps/app.png", 32, Color::rgb(255, 0, 0));
        data.picture("pixmaps/web.png", 32, Color::rgb(0, 255, 0));
        data.file(
            "applications/browser-web-app.desktop",
            "[Desktop Entry]\nName=Torrents\nExec=/opt/browser/browser --app-id=abc\nIcon=web\n",
        );
        data.file(
            "applications/browser.desktop",
            "[Desktop Entry]\nName=Browser\nExec=browser %U\nIcon=app\n",
        );
        let mut store = data.store();
        assert_eq!(
            middle(&store.app_icon("browser", 20).expect("by Exec")),
            (255, 0, 0)
        );
        // With no plain entry, one that runs it with arguments still serves.
        let only = DataDirectory::new();
        only.picture("pixmaps/web.png", 32, Color::rgb(0, 255, 0));
        only.file(
            "applications/web-app.desktop",
            "[Desktop Entry]\nExec=browser --app-id=abc\nIcon=web\n",
        );
        assert!(only.store().app_icon("browser", 20).is_some());
    }

    #[test]
    fn an_entry_in_a_subdirectory_of_applications_is_found() {
        let data = DataDirectory::new();
        data.picture("pixmaps/k.png", 32, Color::rgb(255, 0, 0));
        data.file("applications/kde/kate.desktop", "[Desktop Entry]\nIcon=k\n");
        assert!(data.store().app_icon("kate", 20).is_some());
    }

    #[test]
    fn the_gtk_settings_name_the_icon_theme() {
        let config = DataDirectory::new();
        config.file(
            "gtk-3.0/settings.ini",
            "[Settings]\ngtk-theme-name=Adwaita\ngtk-icon-theme-name = Papirus\n",
        );
        assert_eq!(
            IconStore::gtk_icon_theme(config.0.path()),
            Some("Papirus".to_owned())
        );
        assert_eq!(IconStore::gtk_icon_theme(Path::new("/nonexistent")), None);
    }

    #[test]
    fn the_program_an_exec_line_runs_is_named() {
        for (exec, program) in [
            ("kitty", "kitty"),
            ("/usr/bin/kitty --single-instance %U", "kitty"),
            ("env A=1 B=2 firefox %u", "firefox"),
        ] {
            assert_eq!(
                EntryInfo::program_of(exec).as_deref(),
                Some(program),
                "{exec}"
            );
        }
    }
}
