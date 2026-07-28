//! Test doubles standing in for the clients at the relay's data plane
//! boundaries. Only these boundaries are faked — each substitutes a trait the
//! production code already uses polymorphically — so the relay components
//! under test stay real.

pub(crate) mod downstream_client;
pub(crate) mod upstream_client;
