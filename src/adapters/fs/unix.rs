use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[derive(Debug)]
pub(crate) enum FsError {
    Nul,
    Io(io::Error),
}

fn cstring(value: &OsStr) -> Result<CString, FsError> {
    CString::new(value.as_bytes()).map_err(|_| FsError::Nul)
}

pub(crate) fn open_dir_no_follow(path: &Path) -> Result<File, FsError> {
    let path = cstring(path.as_os_str())?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(FsError::Io(io::Error::last_os_error()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(crate) fn create_new_file_at(parent: &File, name: &OsStr) -> Result<File, FsError> {
    let name = cstring(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(FsError::Io(io::Error::last_os_error()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(crate) fn open_existing_file_at_no_follow(
    parent: &File,
    name: &OsStr,
) -> Result<File, FsError> {
    let name = cstring(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(FsError::Io(io::Error::last_os_error()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(crate) fn renameat_file(parent: &File, from: &OsStr, to: &OsStr) -> Result<(), FsError> {
    let from = cstring(from)?;
    let to = cstring(to)?;
    let rc = unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(FsError::Io(io::Error::last_os_error()))
    }
}

pub(crate) fn linkat_file(parent: &File, from: &OsStr, to: &OsStr) -> Result<(), FsError> {
    let from = cstring(from)?;
    let to = cstring(to)?;
    let rc = unsafe {
        libc::linkat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            0,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(FsError::Io(io::Error::last_os_error()))
    }
}

pub(crate) fn unlinkat_file(parent: &File, name: &OsStr) -> Result<(), FsError> {
    let name = cstring(name)?;
    let rc = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(FsError::Io(io::Error::last_os_error()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn no_follow_open_rejects_symlinks_and_nul_components() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("directory");
        std::fs::create_dir(&directory).unwrap();
        let directory_link = temp.path().join("directory-link");
        symlink(&directory, &directory_link).unwrap();
        assert!(matches!(
            open_dir_no_follow(&directory_link),
            Err(FsError::Io(_))
        ));

        let parent = open_dir_no_follow(&directory).unwrap();
        std::fs::write(directory.join("target"), "safe").unwrap();
        symlink("target", directory.join("target-link")).unwrap();
        assert!(matches!(
            open_existing_file_at_no_follow(&parent, OsStr::new("target-link")),
            Err(FsError::Io(error)) if error.raw_os_error() == Some(libc::ELOOP)
        ));
        assert!(matches!(
            create_new_file_at(&parent, OsStr::from_bytes(b"invalid\0name")),
            Err(FsError::Nul)
        ));
        assert_eq!(
            std::fs::read_to_string(directory.join("target")).unwrap(),
            "safe"
        );
    }
}
