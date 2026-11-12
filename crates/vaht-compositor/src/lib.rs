//! The vaht compositor: windows, outputs, input and rendering, built on Smithay.
//!
//! The design is in `docs/design/01-object-model.md`: one `Compositor` state
//! that holds the subsystems (`Outputs`, `Windows`, `Workspaces`, `Focus`,
//! `Input`, `Overlays`, the services) and routes between them, with policy
//! in plain types and the Smithay handler traits as thin gateways.
//!
//! Nothing is implemented yet; this crate exists so the workspace has its
//! final shape from the start.
