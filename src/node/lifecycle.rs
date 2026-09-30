use super::context::NodeContext;
use super::layout::LIFECYCLE_LOCK_FILE;
use super::NodeError;
use fs2::FileExt;
use std::fs;
use std::io;
use std::path::Path;

impl NodeContext {
    pub(crate) fn acquire_lifecycle_lock(&self) -> Result<NodeLifecycleLock, NodeError> {
        NodeLifecycleLock::acquire(self, false)
    }

    pub(crate) fn try_acquire_lifecycle_lock(&self) -> Result<NodeLifecycleLock, NodeError> {
        NodeLifecycleLock::acquire(self, true)
    }
}

/// Serializes the complete machine-node lifecycle, not just identity writes.
/// The file remains after reset so Windows never needs to unlink an open lock
/// or race a new service between deletion and cleanup.
pub(crate) struct NodeLifecycleLock {
    file: fs::File,
    state_was_present: bool,
}

impl NodeLifecycleLock {
    fn acquire(context: &NodeContext, nonblocking: bool) -> Result<Self, NodeError> {
        let state_was_present = prepare_lifecycle_state(context)?;
        let path = context.state_dir().join(LIFECYCLE_LOCK_FILE);
        let file = open_lifecycle_lock(context, &path, nonblocking)?;
        let file = lock_lifecycle_file(file, nonblocking)?;
        Ok(Self {
            file,
            state_was_present,
        })
    }

    pub(crate) fn state_was_present(&self) -> bool {
        self.state_was_present
    }
}

fn prepare_lifecycle_state(context: &NodeContext) -> Result<bool, NodeError> {
    let state_was_present = match fs::symlink_metadata(context.state_dir()) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    context.ensure_state_directory()?;
    Ok(state_was_present)
}

fn open_lifecycle_lock(
    context: &NodeContext,
    path: &Path,
    nonblocking: bool,
) -> Result<fs::File, NodeError> {
    let mut options = crate::util::fs::no_follow_open_options();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|error| classify_lifecycle_lock_error(error, nonblocking))?;
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(NodeError::InsecurePath(
            "node lifecycle lock is a symlink".to_string(),
        ));
    }
    context.validate_private_file(path)?;
    Ok(file)
}

fn lock_lifecycle_file(file: fs::File, nonblocking: bool) -> Result<fs::File, NodeError> {
    let result = if nonblocking {
        file.try_lock_exclusive()
    } else {
        file.lock_exclusive()
    };
    result
        .map(|()| file)
        .map_err(|error| classify_lifecycle_lock_error(error, nonblocking))
}

fn classify_lifecycle_lock_error(error: io::Error, nonblocking: bool) -> NodeError {
    if nonblocking && is_lifecycle_lock_contention(&error) {
        NodeError::LifecycleBusy
    } else {
        error.into()
    }
}

pub(super) fn is_lifecycle_lock_contention(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock || error.kind() == io::ErrorKind::PermissionDenied
    {
        return true;
    }
    #[cfg(windows)]
    {
        // Windows reports sharing and lock violations as raw Win32 errors
        // instead of mapping them to WouldBlock.
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

impl Drop for NodeLifecycleLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
