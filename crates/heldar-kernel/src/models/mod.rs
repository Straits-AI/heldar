//! Database row models and API request/response types for the kernel.
//!
//! The models are partitioned into domain submodules for readability and then
//! re-exported flat, so every existing `crate::models::Foo` path keeps
//! resolving unchanged.

mod auth;
mod backup;
mod camera;
mod devices;
mod perception;
mod scheduling;
mod webhooks;

pub use auth::*;
pub use backup::*;
pub use camera::*;
pub use devices::*;
pub use perception::*;
pub use scheduling::*;
pub use webhooks::*;
