//! Keeping the screen awake: `zwp_idle_inhibit_manager_v1`, as idle
//! (`ext_idle_notifier_v1`, which swayidle uses to blank or lock) sees it.
//!
//! A video player or a game asks that the session not go idle while its
//! surface shows. [`IdleInhibitors`] keeps the surfaces asking, and says
//! whether any of them inhibits idle now: only one alive and visible
//! counts, so a minimized player, or one on another workspace, lets the
//! screen sleep.

/// The surfaces asking that the session not go idle.
#[derive(Debug)]
pub(crate) struct IdleInhibitors<Surface> {
    /// Each surface asking, in the order asked.
    surfaces: Vec<Surface>,
}

impl<Surface: PartialEq> IdleInhibitors<Surface> {
    /// Notes that `surface` asks that the session not go idle.
    pub(crate) fn add(&mut self, surface: Surface) {
        if !self.surfaces.contains(&surface) {
            self.surfaces.push(surface);
        }
    }

    /// Notes that `surface` no longer asks.
    pub(crate) fn remove(&mut self, surface: &Surface) {
        self.surfaces.retain(|asking| asking != surface);
    }

    /// Returns whether a surface asking inhibits idle now: one `is_alive`
    /// says lives and `is_visible` says shows. Surfaces gone are
    /// forgotten.
    pub(crate) fn is_inhibiting(
        &mut self,
        is_alive: impl Fn(&Surface) -> bool,
        is_visible: impl Fn(&Surface) -> bool,
    ) -> bool {
        self.surfaces.retain(is_alive);
        self.surfaces.iter().any(is_visible)
    }
}

impl<Surface> Default for IdleInhibitors<Surface> {
    fn default() -> Self {
        Self {
            surfaces: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_visible_surface_keeps_the_screen_awake() {
        let mut inhibitors = IdleInhibitors::default();
        inhibitors.add(1);
        inhibitors.add(2);
        let alive = |_: &i32| true;
        assert!(inhibitors.is_inhibiting(alive, |surface| *surface == 2));
        assert!(!inhibitors.is_inhibiting(alive, |_| false), "both hidden");
        inhibitors.remove(&2);
        assert!(!inhibitors.is_inhibiting(alive, |surface| *surface == 2));
    }

    #[test]
    fn a_surface_gone_is_forgotten() {
        let mut inhibitors = IdleInhibitors::default();
        inhibitors.add(1);
        assert!(!inhibitors.is_inhibiting(|_| false, |_| true));
        assert!(
            !inhibitors.is_inhibiting(|_| true, |_| true),
            "it stays gone"
        );
    }
}
