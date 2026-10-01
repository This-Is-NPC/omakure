use super::types::Authenticator;
#[cfg(unix)]
use std::sync::Arc;

/// Install a SIGHUP handler that reloads the authenticator (Unix only).
/// Failed reloads keep the last valid set.
#[cfg(unix)]
pub fn install_sighup_reload(auth: Authenticator) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }

    let flag = Arc::new(AtomicBool::new(false));
    let flag_for_hook = Arc::clone(&flag);
    let _ = signal_hook::flag::register(signal_hook::consts::SIGHUP, flag_for_hook);

    thread::spawn(move || {
        loop {
            if flag.swap(false, Ordering::SeqCst) {
                match auth.reload() {
                    Ok(()) => eprintln!("omakure: reloaded tokens file"),
                    Err(err) => {
                        eprintln!(
                            "omakure: tokens reload failed; keeping last valid set ({})",
                            err.status_message()
                        );
                    }
                }
            }
            thread::sleep(Duration::from_millis(200));
        }
    });
}

#[cfg(not(unix))]
pub fn install_sighup_reload(_auth: Authenticator) {}
