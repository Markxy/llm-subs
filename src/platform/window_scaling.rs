//! A window dragged onto a monitor with different scaling has to take the rectangle
//! Windows suggests. winit picks its own size and position, which the drag loop then
//! undoes, so every crossing grew or shrank the window by the ratio of the scales.
//! While winit handles a scale change, the move it makes is rewritten to the
//! suggested rectangle before Windows applies it.

use winit::window::Window;

#[cfg(windows)]
pub fn follow_suggested_rect_on_scale_change(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    // SAFETY: the handle belongs to a live window; the subclass removes itself on WM_NCDESTROY.
    let installed = unsafe { SetWindowSubclass(handle.hwnd.get() as _, Some(on_window_message), 0, 0) };
    if installed == 0 {
        tracing::warn!("could not hook the window's scale changes");
    }
}

#[cfg(not(windows))]
pub fn follow_suggested_rect_on_scale_change(_window: &Window) {}

#[cfg(windows)]
thread_local! {
    /// The rectangle Windows suggested for the scale change being handled right now.
    static SUGGESTED_RECT: std::cell::Cell<Option<windows_sys::Win32::Foundation::RECT>> = const { std::cell::Cell::new(None) };
}

#[cfg(windows)]
unsafe extern "system" fn on_window_message(
    window: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
    subclass_id: usize,
    _reference_data: usize,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsZoomed, SWP_NOMOVE, SWP_NOSIZE, WINDOWPOS, WM_DPICHANGED, WM_NCDESTROY, WM_WINDOWPOSCHANGING,
    };

    match message {
        WM_DPICHANGED => {
            // SAFETY: for WM_DPICHANGED, lparam points to the suggested window rectangle.
            let suggested = unsafe { *(lparam as *const RECT) };
            let outer_change = SUGGESTED_RECT.replace(Some(suggested));
            // SAFETY: forwarding the message unchanged to winit's handler.
            let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
            SUGGESTED_RECT.set(outer_change);
            result
        }
        WM_WINDOWPOSCHANGING => {
            // SAFETY: for WM_WINDOWPOSCHANGING, lparam points to the pending move, which may be edited.
            if let Some(suggested) = SUGGESTED_RECT.get()
                && unsafe { IsZoomed(window) } == 0
            {
                let pending = unsafe { &mut *(lparam as *mut WINDOWPOS) };
                if pending.flags & SWP_NOMOVE == 0 {
                    (pending.x, pending.y) = (suggested.left, suggested.top);
                }
                if pending.flags & SWP_NOSIZE == 0 {
                    (pending.cx, pending.cy) = (suggested.right - suggested.left, suggested.bottom - suggested.top);
                }
            }
            // SAFETY: forwarding the (possibly edited) message to winit's handler.
            unsafe { DefSubclassProc(window, message, wparam, lparam) }
        }
        WM_NCDESTROY => {
            // SAFETY: removing this same subclass while its window is being destroyed.
            unsafe { RemoveWindowSubclass(window, Some(on_window_message), subclass_id) };
            unsafe { DefSubclassProc(window, message, wparam, lparam) }
        }
        // SAFETY: forwarding every other message unchanged.
        _ => unsafe { DefSubclassProc(window, message, wparam, lparam) },
    }
}
