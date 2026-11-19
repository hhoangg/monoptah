//! Mobile access: a paired phone browses and drives the desktop workspace over
//! the QUIC link in `monocode-mobile-link`. The host here is app-agnostic; the
//! app answers [`host::Event`]s from its own state.
pub mod audit;
pub mod catalog;
pub mod cloud_policy;
pub mod denylist;
pub mod folders;
pub mod host;
pub(crate) mod identity_store;
pub mod mentioned_file;
pub mod notify;
pub mod policy;
mod router;
pub mod watch;
pub mod web_link;

pub use host::{
    Device, Event, Invite, MobileHost, Push, PushRegistration, Request, Status, WatchStart,
};
pub use monocode_mobile_link::Address;
pub use policy::{DeviceContext, DeviceKind, Permission};
pub use web_link::{WebState, WebStatus};
