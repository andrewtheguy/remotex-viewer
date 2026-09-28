use std::sync::mpsc;

use anyhow::{Context, Result};
use tao::event_loop::EventLoopProxy;
use tao::window::WindowId;

use crate::UserEvent;

enum Request {
    Read(WindowId, u64),
    Write(WindowId, u64, String),
}

/// The system clipboard, behind the page's `navigator.clipboard`: read without a
/// prompt, written without focus, on a thread of its own so an application holding
/// the clipboard open never stalls a window. One for every gateway window, each
/// request answered to the window it came from.
pub struct Clipboard(mpsc::Sender<Request>);

impl Clipboard {
    pub fn spawn(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let mut board = None;
                for request in rx {
                    let (window, id, result) = match request {
                        Request::Read(window, id) => (window, id, with(&mut board, |b| Ok(b.get_text()?))),
                        Request::Write(window, id, text) => (window, id, with(&mut board, |b| Ok(b.set_text(text).map(|()| String::new())?))),
                    };
                    if proxy.send_event(UserEvent::Clipboard { window, id, result }).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the clipboard thread");
        Self(tx)
    }

    pub fn read(&self, window: WindowId, id: u64) {
        let _ = self.0.send(Request::Read(window, id));
    }

    pub fn write(&self, window: WindowId, id: u64, text: String) {
        let _ = self.0.send(Request::Write(window, id, text));
    }
}

/// Run `f` on the clipboard, opening it on first use and again after a failure, which
/// is how a clipboard another application broke recovers.
fn with(board: &mut Option<arboard::Clipboard>, f: impl FnOnce(&mut arboard::Clipboard) -> Result<String>) -> Result<String> {
    let b = match board {
        Some(b) => b,
        None => board.insert(arboard::Clipboard::new().context("open the clipboard")?),
    };
    let result = f(b);
    if result.is_err() {
        *board = None;
    }
    result
}
