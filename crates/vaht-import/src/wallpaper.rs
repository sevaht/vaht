//! The wallpaper a Fluxbox `startup` script sets, as vaht's `[wallpaper]`
//! (02 §11).
//!
//! Fluxbox has no wallpaper setting of its own: a `startup` script runs a
//! program that sets one. [`StartupWallpaper::find`] looks for the first
//! line that runs `swaybg`, `feh --bg-*` or `fbsetbg` with an image, and
//! reads its image and how it fills the screen. The script is read as a
//! shell reads it, only as far as that needs: lines joined at a trailing
//! backslash, words split at blanks outside quotes, `$HOME` as `~`.

/// A wallpaper a `startup` script sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupWallpaper {
    /// The line it is set on, from 1.
    pub(crate) line: usize,
    /// The image, `~/` for the home directory.
    pub(crate) image: String,
    /// How it fills the output, as vaht's `fit` says it.
    pub(crate) fit: &'static str,
}

impl StartupWallpaper {
    /// Returns the `[wallpaper]` table that sets this wallpaper.
    pub(crate) fn settings(&self) -> toml_edit::Item {
        let mut table = toml_edit::Table::new();
        table.insert("image", toml_edit::value(self.image.clone()));
        table.insert("fit", toml_edit::value(self.fit));
        toml_edit::Item::Table(table)
    }

    /// Returns the first wallpaper `script` sets with an image vaht can
    /// name.
    pub(crate) fn find(script: &str) -> Option<Self> {
        Self::logical_lines(script)
            .into_iter()
            .find_map(|(line, text)| Self::from_command_line(line, &text))
    }

    /// Returns the script's lines, those ending in a backslash joined with
    /// the next, each with the number of its first line; comments are left
    /// out.
    fn logical_lines(script: &str) -> Vec<(usize, String)> {
        let mut lines: Vec<(usize, String)> = Vec::new();
        let mut is_continued = false;
        for (index, line) in script.lines().enumerate() {
            let (text, continues) = line
                .strip_suffix('\\')
                .map_or((line, false), |text| (text, true));
            match lines.last_mut() {
                Some((_, joined)) if is_continued => {
                    joined.push(' ');
                    joined.push_str(text);
                }
                _ if text.trim_start().starts_with('#') => {}
                _ => lines.push((index + 1, text.to_owned())),
            }
            is_continued = continues;
        }
        lines
    }

    /// Returns the wallpaper `text`, a command line on `line`, sets, if a
    /// command in it is a wallpaper setter.
    fn from_command_line(line: usize, text: &str) -> Option<Self> {
        let words = Self::words(text);
        let commands = words.split(|word| ["&&", "||", ";", "&", "|"].contains(&word.as_str()));
        commands.into_iter().find_map(|command| {
            let mut arguments = command
                .iter()
                .map(String::as_str)
                .take_while(|word| !Self::is_redirection(word));
            let program = arguments.next()?;
            let arguments: Vec<&str> = arguments.collect();
            let (image, fit) = match program {
                "swaybg" => Self::swaybg(&arguments)?,
                "feh" => Self::feh(&arguments)?,
                "fbsetbg" => Self::fbsetbg(&arguments)?,
                _ => return None,
            };
            Some(Self {
                line,
                image: Self::home_relative(image)?,
                fit,
            })
        })
    }

