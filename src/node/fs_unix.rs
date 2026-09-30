use super::layout::NodePlatform;
use super::NodeError;
use std::fs;
use std::io::{self};
use std::path::Path;

#[cfg(unix)]
pub(super) fn validate_open_file_identity(
    path: &Path,
    opened_file: &fs::File,
) -> Result<(), NodeError> {
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err(NodeError::InsecurePath(
            "node configuration path is not a regular file".to_string(),
        ));
    }
    let opened_metadata = opened_file.metadata()?;
    if !same_file_identity(&opened_metadata, &path_metadata) {
        return Err(NodeError::InsecurePath(
            "node configuration path changed while opening".to_string(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(unix)]
pub(super) fn create_secure_directory(path: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "node path contains a NUL byte")
    })?;
    let result = unsafe { libc::mkdir(path.as_ptr(), 0o700) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
pub(super) struct UnixOwner {
    pub(super) uid: u32,
    pub(super) gid: u32,
}

#[cfg(unix)]
const PRINCIPAL_LOOKUP_BUFFER_LIMIT: usize = 1024 * 1024;

#[cfg(unix)]
pub(super) fn grow_principal_lookup_buffer(buffer: &mut Vec<u8>) -> Result<(), NodeError> {
    if buffer.len() >= PRINCIPAL_LOOKUP_BUFFER_LIMIT {
        return Err(NodeError::InsecurePath(
            "configured node service principal lookup exceeded the supported size".to_string(),
        ));
    }
    buffer.resize(
        buffer
            .len()
            .saturating_mul(2)
            .min(PRINCIPAL_LOOKUP_BUFFER_LIMIT),
        0,
    );
    Ok(())
}

#[cfg(unix)]
pub(super) fn lookup_unix_principal(
    user_name: &std::ffi::CStr,
    group_name: &std::ffi::CStr,
) -> Result<UnixOwner, NodeError> {
    use std::ptr;

    let uid = {
        let mut entry = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result = ptr::null_mut();
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let status = unsafe {
                libc::getpwnam_r(
                    user_name.as_ptr(),
                    &mut entry,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if status == libc::ERANGE {
                grow_principal_lookup_buffer(&mut buffer)?;
                continue;
            }
            if status != 0 {
                return Err(NodeError::InsecurePath(format!(
                    "failed to resolve configured node service user: {}",
                    io::Error::from_raw_os_error(status)
                )));
            }
            if result.is_null() {
                return Err(NodeError::InsecurePath(
                    "configured node service user does not exist".to_string(),
                ));
            }
            break entry.pw_uid;
        }
    };

    let gid = {
        let mut entry = unsafe { std::mem::zeroed::<libc::group>() };
        let mut result = ptr::null_mut();
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let status = unsafe {
                libc::getgrnam_r(
                    group_name.as_ptr(),
                    &mut entry,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if status == libc::ERANGE {
                grow_principal_lookup_buffer(&mut buffer)?;
                continue;
            }
            if status != 0 {
                return Err(NodeError::InsecurePath(format!(
                    "failed to resolve configured node service group: {}",
                    io::Error::from_raw_os_error(status)
                )));
            }
            if result.is_null() {
                return Err(NodeError::InsecurePath(
                    "configured node service group does not exist".to_string(),
                ));
            }
            break entry.gr_gid;
        }
    };

    Ok(UnixOwner { uid, gid })
}

#[cfg(unix)]
pub(super) fn owner_policy(
    platform: NodePlatform,
    custom_paths: bool,
    state: bool,
) -> Result<UnixOwner, NodeError> {
    use std::ffi::CString;

    if custom_paths {
        return Ok(UnixOwner {
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
        });
    }
    let service_name = match platform {
        NodePlatform::Linux => "omakure",
        NodePlatform::MacOs => "_omakure",
        NodePlatform::Windows => return Ok(UnixOwner { uid: 0, gid: 0 }),
    };
    let user_name = CString::new(service_name).expect("static principal has no NUL");
    let group_name = CString::new(service_name).expect("static principal has no NUL");
    let service_owner = lookup_unix_principal(&user_name, &group_name)?;
    if state {
        Ok(service_owner)
    } else {
        Ok(UnixOwner {
            uid: 0,
            gid: service_owner.gid,
        })
    }
}

/// Is `mode` no broader than `allowed`?
///
/// `allowed` is the *broadest* permission set a node file may carry, not the
/// only one it may carry. A stricter file is always acceptable: an operator
/// hardening `node.toml` from 0640 to 0600 has removed access, not granted it,
/// and refusing to read it turns a hardening step into an outage.
///
/// This cannot loosen a private file. The modes this admits for `allowed =
/// 0o600` are exactly the subsets of 0600 — 0000, 0200, 0400, 0600 — every one
/// of which is at least as strict as 0600, and no group or other bit can ever
/// pass. So one comparison serves both the public config and the private keys
/// without weakening either.
#[cfg(unix)]
pub(super) fn mode_is_no_broader_than(mode: u32, allowed: u32) -> bool {
    mode & 0o777 & !allowed == 0
}
