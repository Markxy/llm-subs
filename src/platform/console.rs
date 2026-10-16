/// The GUI build has no console on Windows; when started from a terminal for a
/// command-line subcommand, attach to the parent's console so output is visible.
#[cfg(windows)]
pub fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // Safety: plain Win32 call with a constant argument; failure (no parent console) is harmless.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
pub fn attach_parent_console() {}
