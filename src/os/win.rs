use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::monitor::MonitorHandle;
use tao::platform::windows::{MonitorHandleExtWindows, WindowExtWindows};
use tao::window::Window;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CallNextHookEx, CreateWindowExW, DefWindowProcW, FindWindowExW, GetForegroundWindow,
    GetWindowThreadProcessId, HC_ACTION, HHOOK, HWND_MESSAGE, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED,
    RegisterClassW, SendMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_COPYDATA, WM_KEYDOWN, WM_SYSKEYDOWN, WNDCLASSW,
};
use windows_core::{PCWSTR, w};

use super::{KeySink, LaunchSink};

// A low-level hook procedure takes no context, so its sink and window are process-wide;
// only the gateway window holding the keyboard has a hook, so there is at most one.
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

/// The monitor less the taskbar, in physical pixels; the whole monitor when Windows
/// will not say.
pub fn work_area(monitor: &MonitorHandle) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: a monitor handle Windows gave tao, and a sized out-struct.
    let ok = unsafe { GetMonitorInfoW(HMONITOR(monitor.hmonitor() as *mut _), &mut info).as_bool() };
    if !ok {
        return (monitor.position(), monitor.size());
    }
    let r = info.rcWork;
    (PhysicalPosition::new(r.left, r.top), PhysicalSize::new((r.right - r.left) as u32, (r.bottom - r.top) as u32))
}

// One viewer to a desktop session: the first launch holds a named mutex and a
// message-only window, and a later one finds the window and sends it what it was for.
const INSTANCE_MUTEX: PCWSTR = w!(r"Local\remotex-viewer");
const INSTANCE_CLASS: PCWSTR = w!("remotex-viewer.instance");
/// Marks a `WM_COPYDATA` as a launch's, "remx".
const LAUNCH_DATA: usize = 0x7265_6d78;

static LAUNCH: Mutex<Option<LaunchSink>> = Mutex::new(None);

pub struct Instance(HANDLE);

/// This launch as the one viewer, or — when another is already running — `None`, once
/// `launch` has been handed to it. The other is let bring its window forward, which
/// Windows allows only to what the user just started, and that is this.
pub fn claim_instance(launch: &str) -> Result<Option<Instance>> {
    // SAFETY: a constant name; the handle is kept, or closed below.
    let mutex = unsafe { CreateMutexW(None, false, INSTANCE_MUTEX) }.context("create the viewer's mutex")?;
    // SAFETY: no arguments; read straight after the call it describes.
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return Ok(Some(Instance(mutex)));
    }
    // SAFETY: a handle made above and closed once.
    unsafe {
        let _ = CloseHandle(mutex);
    }
    // The first may be between its mutex and its window.
    let deadline = Instant::now() + Duration::from_secs(5);
    let window = loop {
        // SAFETY: a constant class name, looked for among message-only windows.
        match unsafe { FindWindowExW(Some(HWND_MESSAGE), None, INSTANCE_CLASS, PCWSTR::null()) } {
            Ok(window) => break window,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e).context("the running viewer does not answer"),
        }
    };
    let data = COPYDATASTRUCT { dwData: LAUNCH_DATA, cbData: launch.len() as u32, lpData: launch.as_ptr() as *mut _ };
    // SAFETY: a window found above, and data that outlives the synchronous send.
    unsafe {
        let mut process = 0;
        GetWindowThreadProcessId(window, Some(&mut process));
        let _ = AllowSetForegroundWindow(process);
        SendMessageW(window, WM_COPYDATA, None, Some(LPARAM(&data as *const _ as isize)));
    }
    Ok(None)
}

impl Instance {
    /// Hand each later launch to `sink`, from this (the UI) thread's message loop.
    pub fn listen(&self, sink: LaunchSink) -> Result<()> {
        *LAUNCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
        // SAFETY: a class of this module's with `receive` as its procedure, and a
        // message-only window of it that lives as long as the thread.
        unsafe {
            let module = GetModuleHandleW(None).context("GetModuleHandleW")?;
            let class = WNDCLASSW { lpfnWndProc: Some(receive), hInstance: module.into(), lpszClassName: INSTANCE_CLASS, ..Default::default() };
            anyhow::ensure!(RegisterClassW(&class) != 0, "register the viewer's window class");
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                INSTANCE_CLASS,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(module.into()),
                None,
            )
            .context("create the viewer's message window")?;
        }
        Ok(())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the mutex made by `claim_instance`, closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

unsafe extern "system" fn receive(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_COPYDATA {
        // SAFETY: for WM_COPYDATA, lparam points at the sender's COPYDATASTRUCT, valid
        // for the length of the call.
        let data = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
        if data.dwData == LAUNCH_DATA {
            // SAFETY: cbData bytes at lpData, as above.
            let bytes = unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize) };
            if let Some(sink) = LAUNCH.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                sink(String::from_utf8_lossy(bytes).into_owned());
            }
            return LRESULT(1);
        }
    }
    // SAFETY: everything else, as received.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
