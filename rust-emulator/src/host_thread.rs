// SPDX-License-Identifier: GPL-3.0-only
// The host thread that runs the guest. On macOS a thread from
// std::thread::spawn starts at the default quality of service, below the
// main thread's user-interactive class; on a busy host the scheduler then
// runs it on efficiency cores, where a guest that keeps up in real time on a
// performance core plays at about half speed.

/// Ask the scheduler to treat the calling thread as user-interactive work,
/// as it does a process's main thread (macOS; elsewhere nothing changes).
/// Returns whether the class was set.
pub fn favour_performance_cores() -> bool {
    #[cfg(target_os = "macos")]
    {
        // <sys/qos.h>: QOS_CLASS_USER_INTERACTIVE.
        const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
        extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
        }
        // SAFETY: changes only the calling thread's scheduling class.
        unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) == 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
