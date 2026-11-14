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

/// What toolkits are told, unless already told otherwise.
pub(crate) const TOOLKIT_DEFAULTS: [(&str, &str); 3] = [
    // Qt and SDL: Wayland first, X11 if not.
    ("QT_QPA_PLATFORM", "wayland;xcb"),
    ("SDL_VIDEODRIVER", "wayland,x11"),
    // Java's toolkit draws grey windows under a non-reparenting window
    // manager unless told it is one.
    ("_JAVA_AWT_WM_NONREPARENTING", "1"),
];

/// The variables exported to the activation environment.
const EXPORTED: [&str; 5] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
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
             XDG_CURRENT_DESKTOP XDG_SESSION_TYPE VAHT_SOCKET"
        );
    }

    #[test]
    fn on_exit_every_variable_is_emptied() {
        let command = update_command(true, |_| false);
        assert!(command.ends_with(
            "WAYLAND_DISPLAY= DISPLAY= XDG_CURRENT_DESKTOP= XDG_SESSION_TYPE= VAHT_SOCKET="
        ));
    }
}
