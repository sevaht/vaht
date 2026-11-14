//! GPU buffers (`zwp_linux_dmabuf_v1`): how GL and Vulkan clients hand
//! vaht their frames.
//!
//! The global exists only when the backend's renderer can import them (the
//! nested backend's GL renderer can, the headless software one cannot), so
//! clients fall back to shared memory where it cannot. Explicit sync
//! (`wp_linux_drm_syncobj_manager_v1`) is offered on a monitor's GPU that
//! can wait on its timelines; [`crate::buffer_fences`] waits for them.

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::drm_syncobj::{DrmSyncobjHandler, DrmSyncobjState};

use crate::Compositor;
use crate::backend::Backend;

impl DmabufHandler for Compositor {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.protocols.dmabuf
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        if self.backend.import_dmabuf(&dmabuf) {
            // Dropping the token without failing reports success.
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}

impl DrmSyncobjHandler for Compositor {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        match &mut self.backend {
            Backend::Drm(backend) => backend.syncobj(),
            Backend::Winit(_) | Backend::Headless(_) => None,
        }
    }
}
