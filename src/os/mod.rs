//! What the shell needs from each OS and tao does not give it.
//!
//! - `hook_keys`: take keys from the OS ahead of its own shortcuts, handing each to
//!   `sink` by platform scancode (see `keys::dom_code`); a key `sink` declines goes on
//!   to the OS. The keys stay taken until the returned hook is dropped. It is called
//!   only where `HOLDS_KEYS` says the OS can.
//! - `work_area`: the part of a screen that windows may occupy.
//! - `claim_instance`: make this the one viewer running, or hand what this launch was
//!   for to the one that is and say to stop.
//! - `library_item`: a way to the library from a gateway window that takes nothing
//!   from its page.

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

/// Given what a later launch was for: its gateway URL, or empty for the library.
pub type LaunchSink = Box<dyn Fn(String) + Send>;

/// Called when a gateway window asks for the library.
pub type LibrarySink = Box<dyn Fn() + Send>;
