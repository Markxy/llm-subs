//! File and folder dialogs that leave the window running while they are open. Waiting
//! for a dialog on the UI thread keeps the window from answering the desktop, which then
//! reports it as not responding.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;
use futures::future::{BoxFuture, FutureExt};
use rfd::AsyncFileDialog;

use crate::config::WatchConfig;

use super::LlmSubsApp;

type OnChoice = Box<dyn FnOnce(&mut LlmSubsApp, Vec<PathBuf>)>;

enum Selection {
    Folder,
    Files,
}

#[derive(Default)]
enum State {
    #[default]
    Closed,
    /// Asked for while drawing; opened once the window it belongs to is at hand.
    Requested { dialog: AsyncFileDialog, selection: Selection, on_choice: OnChoice },
    Open { choice: mpsc::Receiver<Vec<PathBuf>>, on_choice: OnChoice },
}

/// The window's file dialog. Only one is open at a time, as it is modal to the window.
#[derive(Default)]
pub struct FileDialogs {
    state: State,
}

impl FileDialogs {
    pub fn pick_folder(&mut self, on_choice: impl FnOnce(&mut LlmSubsApp, PathBuf) + 'static) {
        let dialog = AsyncFileDialog::new().set_title("Choose a folder");
        self.request(
            dialog,
            Selection::Folder,
            Box::new(move |app, folders| {
                if let Some(folder) = folders.into_iter().next() {
                    on_choice(app, folder);
                }
            }),
        );
    }

    pub fn pick_media_files(&mut self, watch: &WatchConfig, on_choice: impl FnOnce(&mut LlmSubsApp, Vec<PathBuf>) + 'static) {
        let dialog = AsyncFileDialog::new()
            .set_title("Choose media or subtitle files")
            .add_filter("Media files", &watch.media_extensions)
            .add_filter("Subtitle files", &watch.subtitle_extensions);
        self.request(dialog, Selection::Files, Box::new(on_choice));
    }

    fn request(&mut self, dialog: AsyncFileDialog, selection: Selection, on_choice: OnChoice) {
        if matches!(self.state, State::Closed) {
            self.state = State::Requested { dialog, selection, on_choice };
        }
    }

    /// Opens the requested dialog, if any, on top of `parent`.
    pub fn open_requested(&mut self, parent: &eframe::Frame, ctx: &egui::Context, runtime: &tokio::runtime::Handle) {
        self.state = match std::mem::take(&mut self.state) {
            State::Requested { dialog, selection, on_choice } => {
                let choice = show(dialog.set_parent(parent), selection, ctx.clone(), runtime);
                State::Open { choice, on_choice }
            }
            state => state,
        };
    }

    /// What was chosen in a dialog that has closed, and what to do with it. Nothing when
    /// the dialog was cancelled.
    pub fn take_choice(&mut self) -> Option<(Vec<PathBuf>, OnChoice)> {
        match std::mem::take(&mut self.state) {
            State::Open { choice, on_choice } => match choice.try_recv() {
                Ok(paths) => (!paths.is_empty()).then_some((paths, on_choice)),
                Err(mpsc::TryRecvError::Disconnected) => None,
                Err(mpsc::TryRecvError::Empty) => {
                    self.state = State::Open { choice, on_choice };
                    None
                }
            },
            state => {
                self.state = state;
                None
            }
        }
    }
}

fn show(dialog: AsyncFileDialog, selection: Selection, ctx: egui::Context, runtime: &tokio::runtime::Handle) -> mpsc::Receiver<Vec<PathBuf>> {
    // The dialog starts here, on the UI thread as macOS requires; only the wait moves off it.
    let chosen: BoxFuture<'static, Vec<PathBuf>> = match selection {
        Selection::Folder => dialog.pick_folder().map(|folder| folder.into_iter().map(PathBuf::from).collect()).boxed(),
        Selection::Files => dialog.pick_files().map(|files| files.unwrap_or_default().into_iter().map(PathBuf::from).collect()).boxed(),
    };
    let (sender, receiver) = mpsc::channel();
    runtime.spawn(async move {
        let _ = sender.send(chosen.await);
        ctx.request_repaint();
    });
    receiver
}
