//! IP networking layer: PPP over CMUX DLC2 → embassy-net stack → TLS.

pub mod buffered;
pub mod ppp;
pub mod stack;
pub mod tls;

pub use stack::start;
