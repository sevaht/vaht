//! Outputs as clients see them: `wl_output` and `xdg_output`.

use smithay::wayland::output::OutputHandler;

use crate::Compositor;

impl OutputHandler for Compositor {}
