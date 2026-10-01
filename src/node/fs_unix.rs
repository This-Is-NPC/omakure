use super::NodeError;
use super::layout::NodePlatform;
use crate::adapters::fs::unix as fs_adapter;
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
    fs_adapter::mkdir_private(&path)
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
    let uid = lookup_principal_id(user_name, "user", fs_adapter::user_id_by_name)?;
    let gid = lookup_principal_id(group_name, "group", fs_adapter::group_id_by_name)?;
    Ok(UnixOwner { uid, gid })
}

#[cfg(unix)]
fn lookup_principal_id(
    name: &std::ffi::CStr,
    kind: &str,
    lookup: fn(&std::ffi::CStr, &mut [u8]) -> Result<Option<u32>, i32>,
) -> Result<u32, NodeError> {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        match lookup(name, &mut buffer) {
            Ok(Some(id)) => return Ok(id),
            Ok(None) => {
                return Err(NodeError::InsecurePath(format!(
                    "configured node service {kind} does not exist"
                )));
            }
            Err(libc::ERANGE) => grow_principal_lookup_buffer(&mut buffer)?,
            Err(status) => {
                return Err(NodeError::InsecurePath(format!(
                    "failed to resolve configured node service {kind}: {}",
                    io::Error::from_raw_os_error(status)
                )));
            }
        }
    }
}

#[cfg(unix)]
pub(super) fn owner_policy(
    platform: NodePlatform,
    custom_paths: bool,
    state: bool,
) -> Result<UnixOwner, NodeError> {
    use std::ffi::CString;

    if custom_paths {
        let (uid, gid) = fs_adapter::effective_owner();
        return Ok(UnixOwner { uid, gid });
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
