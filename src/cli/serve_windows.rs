/// Native Windows process and named-event primitives for `serve`.
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, HANDLE,
    WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, OpenProcess, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

use crate::util::windows::{wide_path, wide_str};

const STOP_EVENT_PREFIX: &str = "Local\\OmakureServeStop-";

pub(crate) fn is_stop_event_name(name: &str) -> bool {
    name.strip_prefix(STOP_EVENT_PREFIX)
        .is_some_and(|suffix| suffix.len() == 32 && suffix.chars().all(|c| c.is_ascii_hexdigit()))
}

pub(crate) struct StopEvent {
    handle: OwnedHandle,
}

impl StopEvent {
    pub(crate) fn is_signaled(&self) -> Result<bool, String> {
        wait_for(&self.handle, 0)
    }
}

pub(crate) struct ProcessHandle {
    handle: OwnedHandle,
}

impl ProcessHandle {
    pub(crate) fn wait(&self, timeout: std::time::Duration) -> Result<bool, String> {
        wait_for(
            &self.handle,
            timeout.as_millis().min(u32::MAX as u128) as u32,
        )
    }
}

/// Take ownership of a handle a Win32 call just returned, so dropping it
/// closes it.
fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: callers pass a non-null handle freshly returned by a Win32
    // open/create call and never close or share it elsewhere.
    unsafe { OwnedHandle::from_raw_handle(handle) }
}

fn wait_for(handle: &OwnedHandle, milliseconds: u32) -> Result<bool, String> {
    let result = unsafe { WaitForSingleObject(handle.as_raw_handle(), milliseconds) };
    match result {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        WAIT_FAILED => Err(last_error("WaitForSingleObject")),
        other => Err(format!(
            "WaitForSingleObject returned unexpected status {other}"
        )),
    }
}

pub(crate) enum ProcessProbe {
    Live(ProcessHandle),
    Dead,
    Indeterminate(String),
}

pub(crate) fn probe_process(pid: u32) -> ProcessProbe {
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        return if error == ERROR_INVALID_PARAMETER {
            ProcessProbe::Dead
        } else {
            ProcessProbe::Indeterminate(format!(
                "OpenProcess({pid}) failed with Windows error {error}"
            ))
        };
    }

    let process = ProcessHandle {
        handle: owned(handle),
    };
    match process.wait(std::time::Duration::ZERO) {
        Ok(true) => ProcessProbe::Dead,
        Ok(false) => ProcessProbe::Live(process),
        Err(error) => ProcessProbe::Indeterminate(error),
    }
}

pub(crate) fn create_stop_event() -> Result<(String, StopEvent), String> {
    let name = format!("{STOP_EVENT_PREFIX}{:032x}", rand::random::<u128>());
    let wide_name = wide_str(&name);
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide_name.as_ptr()) };
    if handle.is_null() {
        return Err(last_error("CreateEventW"));
    }
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let event = StopEvent {
        handle: owned(handle),
    };
    if already_exists {
        return Err("CreateEventW generated an existing event identity".to_string());
    }
    Ok((name, event))
}

pub(crate) enum OpenEventError {
    NotFound,
    Indeterminate(String),
}

pub(crate) fn open_stop_event(name: &str) -> Result<StopEvent, OpenEventError> {
    let wide_name = wide_str(name);
    let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide_name.as_ptr()) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        if error == ERROR_FILE_NOT_FOUND {
            Err(OpenEventError::NotFound)
        } else {
            Err(OpenEventError::Indeterminate(format!(
                "OpenEventW failed with Windows error {error}"
            )))
        }
    } else {
        Ok(StopEvent {
            handle: owned(handle),
        })
    }
}

pub(crate) fn signal_stop(name: &str) -> Result<(), String> {
    let event = open_stop_event(name).map_err(|error| match error {
        OpenEventError::NotFound => "the daemon stop event no longer exists".to_string(),
        OpenEventError::Indeterminate(error) => error,
    })?;
    if unsafe { SetEvent(event.handle.as_raw_handle()) } == 0 {
        return Err(last_error("SetEvent"));
    }
    Ok(())
}

pub(crate) fn publish_exclusive(
    from: &std::path::Path,
    to: &std::path::Path,
) -> Result<(), PublishExclusiveError> {
    let from = wide_path(from);
    let to = wide_path(to);
    // Omitting MOVEFILE_REPLACE_EXISTING makes a competing starter fail rather
    // than replacing the already-published daemon identity.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(PublishExclusiveError(unsafe { GetLastError() }));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("MoveFileExW failed with Windows error {0}")]
pub(crate) struct PublishExclusiveError(u32);

fn last_error(operation: &str) -> String {
    let error = unsafe { GetLastError() };
    format!("{operation} failed with Windows error {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_live() {
        assert!(matches!(
            probe_process(std::process::id()),
            ProcessProbe::Live(_)
        ));
    }

    #[test]
    fn invalid_process_id_is_dead() {
        assert!(matches!(probe_process(u32::MAX), ProcessProbe::Dead));
    }

    #[test]
    fn named_event_round_trip_is_native_and_manual_reset() {
        let (name, event) = create_stop_event().expect("create event");
        assert!(!event.is_signaled().expect("initial event state"));
        signal_stop(&name).expect("signal event");
        assert!(event.is_signaled().expect("signaled event state"));
        assert!(event.is_signaled().expect("manual-reset event state"));
    }
}
