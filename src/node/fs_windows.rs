use super::NodeError;
use crate::adapters::fs::windows::{self as fs_adapter, AceRead, AllowedSids, SecurityDescriptor};
use std::fs;
use std::io;
use std::path::Path;

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
    if windows_has_reparse_point(path)? {
        return Err(NodeError::UnsafePath(
            "node configuration path has a reparse point".to_string(),
        ));
    }
    let mut options = crate::util::fs::no_follow_open_options();
    options.read(true);
    let current_file = options.open(path).map_err(NodeError::Io)?;
    let opened_identity = fs_adapter::file_identity(opened_file).map_err(NodeError::Io)?;
    let current_identity = fs_adapter::file_identity(&current_file).map_err(NodeError::Io)?;
    if opened_identity.reparse_point || current_identity.reparse_point {
        return Err(NodeError::UnsafePath(
            "node configuration path has a reparse point".to_string(),
        ));
    }
    if opened_identity != current_identity {
        return Err(NodeError::InsecurePath(
            "node configuration path changed while opening".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn windows_has_reparse_point(path: &Path) -> Result<bool, NodeError> {
    fs_adapter::has_reparse_point(path).map_err(|err| {
        NodeError::Io(io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))
    })
}

pub(super) fn validate_windows_security(
    path: &Path,
    directory: bool,
    test_mode: bool,
) -> Result<(), NodeError> {
    if windows_has_reparse_point(path)? {
        return Err(NodeError::UnsafePath(path.display().to_string()));
    }
    let descriptor = SecurityDescriptor::named(path).map_err(|status| {
        NodeError::InsecurePath(format!(
            "cannot read ACL for {} (error {status})",
            path.display()
        ))
    })?;
    validate_windows_security_descriptor(path, directory, test_mode, &descriptor)
}

pub(super) fn validate_windows_security_handle(
    path: &Path,
    file: &fs::File,
    test_mode: bool,
) -> Result<(), NodeError> {
    let descriptor = SecurityDescriptor::from_handle(file).map_err(|status| {
        NodeError::InsecurePath(format!(
            "cannot read ACL for {} (error {status})",
            path.display()
        ))
    })?;
    validate_windows_security_descriptor(path, false, test_mode, &descriptor)
}

fn validate_windows_security_descriptor(
    path: &Path,
    directory: bool,
    test_mode: bool,
    descriptor: &SecurityDescriptor,
) -> Result<(), NodeError> {
    if !descriptor.has_dacl() {
        return Err(NodeError::InsecurePath(format!(
            "{} has no explicit DACL",
            path.display()
        )));
    }
    if test_mode {
        return Ok(());
    }

    let ace_count = descriptor
        .ace_count()
        .filter(|count| *count > 0)
        .ok_or_else(|| unreadable_or_empty_dacl(path))?;
    let mut allowed = SecurityDescriptor::allowed_sids().ok_or_else(|| {
        NodeError::InsecurePath("cannot construct required Windows service SIDs".to_string())
    })?;
    if !descriptor.owner_is_system(&mut allowed) {
        return Err(NodeError::InsecurePath(format!(
            "{} has an unexpected owner",
            path.display()
        )));
    }

    let mut saw_system = false;
    let mut saw_service = false;
    for index in 0..ace_count {
        let (mask, is_system, is_service) = validated_ace(path, descriptor, index, &mut allowed)?;
        if !is_system && !is_service {
            return Err(NodeError::InsecurePath(format!(
                "{} grants access to an unexpected principal",
                path.display()
            )));
        }
        if !windows_security_access_allowed(directory, is_system, is_service, mask) {
            return Err(NodeError::InsecurePath(format!(
                "{} grants an invalid access mask",
                path.display()
            )));
        }
        saw_system |= is_system;
        saw_service |= is_service;
    }
    if !saw_system || !saw_service {
        return Err(NodeError::InsecurePath(format!(
            "{} must grant only LocalService and SYSTEM and include both",
            path.display()
        )));
    }
    Ok(())
}

fn validated_ace(
    path: &Path,
    descriptor: &SecurityDescriptor,
    index: u32,
    allowed: &mut AllowedSids,
) -> Result<(u32, bool, bool), NodeError> {
    match descriptor.ace(index, allowed) {
        AceRead::Unreadable => Err(NodeError::InsecurePath(format!(
            "cannot inspect ACL for {}",
            path.display()
        ))),
        AceRead::NonAllow => Err(NodeError::InsecurePath(format!(
            "{} has a non-allow ACL entry",
            path.display()
        ))),
        AceRead::Malformed => Err(NodeError::InsecurePath(format!(
            "{} has a malformed ACL entry",
            path.display()
        ))),
        AceRead::Entry {
            mask,
            is_system,
            is_service,
        } => Ok((mask, is_system, is_service)),
    }
}

fn unreadable_or_empty_dacl(path: &Path) -> NodeError {
    NodeError::InsecurePath(format!(
        "{} has an unreadable or empty DACL",
        path.display()
    ))
}

pub(super) fn windows_security_access_allowed(
    directory: bool,
    is_system: bool,
    is_service: bool,
    mask: u32,
) -> bool {
    const FILE_GENERIC_READ: u32 = 0x0012_0089;
    const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
    const FILE_WRITABLE_SPECIFIC: u32 = 0x0000_0116;
    if is_system == is_service || mask & FILE_GENERIC_READ != FILE_GENERIC_READ {
        return false;
    }
    (is_system && mask & FILE_GENERIC_WRITE == FILE_GENERIC_WRITE)
        || (is_service && (directory || mask & FILE_WRITABLE_SPECIFIC == 0))
}
