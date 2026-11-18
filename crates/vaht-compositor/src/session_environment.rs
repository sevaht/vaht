//! The session's environment, for programs vaht starts and for those the
//! user's service manager starts.
//!
//! Toolkits are asked to prefer Wayland and fall back to X11
//! ([`TOOLKIT_DEFAULTS`], each kept if already set, as fluxland did).
//! Portals and other D-Bus-activated services are not vaht's children, so
//! they learn the session's variables from the D-Bus and systemd
//! activation environment, which vaht updates when it runs the session
//! (on the DRM backend; a nested or headless vaht must not point the
//! user's services at itself) and empties on exit
//! (`[session] update_activation_environment`).
//!
//! When it does, vaht also starts its session target as the session starts
//! and stops it as it ends ([`SESSION_TARGET`], `data/vaht-session.target`):
//! services that are `PartOf=graphical-session.target`, the desktop portals
//! among them, then live as long as the session, rather than outliving the
//! vaht they were started for and serving the next one stale.

/// What toolkits are told, unless already told otherwise.
pub(crate) const TOOLKIT_DEFAULTS: [(&str, &str); 3] = [
    // Qt and SDL: Wayland first, X11 if not.
    ("QT_QPA_PLATFORM", "wayland;xcb"),
    ("SDL_VIDEODRIVER", "wayland,x11"),
    // Java's toolkit draws grey windows under a non-reparenting window
    // manager unless told it is one.
    ("_JAVA_AWT_WM_NONREPARENTING", "1"),
];

use crate::config::Settings;

/// The settings that start and stop the parts of a session, so a change of
/// them while vaht runs is noticed ([`crate::Compositor`] acts on it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveSessionSettings {
    /// Whether Xwayland runs.
    pub(crate) xwayland_enabled: bool,
    /// The X resources file, as written.
    pub(crate) x_resources: Option<String>,
    /// When the activation environment is told of the session.
    pub(crate) update_activation: ActivationUpdate,
}

impl LiveSessionSettings {
    /// Returns what `settings` say of the session.
    pub(crate) fn of(settings: &Settings) -> Self {
        Self {
            xwayland_enabled: settings.xwayland.enabled,
            x_resources: settings.xwayland.resources.clone(),
            update_activation: settings.session.update_activation_environment,
        }
    }
}

/// vaht's session target, binding `graphical-session.target`.
pub(crate) const SESSION_TARGET: &str = "vaht-session.target";

/// The variables exported to the activation environment.
const EXPORTED: [&str; 6] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    vaht_ipc::SOCKET_VARIABLE,
];

/// When vaht updates the activation environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ActivationUpdate {
    /// When it runs the session: on the DRM backend.
    #[default]
    Auto,
    /// Always, nested too.
    Always,
    /// Never.
    Never,
}

impl ActivationUpdate {
    /// Returns whether vaht updates the activation environment, running
    /// the session itself (`owns_seat`) or not.
    pub(crate) const fn applies(self, owns_seat: bool) -> bool {
        match self {
            Self::Auto => owns_seat,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

/// Returns the command line that updates the activation environment: each
/// exported variable `is_set` says is set exported at its value, or every
/// one emptied if `is_clearing`.
pub(crate) fn update_command(is_clearing: bool, is_set: impl Fn(&str) -> bool) -> String {
    let mut words = vec![
        "dbus-update-activation-environment".to_owned(),
        "--systemd".to_owned(),
    ];
    for name in EXPORTED {
        if is_clearing {
            // NAME= empties it for services started from now on.
            words.push(format!("{name}="));
        } else if is_set(name) {
            // NAME alone exports its value in the command's environment.
            words.push(name.to_owned());
        }
    }
    words.join(" ")
}

/// Returns the command that empties `DISPLAY` in the activation
/// environment: Xwayland was turned off, so services started from now on
/// must not look for it.
pub(crate) const fn forget_display_command() -> &'static str {
    "dbus-update-activation-environment --systemd DISPLAY="
}

/// Returns `export`, the command updating the activation environment as
/// the session starts, followed by starting the session target afresh:
/// stopped first, in case a vaht that crashed left it running.
pub(crate) fn starting_command(export: &str) -> String {
    format!(
        "{export} && {{ systemctl --user stop {SESSION_TARGET}; \
         systemctl --user start {SESSION_TARGET} || \
         echo 'vaht: {SESSION_TARGET} is not installed; the desktop portals \
         are not tied to the session' >&2; }}"
    )
}

/// Returns `clearing`, the command emptying the activation environment as
/// the session ends, after stopping the session target.
pub(crate) fn ending_command(clearing: &str) -> String {
    format!("systemctl --user stop {SESSION_TARGET}; {clearing}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_is_exported_when_vaht_runs_it() {
        assert!(ActivationUpdate::Auto.applies(true));
        assert!(!ActivationUpdate::Auto.applies(false), "nested or headless");
        assert!(ActivationUpdate::Always.applies(false));
        assert!(!ActivationUpdate::Never.applies(true));
    }

    #[test]
    fn only_the_variables_set_are_exported() {
        let command = update_command(false, |name| name != "DISPLAY");
        assert_eq!(
            command,
            "dbus-update-activation-environment --systemd WAYLAND_DISPLAY \
             XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP XDG_SESSION_TYPE VAHT_SOCKET"
        );
    }

    #[test]
    fn the_session_target_starts_after_the_export_and_stops_before_the_clearing() {
        let starting = starting_command("export");
        assert!(
            starting.starts_with(
                "export && { systemctl --user stop vaht-session.target; \
                 systemctl --user start vaht-session.target"
            ),
            "{starting}"
        );
        assert_eq!(
            ending_command("clear"),
            "systemctl --user stop vaht-session.target; clear"
        );
    }

    #[test]
    fn on_exit_every_variable_is_emptied() {
        let command = update_command(true, |_| false);
        assert!(command.ends_with(
            "WAYLAND_DISPLAY= DISPLAY= XDG_CURRENT_DESKTOP= XDG_SESSION_DESKTOP= \
             XDG_SESSION_TYPE= VAHT_SOCKET="
        ));
    }
}
