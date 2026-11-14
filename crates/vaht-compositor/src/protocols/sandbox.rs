//! Sandboxed programs (`wp_security_context_v1`): a sandbox engine, such as
//! Flatpak, asks for a socket of its own for the programs it runs, and
//! vaht accepts them there as restricted clients
//! ([`crate::client::ClientState::is_unrestricted`]).
//!
//! The socket's listener ends when the sandbox engine closes its end; vaht
//! then forgets it.

use smithay::wayland::security_context::{
    SecurityContext, SecurityContextHandler, SecurityContextListenerSource,
};

use crate::Compositor;
use crate::client::ClientState;
use crate::event_source::EndingSource;

impl SecurityContextHandler for Compositor {
    fn context_created(&mut self, source: SecurityContextListenerSource, context: SecurityContext) {
        self.sandbox_listeners
            .retain(|listener| !listener.has_ended());
        tracing::info!(
            engine = context.sandbox_engine.as_deref().unwrap_or("unknown"),
            app_id = context.app_id.as_deref().unwrap_or("unknown"),
            "a sandbox's programs may connect"
        );
        let listener = EndingSource::insert(
            &self.loop_handle,
            &self.ledger,
            source,
            move |stream, (), compositor: &mut Self| {
                let state = ClientState::sandboxed(&compositor.ledger, context.clone());
                if let Err(error) = compositor
                    .display_handle
                    .clone()
                    .insert_client(stream, std::sync::Arc::new(state))
                {
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        "could not accept a sandboxed client"
                    );
                }
            },
        );
        match listener {
            Ok(listener) => self.sandbox_listeners.push(listener),
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not listen for a sandbox's programs"
            ),
        }
    }
}
