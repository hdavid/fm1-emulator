// SPDX-License-Identifier: GPL-3.0-only
// fm1-ui is built for the Windows GUI subsystem (see the attribute in ui.rs) so
// that double-clicking it opens only the window, with no console behind it.
// A GUI-subsystem program has no console, so when it is started from a
// terminal its messages and stdin would be lost. `attach_parent` joins the
// parent's console, if there is one, and points the standard handles at it, so
// `emulator.exe firmware.fwsc` prints and reads as a console program does.
// Double-clicked there is no parent console and nothing changes.

/// Join the parent process's console and use it for stdout, stderr and stdin.
#[cfg(windows)]
pub(super) fn attach_parent() {
    use std::{fs::OpenOptions, os::windows::io::IntoRawHandle};
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    // SAFETY: plain Win32 calls with valid arguments, made once at startup
    // before any other thread exists; a failure only leaves things as they were.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        // Keep a handle the process was given (a redirect or a pipe); replace
        // only the missing ones with the console.
        let missing = |id| {
            let handle = GetStdHandle(id);
            handle.is_null() || handle as isize == -1
        };
        for (id, device, write) in [
            (STD_OUTPUT_HANDLE, "CONOUT$", true),
            (STD_ERROR_HANDLE, "CONOUT$", true),
            (STD_INPUT_HANDLE, "CONIN$", false),
        ] {
            if !missing(id) {
                continue;
            }
            let file = OpenOptions::new().read(!write).write(write).open(device);
            if let Ok(file) = file {
                SetStdHandle(id, file.into_raw_handle() as _);
            }
        }
    }
}

#[cfg(not(windows))]
pub(super) fn attach_parent() {}
