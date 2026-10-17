//! System tray icon with a small menu. Where the desktop offers no tray, the app has
//! none either and closing the window quits.

use std::cell::Cell;
use std::sync::mpsc;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::background::Wakeup;
use super::icon::IconImage;

const OPEN_ID: &str = "llm-subs.open";
const PAUSE_WATCHING_ID: &str = "llm-subs.pause";
const PAUSE_TRANSLATIONS_ID: &str = "llm-subs.pause_translations";
const QUIT_ID: &str = "llm-subs.quit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    TogglePauseWatching,
    TogglePauseTranslations,
    Quit,
}

/// What the tray menu and tooltip reflect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrayState {
    pub watching_paused: bool,
    pub translations_paused: bool,
    pub active_jobs: usize,
}

struct PauseItems {
    watching: CheckMenuItem,
    translations: CheckMenuItem,
}

pub struct TrayController {
    receiver: mpsc::Receiver<TrayCommand>,
    tray: TrayIcon,
    pause_items: PauseItems,
    last_state: Cell<TrayState>,
}

impl TrayController {
    /// `wakeup` is woken whenever a command is waiting to be polled.
    pub fn start(wakeup: Wakeup, icon: IconImage) -> Option<Self> {
        let (sender, receiver) = mpsc::channel();
        install_event_handlers(wakeup, sender);
        let (tray, pause_items) = build_tray(icon)?;
        Some(Self { receiver, tray, pause_items, last_state: Cell::new(TrayState::default()) })
    }

    pub fn poll(&self) -> Vec<TrayCommand> {
        self.receiver.try_iter().collect()
    }

    /// Reflects the pause states and active job count in the menu and tooltip.
    pub fn set_state(&self, state: TrayState) {
        if self.last_state.get() == state {
            return;
        }
        self.last_state.set(state);
        self.pause_items.watching.set_checked(state.watching_paused);
        self.pause_items.translations.set_checked(state.translations_paused);
        let _ = self.tray.set_tooltip(Some(tooltip(state)));
    }
}

fn tooltip(state: TrayState) -> String {
    match (state.watching_paused, state.translations_paused, state.active_jobs) {
        (true, true, _) => "LLM Subs - paused".to_string(),
        (false, true, _) => "LLM Subs - translations paused".to_string(),
        (true, false, _) => "LLM Subs - watching paused".to_string(),
        (false, false, 0) => "LLM Subs - idle".to_string(),
        (false, false, n) => format!("LLM Subs - {n} job{} running", if n == 1 { "" } else { "s" }),
    }
}

fn install_event_handlers(wakeup: Wakeup, sender: mpsc::Sender<TrayCommand>) {
    let menu_sender = sender.clone();
    let menu_wakeup = wakeup.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = match event.id().0.as_str() {
            OPEN_ID => TrayCommand::ShowWindow,
            PAUSE_WATCHING_ID => TrayCommand::TogglePauseWatching,
            PAUSE_TRANSLATIONS_ID => TrayCommand::TogglePauseTranslations,
            QUIT_ID => TrayCommand::Quit,
            _ => return,
        };
        let _ = menu_sender.send(command);
        menu_wakeup.wake();
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        let open = match event {
            TrayIconEvent::DoubleClick { .. } => true,
            TrayIconEvent::Click { button: tray_icon::MouseButton::Left, button_state: tray_icon::MouseButtonState::Up, .. } => {
                !cfg!(target_os = "macos")
            }
            _ => false,
        };
        if open {
            let _ = sender.send(TrayCommand::ShowWindow);
            wakeup.wake();
        }
    }));
}

fn build_tray(icon: IconImage) -> Option<(TrayIcon, PauseItems)> {
    let menu = Menu::new();
    let open_item = MenuItem::with_id(OPEN_ID, "Open LLM Subs", true, None);
    let pause_items = PauseItems {
        watching: CheckMenuItem::with_id(PAUSE_WATCHING_ID, "Pause watching", true, false, None),
        translations: CheckMenuItem::with_id(PAUSE_TRANSLATIONS_ID, "Pause translations", true, false, None),
    };
    let quit_item = MenuItem::with_id(QUIT_ID, "Quit", true, None);
    menu.append_items(&[&open_item, &pause_items.watching, &pause_items.translations, &PredefinedMenuItem::separator(), &quit_item]).ok()?;
    let tray_icon = Icon::from_rgba(icon.rgba, icon.width, icon.height).ok()?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(tooltip(TrayState::default()))
        .with_icon(tray_icon)
        .with_menu_on_left_click(cfg!(target_os = "macos"))
        .build()
        .map_err(|error| tracing::warn!(%error, "tray icon unavailable"))
        .ok()?;
    Some((tray, pause_items))
}
