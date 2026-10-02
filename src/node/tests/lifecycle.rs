use super::super::lifecycle::is_lifecycle_lock_contention;
use std::io;

#[cfg(windows)]
#[test]
fn windows_lifecycle_lock_sharing_errors_are_contention() {
    for code in [32, 33] {
        assert!(is_lifecycle_lock_contention(&io::Error::from_raw_os_error(
            code
        )));
    }
}
