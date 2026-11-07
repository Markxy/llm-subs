//! A Windows file dialog that selects files and folders in one go. The system dialog has a
//! mode for files and a mode for folders but none for both: in file mode its button walks
//! into a selected folder. Here that one step is turned into choosing the folder, when the
//! button is what caused it. Double-clicking a folder still opens it.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::core::{implement, w, Error, Interface, Ref, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{E_NOTIMPL, ERROR_CANCELLED, HWND, LPARAM, LRESULT, S_OK, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IServiceProvider, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Ole::IOleWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_SPACE;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DefSubclassProc, FileOpenDialog, IFileDialog, IFileDialogEvents, IFileDialogEvents_Impl, IFileOpenDialog, IFolderView2, IShellBrowser, IShellItem,
    IShellItemArray, RemoveWindowSubclass, SetWindowSubclass, FDE_OVERWRITE_RESPONSE, FDE_SHAREVIOLATION_RESPONSE, FOS_ALLOWMULTISELECT,
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, SID_STopLevelBrowser, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{GetDlgItem, BM_CLICK, IDOK, WM_KEYUP, WM_LBUTTONUP, WM_NCDESTROY};

/// Lets the user choose any mix of files with these extensions and folders. Empty when
/// the dialog was cancelled or could not be shown.
pub fn pick_files_and_folders(extensions: &[String]) -> Vec<PathBuf> {
    // SAFETY: called on the window's thread and balanced below; winit has usually set up COM
    // here already, which makes this a counted no-op.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let picked = show_dialog(extensions);
    if initialized {
        // SAFETY: balances the successful call above, after every COM object has been released.
        unsafe { CoUninitialize() };
    }
    picked.unwrap_or_else(|error| {
        tracing::warn!(%error, "could not show the file dialog");
        Vec::new()
    })
}

fn show_dialog(extensions: &[String]) -> Result<Vec<PathBuf>> {
    let name = HSTRING::from("Media and subtitle files");
    let pattern = HSTRING::from(filter_pattern(extensions));
    let filters = [COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: PCWSTR(pattern.as_ptr()) }];
    let picked_folders = Rc::new(RefCell::new(Vec::new()));

    // SAFETY: plain COM calls on this thread; `name` and `pattern` outlive the dialog.
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_ALLOWMULTISELECT | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
        dialog.SetTitle(w!("Choose files or folders"))?;
        dialog.SetOkButtonLabel(w!("Add"))?;
        dialog.SetFileTypes(&filters)?;

        let events: IFileDialogEvents = ChoosesFolders { picked: picked_folders.clone(), watching_button: Cell::new(false) }.into();
        let cookie = dialog.Advise(&events)?;
        let shown = dialog.Show(None);
        dialog.Unadvise(cookie)?;

        let from_folder_step = std::mem::take(&mut *picked_folders.borrow_mut());
        if !from_folder_step.is_empty() {
            return Ok(from_folder_step);
        }
        match shown {
            Ok(()) => Ok(paths_of(&dialog.GetResults()?)),
            Err(error) if error.code() == ERROR_CANCELLED.to_hresult() => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }
}

/// `["mkv", "srt"]` -> `*.mkv;*.srt`.
fn filter_pattern(extensions: &[String]) -> String {
    extensions.iter().map(|extension| format!("*.{extension}")).collect::<Vec<_>>().join(";")
}

