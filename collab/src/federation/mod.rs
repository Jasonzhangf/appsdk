pub mod auth;
pub mod config;
pub mod management;
pub mod server;
pub mod transport;

pub use config::{FederationConfig, FederationPeer};
pub use transport::FederationClient;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