    /// Reads `swaybg -i <image> -m <mode>`.
    fn swaybg<'text>(arguments: &[&'text str]) -> Option<(&'text str, &'static str)> {
        let value = |names: [&str; 2]| {
            arguments
                .windows(2)
                .find(|pair| names.contains(&pair[0]))
                .map(|pair| pair[1])
        };
        let image = value(["-i", "--image"])?;
        let fit = match value(["-m", "--mode"]).unwrap_or("stretch") {
            "fill" => "fill",
            "fit" => "fit",
            "center" => "center",
            "tile" => "tile",
            _ => "stretch",
        };
        Some((image, fit))
    }

    /// Reads `feh --bg-<mode> <image>`.
    fn feh<'text>(arguments: &[&'text str]) -> Option<(&'text str, &'static str)> {
        let fit = arguments.iter().find_map(|argument| match *argument {
            "--bg-scale" => Some("stretch"),
            "--bg-fill" => Some("fill"),
            "--bg-max" => Some("fit"),
            "--bg-center" => Some("center"),
            "--bg-tile" => Some("tile"),
            _ => None,
        })?;
        let image = arguments
            .iter()
            .rev()
            .find(|argument| !argument.starts_with('-'))?;
        Some((image, fit))
    }

    /// Reads `fbsetbg -<mode> <image>`; without a mode it stretches the
    /// image over the screen, as fbsetbg's `-f` does.
    fn fbsetbg<'text>(arguments: &[&'text str]) -> Option<(&'text str, &'static str)> {
        let fit = arguments
            .iter()
            .find_map(|argument| match *argument {
                "-f" | "-F" => Some("stretch"),
                "-c" | "-C" => Some("center"),
                "-t" | "-T" => Some("tile"),
                "-a" | "-A" => Some("fit"),
                _ => None,
            })
            .unwrap_or("stretch");
        let image = arguments
            .last()
            .filter(|argument| !argument.starts_with('-'))?;
        Some((image, fit))
    }

    /// Returns `path` with `$HOME` or `${HOME}` at its start written `~`;
    /// none if it still names a variable vaht cannot know.
    fn home_relative(path: &str) -> Option<String> {
        let path = ["${HOME}", "$HOME"]
            .iter()
            .find_map(|home| path.strip_prefix(home).map(|rest| format!("~{rest}")))
            .unwrap_or_else(|| path.to_owned());
        (!path.contains('$')).then_some(path)
    }

    /// Returns whether `word` redirects input or output: the command's
    /// arguments end before it.
    fn is_redirection(word: &str) -> bool {
        let rest = word
            .trim_start_matches(|character: char| character.is_ascii_digit() || character == '&');
        rest.starts_with('>') || rest.starts_with('<')
    }

    /// Splits `text` into words at blanks outside quotes, with the quotes
    /// taken away; `;`, `&`, `&&`, `|` and `||` are words of their own.
    fn words(text: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut quote = None;
        let mut characters = text.chars().peekable();
        let finish = |word: &mut String, words: &mut Vec<String>| {
            if !word.is_empty() {
                words.push(std::mem::take(word));
            }
        };
        while let Some(character) = characters.next() {
            match (quote, character) {
                (Some(open), _) if character == open => quote = None,
                (None, '\'' | '"') => quote = Some(character),
                (None, ' ' | '\t') => finish(&mut word, &mut words),
                (None, ';') => {
                    finish(&mut word, &mut words);
                    words.push(";".to_owned());
                }
                (None, '&' | '|') if characters.peek() != Some(&'>') => {
                    finish(&mut word, &mut words);
                    let mut operator = character.to_string();
                    if characters.peek() == Some(&character) {
                        characters.next();
                        operator.push(character);
                    }
                    words.push(operator);
                }
                (Some(_) | None, _) => word.push(character),
            }
        }
        finish(&mut word, &mut words);
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// My fluxland `startup`'s wallpaper lines.
    const MY_STARTUP: &str = "\
#!/bin/bash
# fluxland's own SetWallpaper only covers the first monitor; swaybg does every output
command -v swaybg &>/dev/null && swaybg -m fill \\
    -i \"${HOME}/Pictures/wallpaper/gucken/Starry_Night_by_gucken_1920x1080.png\" &>swaybg.log &
command -v dunst &>/dev/null && dunst &>dunst.log &
";

    #[test]
    fn my_swaybg_line_is_found() {
        assert_eq!(
            StartupWallpaper::find(MY_STARTUP),
            Some(StartupWallpaper {
                line: 3,
                image: "~/Pictures/wallpaper/gucken/Starry_Night_by_gucken_1920x1080.png"
                    .to_owned(),
                fit: "fill",
            })
        );
    }

    #[test]
    fn feh_and_fbsetbg_are_read_too() {
        let feh = StartupWallpaper::find("feh --bg-tile '$HOME/tile.png' &\n").expect("found");
        assert_eq!((feh.image.as_str(), feh.fit), ("~/tile.png", "tile"));
        let fbsetbg = StartupWallpaper::find("fbsetbg -a /usr/share/sky.png\n").expect("found");
        assert_eq!(
            (fbsetbg.image.as_str(), fbsetbg.fit),
            ("/usr/share/sky.png", "fit")
        );
    }

    #[test]
    fn a_command_merely_naming_a_setter_sets_nothing() {
        assert_eq!(
            StartupWallpaper::find("command -v swaybg && echo swaybg\n"),
            None
        );
        assert_eq!(StartupWallpaper::find("# swaybg -i x.png\n"), None);
        assert_eq!(
            StartupWallpaper::find("swaybg -i \"$WALLPAPERS/x.png\"\n"),
            None,
            "a variable vaht cannot know"
        );
    }
}
