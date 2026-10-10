//! Optional local XSMP server; native desktop managers keep their own server.

#[cfg(feature = "xsmp")]
mod authority;
#[cfg(feature = "xsmp")]
mod callbacks;
#[cfg(feature = "xsmp")]
mod ffi;
#[cfg(feature = "xsmp")]
mod listeners;
#[cfg(feature = "xsmp")]
mod peer;
#[cfg(feature = "xsmp")]
mod properties;
#[cfg(feature = "xsmp")]
mod protocol;
#[cfg(feature = "xsmp")]
mod server;
#[cfg(feature = "xsmp")]
mod transaction;

#[cfg(feature = "xsmp")]
mod handle;
#[cfg(not(feature = "xsmp"))]
mod disabled;

#[cfg(feature = "xsmp")]
pub(super) use handle::{Handle, Message, Notice};
#[cfg(not(feature = "xsmp"))]
pub(super) use disabled::Handle;
