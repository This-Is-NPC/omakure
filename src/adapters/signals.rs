use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub(crate) fn install_signal_handlers(flag: Arc<AtomicBool>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let _ = signal_hook::flag::register(SIGINT, Arc::clone(&flag));
    let _ = signal_hook::flag::register(SIGTERM, flag);
}
