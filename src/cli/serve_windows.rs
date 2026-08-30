/// Native Windows process and named-event primitives for `serve`.
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER,
    HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, OpenProcess, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

const STOP_EVENT_PREFIX: &str = "Local\\OmakureServeStop-";

pub(crate) fn is_stop_event_name(name: &str) -> bool {
    name.strip_prefix(STOP_EVENT_PREFIX)
        .is_some_and(|suffix| suffix.len() == 32 && suffix.chars().all(|c| c.is_ascii_hexdigit()))
}

pub(crate) struct StopEvent {
    handle: HANDLE,
}

impl StopEvent {
    pub(crate) fn is_signaled(&self) -> Result<bool, String> {
        let result = unsafe { WaitForSingleObject(self.handle, 0) };
        match result {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(last_error("WaitForSingleObject")),
            other => Err(format!(
                "WaitForSingleObject returned unexpected status {other}"
            )),
        }
    }
}

impl Drop for StopEvent {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub(crate) struct ProcessHandle {
    handle: HANDLE,
}

impl ProcessHandle {
    pub(crate) fn wait(&self, timeout: std::time::Duration) -> Result<bool, String> {
        let milliseconds = timeout.as_millis().min(u32::MAX as u128) as u32;
        let result = unsafe { WaitForSingleObject(self.handle, milliseconds) };
        match result {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(last_error("WaitForSingleObject")),
            other => Err(format!(
                "WaitForSingleObject returned unexpected status {other}"
            )),
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
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

    let process = ProcessHandle { handle };
    match process.wait(std::time::Duration::ZERO) {
        Ok(true) => ProcessProbe::Dead,
        Ok(false) => ProcessProbe::Live(process),
        Err(error) => ProcessProbe::Indeterminate(error),
    }
}

pub(crate) fn create_stop_event() -> Result<(String, StopEvent), String> {
    let name = format!("{STOP_EVENT_PREFIX}{:032x}", rand::random::<u128>());
    let wide_name = wide_null(&name);
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide_name.as_ptr()) };
    if handle.is_null() {
        return Err(last_error("CreateEventW"));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        return Err("CreateEventW generated an existing event identity".to_string());
    }
    Ok((name, StopEvent { handle }))
}

pub(crate) enum OpenEventError {
    NotFound,
    Indeterminate(String),
}

pub(crate) fn open_stop_event(name: &str) -> Result<StopEvent, OpenEventError> {
    let wide_name = wide_null(name);
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
        Ok(StopEvent { handle })
    }
}

pub(crate) fn signal_stop(name: &str) -> Result<(), String> {
    let event = open_stop_event(name).map_err(|error| match error {
        OpenEventError::NotFound => "the daemon stop event no longer exists".to_string(),
        OpenEventError::Indeterminate(error) => error,
    })?;
    if unsafe { SetEvent(event.handle) } == 0 {
        return Err(last_error("SetEvent"));
    }
    Ok(())
}

pub(crate) fn publish_exclusive(
    from: &std::path::Path,
    to: &std::path::Path,
) -> Result<(), String> {
    let from = wide_path(from);
    let to = wide_path(to);
    // Omitting MOVEFILE_REPLACE_EXISTING makes a competing starter fail rather
    // than replacing the already-published daemon identity.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(last_error("MoveFileExW"));
    }
    Ok(())
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

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
