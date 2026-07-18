//! App events delivered to the main loop via channel, herdr-style:
//! background work (terminal input, view prefetching) sends events
//! instead of the main loop polling.

use std::sync::mpsc::Sender;
use std::thread;

use crate::processor::view::FileView;

pub enum AppEvent {
    Input(crossterm::event::Event),
    /// A background-computed view; stale generations are discarded.
    ViewReady {
        generation: u64,
        index: usize,
        view: FileView,
    },
}

pub fn spawn_input_thread(tx: Sender<AppEvent>) {
    thread::spawn(move || {
        while let Ok(event) = crossterm::event::read() {
            if tx.send(AppEvent::Input(event)).is_err() {
                break;
            }
        }
    });
}
