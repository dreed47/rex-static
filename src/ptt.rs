//! Push-to-talk from outside the window. A compositor key binding sends
//! SIGUSR1 on press and SIGUSR2 on release (`pkill -USR1 -x rex-static`),
//! so the key works whichever window has focus.

use std::sync::atomic::{AtomicBool, Ordering};

const SIGUSR1: i32 = 10;
const SIGUSR2: i32 = 12;
const SIG_IGN: usize = 1;

static DOWN: AtomicBool = AtomicBool::new(false);
static UP: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
}

extern "C" fn on_signal(sig: i32) {
    // Only an atomic store: safe inside a signal handler.
    match sig {
        SIGUSR1 => DOWN.store(true, Ordering::SeqCst),
        SIGUSR2 => UP.store(true, Ordering::SeqCst),
        _ => {}
    }
}

/// Catch both signals. Call first thing in `main`: by default they end the program.
pub fn install() {
    let h = on_signal as extern "C" fn(i32) as usize;
    unsafe {
        signal(SIGUSR1, h);
        signal(SIGUSR2, h);
    }
}

/// Ignore both signals across an exec, so a key press during a restart
/// can't end the new copy before it calls `install`. Caught signals reset
/// to their default on exec; ignored ones stay ignored.
pub fn ignore() {
    unsafe {
        signal(SIGUSR1, SIG_IGN);
        signal(SIGUSR2, SIG_IGN);
    }
}

/// The key went down since the last call.
pub fn pressed() -> bool {
    DOWN.swap(false, Ordering::SeqCst)
}

/// The key came up since the last call.
pub fn released() -> bool {
    UP.swap(false, Ordering::SeqCst)
}
