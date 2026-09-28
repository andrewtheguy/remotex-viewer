use anyhow::Result;
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::window::Window;

use super::KeySink;

pub struct KeyHook;

/// Not yet: on macOS this is a `CGEventTap` handing the sink each key's virtual key
/// code, which `keys::dom_code` already reads.
pub fn hook_keys(_window: &Window, _sink: KeySink) -> Result<KeyHook> {
    anyhow::bail!("the macOS viewer does not take the keyboard yet")
}

/// Not yet (`NSScreen.visibleFrame`); the monitor's whole size stands in.
pub fn work_area(_window: &Window) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    None
}
