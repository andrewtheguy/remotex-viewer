use anyhow::Result;
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::monitor::MonitorHandle;
use tao::window::Window;

use super::{KeySink, LaunchSink, LibrarySink};

pub struct KeyHook;

/// Not yet, so the keyboard is never asked for and the page keeps its own keys.
pub const HOLDS_KEYS: bool = false;

/// Not yet: on macOS this is a `CGEventTap` handing the sink each key's virtual key
/// code, which `keys::dom_code` already reads.
pub fn hook_keys(_window: &Window, _sink: KeySink) -> Result<KeyHook> {
    anyhow::bail!("the macOS viewer does not take the keyboard yet")
}

/// Not yet (`NSScreen.visibleFrame`); the monitor's whole size stands in.
pub fn work_area(monitor: &MonitorHandle) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    (monitor.position(), monitor.size())
}

pub struct Instance;

/// Launch Services already runs one copy of an app bundle; a later launch reaches it as
/// a reopen, which the viewer does not take yet.
pub fn claim_instance(_launch: &str) -> Result<Option<Instance>> {
    Ok(Some(Instance))
}

impl Instance {
    pub fn listen(&self, _sink: LaunchSink) -> Result<()> {
        Ok(())
    }
}

/// Not yet: on macOS this is a **Library** item in the app's menu bar.
pub fn library_item(_window: &Window, _sink: LibrarySink) -> Result<()> {
    Ok(())
}
