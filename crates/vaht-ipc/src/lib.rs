//! The vaht IPC protocol: the messages exchanged over vaht's Unix socket.
//!
//! One JSON message per line, in both directions. The request, reply and
//! event types defined here are the single source of the protocol: `vaht msg`
//! uses them, and their JSON Schema is published for clients in other
//! languages. See `docs/design/00-feature-inventory.md` §11.
//!
//! Nothing is implemented yet.
