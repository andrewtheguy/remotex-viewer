use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use anyhow::{Context, Result};
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::platform::windows::WindowExtWindows;
use tao::window::Window;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use super::KeySink;

// A low-level hook procedure takes no context, so its sink and window are process-wide;
// there is one window and at most one hook.
static SINK: Mutex<Option<KeySink>> = Mutex::new(None);
static WINDOW: AtomicIsize = AtomicIsize::new(0);

pub struct KeyHook(HHOOK);

/// A low-level keyboard hook: it sees a key before the shell does, so the Windows key,
/// Alt+Tab, Alt+F4 and Ctrl+Esc are the page's while it is installed. Only keys typed
/// while this window is in front are taken; injected ones pass, and Ctrl+Alt+Del and
/// Win+L never reach a hook at all. The hook runs on this (the UI) thread, whose
/// message loop is what calls it.
pub fn hook_keys(window: &Window, sink: KeySink) -> Result<KeyHook> {
    WINDOW.store(window.hwnd(), Ordering::Relaxed);
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    // SAFETY: `hook` matches HOOKPROC, and the module is this executable.
    unsafe {
        let module = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(module.into()), 0)
            .context("install the keyboard hook")?;
        Ok(KeyHook(hook))
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        // SAFETY: the hook was installed by `hook_keys` and is removed once.
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
        *SINK.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: for HC_ACTION, lparam points at the event's KBDLLHOOKSTRUCT.
    if code == HC_ACTION as i32 && take(wparam, unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) }) {
        return LRESULT(1);
    }
    // SAFETY: passing the event on, as received.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn take(wparam: WPARAM, key: &KBDLLHOOKSTRUCT) -> bool {
    if key.flags.0 & LLKHF_INJECTED.0 != 0 {
        return false;
    }
    // SAFETY: no arguments; the handle is only compared.
    if unsafe { GetForegroundWindow() }.0 as isize != WINDOW.load(Ordering::Relaxed) {
        return false;
    }
    let pressed = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
    let scancode = if key.flags.0 & LLKHF_EXTENDED.0 != 0 { 0xE000 | key.scanCode } else { key.scanCode };
    SINK.lock().ok().and_then(|sink| sink.as_ref().map(|sink| sink(scancode, pressed))).unwrap_or(false)
}

/// The window's monitor less the taskbar, in physical pixels.
pub fn work_area(window: &Window) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: a live window handle and a sized out-struct.
    let ok = unsafe {
        let monitor = MonitorFromWindow(HWND(window.hwnd() as *mut _), MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info).as_bool()
    };
    let r = info.rcWork;
    ok.then(|| {
        (
            PhysicalPosition::new(r.left, r.top),
            PhysicalSize::new((r.right - r.left) as u32, (r.bottom - r.top) as u32),
        )
    })
}
