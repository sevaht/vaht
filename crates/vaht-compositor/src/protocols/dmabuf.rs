//! GPU buffers (`zwp_linux_dmabuf_v1`): how GL and Vulkan clients hand
//! vaht their frames.
//!
//! The global exists only when the backend's renderer can import them (the
//! nested backend's GL renderer can, the headless software one cannot), so
//! clients fall back to shared memory where it cannot.

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};

use crate::Compositor;

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
