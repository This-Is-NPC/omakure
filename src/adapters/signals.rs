use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub(crate) fn install_signal_handlers(flag: Arc<AtomicBool>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let _ = signal_hook::flag::register(SIGINT, Arc::clone(&flag));
    let _ = signal_hook::flag::register(SIGTERM, flag);
}