thread_local! {
    /// When the dialog's button was last pressed, not yet matched to a folder step.
    static ADD_BUTTON_PRESSED_AT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Longest time from pressing the button to the dialog acting on it. Long enough for a slow
/// machine, and too short to press the button and then double-click a folder.
const PRESS_TO_FOLDER_STEP: Duration = Duration::from_millis(500);

#[implement(IFileDialogEvents)]
struct ChoosesFolders {
    picked: Rc<RefCell<Vec<PathBuf>>>,
    watching_button: Cell<bool>,
}

impl IFileDialogEvents_Impl for ChoosesFolders_Impl {
    fn OnFolderChanging(&self, dialog: Ref<IFileDialog>, folder: Ref<IShellItem>) -> Result<()> {
        let (Some(dialog), Some(folder)) = (dialog.as_ref(), folder.as_ref()) else { return Ok(()) };
        // The first step, to the folder the dialog opens in, comes when its button exists.
        if !self.watching_button.replace(true) {
            watch_add_button(dialog);
        }
        if !add_button_just_pressed() {
            return Ok(());
        }
        let Some(folder) = path_of(folder) else { return Ok(()) };
        // Places that are not folders on disk, like This PC, stay something to walk into.
        *self.picked.borrow_mut() = chosen_with(selected_paths(dialog), folder);
        // SAFETY: closing from one of its own callbacks is what this method is documented for.
        unsafe { dialog.Close(S_OK)? };
        // Any error stops the dialog from moving into the folder.
        Err(Error::from(ERROR_CANCELLED))
    }

    fn OnFileOk(&self, dialog: Ref<IFileDialog>) -> Result<()> {
        // Files selected together with a folder: the dialog itself would hand over only the files.
        if let Some(dialog) = dialog.as_ref()
            && add_button_just_pressed()
        {
            let selected = selected_paths(dialog);
            if selected.iter().any(|path| path.is_dir()) {
                *self.picked.borrow_mut() = selected;
            }
        }
        Ok(())
    }

    fn OnFolderChange(&self, _: Ref<IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnSelectionChange(&self, _: Ref<IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnShareViolation(&self, _: Ref<IFileDialog>, _: Ref<IShellItem>) -> Result<FDE_SHAREVIOLATION_RESPONSE> {
        Err(E_NOTIMPL.into())
    }

    fn OnTypeChange(&self, _: Ref<IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnOverwrite(&self, _: Ref<IFileDialog>, _: Ref<IShellItem>) -> Result<FDE_OVERWRITE_RESPONSE> {
        Err(E_NOTIMPL.into())
    }
}

/// What the Add button chose while the dialog was about to enter `target`: everything
/// selected, or just `target` when it came from elsewhere, like a path typed into the name box.
fn chosen_with(selected: Vec<PathBuf>, target: PathBuf) -> Vec<PathBuf> {
    if selected.contains(&target) { selected } else { vec![target] }
}

/// Hooks the button so it notes when it is pressed. The dialog says nothing about what made
/// it change folder, and it has moved the focus away from the button by then.
fn watch_add_button(dialog: &IFileDialog) {
    // SAFETY: the handles belong to this open dialog; the hook removes itself on WM_NCDESTROY.
    unsafe {
        let Ok(window) = dialog.cast::<IOleWindow>().and_then(|window| window.GetWindow()) else { return };
        let Ok(button) = GetDlgItem(Some(window), IDOK.0) else { return };
        if !SetWindowSubclass(button, Some(on_add_button_message), 0, 0).as_bool() {
            tracing::warn!("could not watch the file dialog's button");
        }
    }
}

fn add_button_just_pressed() -> bool {
    ADD_BUTTON_PRESSED_AT.take().is_some_and(|pressed_at| pressed_at.elapsed() < PRESS_TO_FOLDER_STEP)
}

unsafe extern "system" fn on_add_button_message(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, subclass_id: usize, _reference_data: usize) -> LRESULT {
    let pressed = match message {
        WM_LBUTTONUP | BM_CLICK => true,
        WM_KEYUP => wparam.0 == VK_SPACE.0 as usize,
        _ => false,
    };
    if pressed {
        ADD_BUTTON_PRESSED_AT.set(Some(Instant::now()));
    }
    // SAFETY: forwards the message unchanged, and removes this same hook as its window goes.
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(window, Some(on_add_button_message), subclass_id);
        }
        DefSubclassProc(window, message, wparam, lparam)
    }
}

/// What is selected in the dialog's file list. The dialog's own `GetSelectedItems` leaves out
/// everything but one item as soon as folders are among the selection.
fn selected_paths(dialog: &IFileDialog) -> Vec<PathBuf> {
    // SAFETY: these calls only read the current selection of the open dialog's view.
    let selection = unsafe {
        dialog
            .cast::<IServiceProvider>()
            .and_then(|provider| provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser))
            .and_then(|browser| browser.QueryActiveShellView())
            .and_then(|view| view.cast::<IFolderView2>())
            .and_then(|view| view.GetSelection(false))
    };
    selection.map(|items| paths_of(&items)).unwrap_or_default()
}

fn paths_of(items: &IShellItemArray) -> Vec<PathBuf> {
    // SAFETY: the indexes stay inside the count the array reports.
    unsafe {
        let count = items.GetCount().unwrap_or(0);
        (0..count).filter_map(|index| items.GetItemAt(index).ok()).filter_map(|item| path_of(&item)).collect()
    }
}

fn path_of(item: &IShellItem) -> Option<PathBuf> {
    // SAFETY: the returned string is valid until the free right after it is copied.
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.as_ptr().cast()));
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_filter_pattern() {
        assert_eq!(filter_pattern(&["mkv".to_string(), "srt".to_string()]), "*.mkv;*.srt");
    }

    #[test]
    fn keeps_the_selection_only_when_it_holds_the_target() {
        let (a, b, typed) = (PathBuf::from("C:/a"), PathBuf::from("C:/b"), PathBuf::from("C:/typed"));
        assert_eq!(chosen_with(vec![a.clone(), b.clone()], b.clone()), vec![a.clone(), b.clone()]);
        assert_eq!(chosen_with(vec![a, b], typed.clone()), vec![typed]);
    }
}
