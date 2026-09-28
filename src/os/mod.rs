//! What the shell needs from each OS and tao does not give it.
//!
//! - `hook_keys`: take keys from the OS ahead of its own shortcuts, handing each to
//!   `sink` by platform scancode (see `keys::dom_code`); a key `sink` declines goes on
//!   to the OS. The keys stay taken until the returned hook is dropped.
//! - `work_area`: the part of the window's screen that windows may occupy.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

#[cfg(target_os = "macos")]
pub use mac::*;
#[cfg(windows)]
pub use win::*;

/// Given a key's platform scancode and whether it went down; answers whether it took it.
pub type KeySink = Box<dyn Fn(u32, bool) -> bool + Send>;
