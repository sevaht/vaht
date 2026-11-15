//! Leasing headsets to the programs that drive them (`wp_drm_lease_device_v1`):
//! `SteamVR` and Monado take a VR headset's connector, a CRTC and its
//! planes, and show frames on it themselves, without vaht in between.
//!
//! vaht leaves non-desktop monitors (headsets) dark ([`super::device`]);
//! each GPU offers the ones on its connectors here. A lease ends when its
//! program lets go or quits, when the headset is unplugged, or when vaht
//! gives the GPUs up for another terminal (the program asks again once
//! vaht is back).
//!
//! Smithay looks a GPU's lease state up whenever a client speaks of it, so
//! each GPU's [`Leasing`] stays for as long as vaht runs, emptied and its
//! global hidden once the GPU is gone.

use std::collections::HashMap;

use smithay::backend::drm::{DrmDevice, DrmNode};
use smithay::reexports::drm::control::{connector, crtc};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::drm_lease::{
    DrmLease, DrmLeaseBuilder, DrmLeaseHandler, DrmLeaseRequest, DrmLeaseState, LeaseRejected,
};

use super::DrmBackend;
use crate::Compositor;
use crate::backend::Backend;

/// A headset plugged into a GPU, which vaht does not show the desktop on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Headset {
    /// Its connector.
    pub(super) connector: connector::Handle,
    /// The CRTC that would drive it.
    pub(super) crtc: crtc::Handle,
    /// The connector's name, such as `DP-2`.
    pub(super) name: String,
    /// What the headset says it is.
    pub(super) description: String,
}

/// What one GPU offers for lease, and the leases it has given.
#[derive(Debug)]
pub(super) struct Leasing {
    /// The protocol's state for the GPU.
    state: DrmLeaseState,
    /// The headsets offered.
    offered: Vec<Headset>,
    /// The leases given, revoked when dropped.
    active: Vec<DrmLease>,
}

/// Every GPU's leasing, by the GPU's node; none is ever removed.
pub(super) type Leases = HashMap<DrmNode, Leasing>;

impl Leasing {
    /// Starts offering leases on the GPU `node`, nothing yet; none if the
    /// GPU's node cannot be named to clients.
    pub(super) fn new(display: &DisplayHandle, node: DrmNode) -> Option<Self> {
        match DrmLeaseState::new::<Compositor>(display, &node) {
            Ok(state) => Some(Self {
                state,
                offered: Vec::new(),
                active: Vec::new(),
            }),
            Err(error) => {
                tracing::warn!(%error, gpu = %node, "no headset can be leased from the GPU");
                None
            }
        }
    }

    /// Offers `headsets`, the GPU's now: those new are offered, those gone
    /// withdrawn, ending their leases.
    pub(super) fn follow(&mut self, headsets: &[Headset]) {
        let gone: Vec<Headset> = self
            .offered
            .iter()
            .filter(|offered| !headsets.contains(offered))
            .cloned()
            .collect();
        for headset in gone {
            self.withdraw(&headset);
        }
        for headset in headsets {
            if !self.offered.contains(headset) {
                tracing::info!(connector = headset.name, "a headset can be leased");
                self.state.add_connector::<Compositor>(
                    headset.connector,
                    headset.name.clone(),
                    headset.description.clone(),
                );
                self.offered.push(headset.clone());
            }
        }
    }

    /// Withdraws everything and hides the global: the GPU is gone.
    pub(super) fn retire(&mut self) {
        for headset in std::mem::take(&mut self.offered) {
            self.withdraw(&headset);
        }
        self.active.clear();
        self.state.disable_global::<Compositor>();
    }

    /// Ends every lease and offers nothing until [`Self::resume`]: vaht
    /// gives the GPU up for another terminal.
    pub(super) fn suspend(&mut self) {
        self.active.clear();
        self.state.suspend();
    }

    /// Offers the headsets again.
    pub(super) fn resume(&mut self) {
        self.state.resume::<Compositor>();
    }

    /// Stops offering `headset`, ending its lease.
    fn withdraw(&mut self, headset: &Headset) {
        tracing::info!(
            connector = headset.name,
            "a headset can no longer be leased"
        );
        self.state.withdraw_connector(headset.connector);
        self.offered.retain(|offered| offered != headset);
        self.active.retain(|lease| {
            !lease
                .connectors()
                .any(|leased| *leased == headset.connector)
        });
    }

    /// Returns what to lease for `request` on `device`: each headset's
    /// connector, its CRTC and planes; rejected if it names a monitor
    /// that is not an offered headset, or a plane cannot be had.
    fn lease_for(
        &self,
        device: &DrmDevice,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected> {
        let mut builder = DrmLeaseBuilder::new(device);
        for wanted in request.connectors {
            let Some(headset) = self
                .offered
                .iter()
                .find(|headset| headset.connector == wanted)
            else {
                tracing::warn!(?wanted, "a lease asked for a monitor that is no headset");
                return Err(LeaseRejected::default());
            };
            builder.add_connector(headset.connector);
            builder.add_crtc(headset.crtc);
            let planes = device
                .planes(&headset.crtc)
                .map_err(LeaseRejected::with_cause)?;
            let (primary, claim) = planes
                .primary
                .iter()
                .find_map(|plane| {
                    device
                        .claim_plane(plane.handle, headset.crtc)
                        .map(|claim| (plane.handle, claim))
                })
                .ok_or_else(LeaseRejected::default)?;
            builder.add_plane(primary, claim);
            if let Some((cursor, claim)) = planes.cursor.iter().find_map(|plane| {
                device
                    .claim_plane(plane.handle, headset.crtc)
                    .map(|claim| (plane.handle, claim))
            }) {
                builder.add_plane(cursor, claim);
            }
        }
        Ok(builder)
    }
}

impl DrmBackend {
    /// Returns the GPU `node`'s lease state.
    fn lease_state(&mut self, node: DrmNode) -> Option<&mut DrmLeaseState> {
        self.leases.get_mut(&node).map(|leasing| &mut leasing.state)
    }
}

impl DrmLeaseHandler for Compositor {
    fn drm_lease_state(&mut self, node: DrmNode) -> &mut DrmLeaseState {
        let state = match &mut self.backend {
            Backend::Drm(backend) => backend.lease_state(node),
            Backend::Winit(_) | Backend::Headless(_) => None,
        };
        // Only a GPU with a leasing has a global for clients to speak of,
        // and a leasing is never removed.
        state.expect("a client speaks only of GPUs offering leases")
    }

    fn lease_request(
        &mut self,
        node: DrmNode,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected> {
        let Backend::Drm(backend) = &self.backend else {
            return Err(LeaseRejected::default());
        };
        let (Some(leasing), Some(device)) = (backend.leases.get(&node), backend.devices.get(&node))
        else {
            return Err(LeaseRejected::default());
        };
        leasing.lease_for(device.manager.device(), request)
    }

    fn new_active_lease(&mut self, node: DrmNode, lease: DrmLease) {
        if let Backend::Drm(backend) = &mut self.backend
            && let Some(leasing) = backend.leases.get_mut(&node)
        {
            tracing::info!(gpu = %node, lease = lease.id(), "a headset is leased");
            leasing.active.push(lease);
        }
    }

    fn lease_destroyed(&mut self, node: DrmNode, lease_id: u32) {
        if let Backend::Drm(backend) = &mut self.backend
            && let Some(leasing) = backend.leases.get_mut(&node)
        {
            tracing::info!(gpu = %node, lease = lease_id, "a headset's lease ended");
            leasing.active.retain(|lease| lease.id() != lease_id);
        }
    }
}
