//! The LAN tunnel: rust-dos instances join rooms of a relay over UDP, and
//! each room passes Ethernet frames between its members as a switch does.
//! A relay runs inside any instance (`LAN HOST`) or on its own
//! (`rust-dos-relay`), on the LAN or on a server on the internet.
//!
//! The protocol is rust-dos's own (`wire`); it is not DOSBox's IPXNET.
//! Joining a room with a password proves knowledge of it without sending
//! it (`auth`), but the frames themselves are not encrypted.

pub mod auth;
pub mod client;
pub mod frag;
pub mod relay;
pub mod wire;
