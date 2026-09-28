use std::sync::mpsc;

use anyhow::{Context, Result};
use tao::event_loop::EventLoopProxy;

use crate::UserEvent;

enum Request {
    Read(u64),
    Write(u64, String),
}

/// The system clipboard, behind the page's `navigator.clipboard`: read without a
/// prompt, written without focus, on a thread of its own so an application holding
/// the clipboard open never stalls the window.
pub struct Clipboard(mpsc::Sender<Request>);

impl Clipboard {
    pub fn spawn(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let mut board = None;
                for request in rx {
                    let (id, result) = match request {
                        Request::Read(id) => (id, with(&mut board, |b| Ok(b.get_text()?))),
                        Request::Write(id, text) => (id, with(&mut board, |b| Ok(b.set_text(text).map(|()| String::new())?))),
                    };
                    if proxy.send_event(UserEvent::Clipboard { id, result }).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the clipboard thread");
        Self(tx)
    }

    pub fn read(&self, id: u64) {
        let _ = self.0.send(Request::Read(id));
    }

    pub fn write(&self, id: u64, text: String) {
        let _ = self.0.send(Request::Write(id, text));
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
