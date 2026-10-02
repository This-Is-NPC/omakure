#[cfg(unix)]
pub(crate) mod unix;
#[cfg(windows)]
pub(crate) mod windows;

#[cfg(unix)]
pub(crate) use unix::open_existing_file_read;
#[cfg(windows)]
pub(crate) use windows::open_existing_file_read;
