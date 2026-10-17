//! The part of the desktop app that keeps going whether or not the window exists: the
//! tray icon and its menu, notifications about jobs, and later starts of the app asking
//! for the window.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use eframe::egui;
use parking_lot::Mutex;

use crate::app::AppRuntime;
use crate::jobs::JobRecord;
use crate::platform::single_instance::ActivationRequests;

use super::icon;
use super::notifications::JobAnnouncer;
use super::tray::{TrayCommand, TrayController, TrayState};

/// What the background asks of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    ShowWindow,
    Quit,
}

pub struct Background {
    runtime: Arc<AppRuntime>,
    tray: Option<TrayController>,
    announcer: JobAnnouncer,
    activations: ActivationRequests,
    jobs_changed: Arc<AtomicBool>,
    wakeup: Wakeup,
    wakeups: mpsc::Receiver<()>,
}

impl Background {
    pub fn start(runtime: Arc<AppRuntime>, activations: ActivationRequests) -> Self {
        let (sender, wakeups) = mpsc::channel();
        let wakeup = Wakeup(Arc::new(Mutex::new(WakeTarget::Waiting(sender))));
        let jobs_changed = Arc::new(AtomicBool::new(true));
        runtime.set_ui_notifier(Arc::new({
            let jobs_changed = jobs_changed.clone();
            let wakeup = wakeup.clone();
            move || {
                jobs_changed.store(true, Ordering::Relaxed);
                wakeup.wake();
            }
        }));
        activations.set_waker({
            let wakeup = wakeup.clone();
            move || wakeup.wake()
        });
        let tray = TrayController::start(wakeup.clone(), icon::render(32));
        Self { runtime, tray, announcer: JobAnnouncer::default(), activations, jobs_changed, wakeup, wakeups }
    }

    pub fn has_tray(&self) -> bool {
        self.tray.is_some()
    }

    /// Runs the background on the calling thread until the window is asked for or the
    /// app is quit from the tray.
    pub fn wait_for_window(&mut self) -> Request {
        let mut jobs = Vec::new();
        loop {
            if let Some(request) = self.update(&mut jobs) {
                return request;
            }
            if self.wakeups.recv().is_err() {
                return Request::ShowWindow;
            }
        }
    }

    /// From now on the window is woken up to handle what happens.
    pub fn attach_window(&self, ctx: &egui::Context) {
        *self.wakeup.0.lock() = WakeTarget::Window(ctx.clone());
        self.jobs_changed.store(true, Ordering::Relaxed);
    }

    /// Catches up on what happened since the last call: refreshes `jobs` when they
    /// changed, applies the tray menu's pause toggles and returns what the window should do.
    pub fn update(&mut self, jobs: &mut Vec<JobRecord>) -> Option<Request> {
        if self.jobs_changed.swap(false, Ordering::Relaxed) {
            *jobs = self.runtime.jobs().snapshot();
            self.announcer.update(jobs);
        }
        let mut request = self.activations.take().then_some(Request::ShowWindow);
        let Some(tray) = &self.tray else { return request };
        for command in tray.poll() {
            match command {
                TrayCommand::ShowWindow => request = request.or(Some(Request::ShowWindow)),
                TrayCommand::Quit => request = Some(Request::Quit),
                TrayCommand::TogglePauseWatching => self.runtime.set_watch_paused(!self.runtime.is_watch_paused()),
                TrayCommand::TogglePauseTranslations => {
                    let queue = self.runtime.jobs();
                    queue.set_paused(!queue.is_paused());
                }
            }
        }
        tray.set_state(TrayState {
            watching_paused: self.runtime.is_watch_paused(),
            translations_paused: self.runtime.jobs().is_paused(),
            active_jobs: jobs.iter().filter(|job| job.status.is_active()).count(),
        });
        request
    }
}

/// Wakes whatever drives the app: the wait before the window opens, then the window.
#[derive(Clone)]
pub struct Wakeup(Arc<Mutex<WakeTarget>>);

enum WakeTarget {
    Waiting(mpsc::Sender<()>),
    Window(egui::Context),
}

impl Wakeup {
    pub fn wake(&self) {
        match &*self.0.lock() {
            WakeTarget::Waiting(sender) => {
                let _ = sender.send(());
            }
            WakeTarget::Window(ctx) => ctx.request_repaint(),
        }
    }
}
