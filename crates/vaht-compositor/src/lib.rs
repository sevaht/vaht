//! The vaht compositor: windows, outputs, input and rendering, built on Smithay.
//!
//! The design is in `docs/design/01-object-model.md`: one `Compositor` state
//! that holds the subsystems (`Outputs`, `Windows`, `Workspaces`, `Focus`,
//! `Input`, `Overlays`, the services) and routes between them, with policy
//! in plain types and the Smithay handler traits as thin gateways.
//!
//! So far it provides the resource foundations every subsystem builds on:
//! counted resources ([`resources`]) and self-removing event-loop
//! registrations ([`event_source`]).

pub mod event_source;
pub mod resources;
