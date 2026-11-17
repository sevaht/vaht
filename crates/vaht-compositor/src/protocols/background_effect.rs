//! `ext_background_effect_manager_v1`: programs asking for what is behind
//! part of a surface to be blurred (`[blur]`).
//!
//! Smithay serves the protocol and keeps each surface's blur region with
//! its committed state; the scene reads it there when drawing
//! ([`crate::scene`]'s blur elements). vaht only says whether it blurs.

use smithay::wayland::background_effect::{Capability, ExtBackgroundEffectHandler};

use crate::Compositor;

impl ExtBackgroundEffectHandler for Compositor {
    fn capabilities(&self) -> Capability {
        // Told to each program as it binds: one bound while blur was off
        // learns it is on only when it binds again.
        if self.config.settings.blur.enabled {
            Capability::Blur
        } else {
            Capability::empty()
        }
    }
}
