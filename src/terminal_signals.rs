//! Terminal cleanup handlers used by the interactive checkpoint picker.

#![allow(unsafe_code)]

/// Installs handlers that restore cursor visibility before termination.
///
/// The handler uses only async-signal-safe operations, restores the default
/// disposition, and re-raises the signal so the process retains its expected
/// termination status.
#[cfg(unix)]
pub(crate) fn install_cursor_restore_signal_handlers() {
    extern "C" fn show_cursor_and_reraise(signal: libc::c_int) {
        const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

        // SAFETY: `SHOW_CURSOR` is valid for the duration of `write`; write,
        // signal, and raise are async-signal-safe, and the handler restores
        // the default disposition before re-raising the received signal.
        unsafe {
            libc::write(
                libc::STDERR_FILENO,
                SHOW_CURSOR.as_ptr().cast(),
                SHOW_CURSOR.len(),
            );
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }

    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let handler = show_cursor_and_reraise as extern "C" fn(libc::c_int) as libc::sighandler_t;

        // SAFETY: `handler` has the C ABI and remains valid for the process
        // lifetime. It only performs the signal-safe operations documented
        // above.
        unsafe {
            libc::signal(libc::SIGTERM, handler);
            libc::signal(libc::SIGHUP, handler);
        }
    });
}
