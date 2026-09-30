use super::NodeError;
use std::fs;
use std::io;
use std::path::Path;

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsFileIdentity {
    volume_serial_number: u32,
    file_index: u64,
    reparse_point: bool,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Default)]
struct ByHandleFileInformation {
    file_attributes: u32,
    creation_time: [u32; 2],
    last_access_time: [u32; 2],
    last_write_time: [u32; 2],
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
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
    let opened_identity = windows_file_identity(opened_file)?;
    let current_identity = windows_file_identity(&current_file)?;
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

#[cfg(windows)]
fn windows_file_identity(file: &fs::File) -> Result<WindowsFileIdentity, NodeError> {
    use std::os::windows::io::AsRawHandle;

    let mut information = ByHandleFileInformation::default();
    let success = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    if success == 0 {
        return Err(NodeError::Io(io::Error::last_os_error()));
    }
    Ok(WindowsFileIdentity {
        volume_serial_number: information.volume_serial_number,
        file_index: (u64::from(information.file_index_high) << 32)
            | u64::from(information.file_index_low),
        reparse_point: information.file_attributes & 0x400 != 0,
    })
}

#[cfg(windows)]
pub(super) fn windows_has_reparse_point(path: &Path) -> Result<bool, NodeError> {
    const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let wide = crate::util::windows::wide_path(path);
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        let err = io::Error::last_os_error();
        return Err(NodeError::Io(io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        )));
    }
    Ok(attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(windows)]
const OWNER_AND_DACL_SECURITY_INFORMATION: u32 = 0x0000_0005;

#[cfg(windows)]
const SE_FILE_OBJECT: u32 = 1;

#[cfg(windows)]
const ERROR_SUCCESS: u32 = 0;

#[cfg(windows)]
pub(super) fn validate_windows_security(
    path: &Path,
    directory: bool,
    test_mode: bool,
) -> Result<(), NodeError> {
    use std::ptr;

    if windows_has_reparse_point(path)? {
        return Err(NodeError::UnsafePath(path.display().to_string()));
    }

    let wide = crate::util::windows::wide_path(path);
    let mut security_descriptor: *mut std::ffi::c_void = ptr::null_mut();
    let mut owner_sid: *mut std::ffi::c_void = ptr::null_mut();
    let mut dacl: *mut std::ffi::c_void = ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_AND_DACL_SECURITY_INFORMATION,
            &mut owner_sid,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut security_descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(NodeError::InsecurePath(format!(
            "cannot read ACL for {} (error {status})",
            path.display()
        )));
    }
    validate_windows_security_descriptor(
        path,
        directory,
        test_mode,
        owner_sid,
        dacl,
        security_descriptor,
    )
}

#[cfg(windows)]
pub(super) fn validate_windows_security_handle(
    path: &Path,
    file: &fs::File,
    test_mode: bool,
) -> Result<(), NodeError> {
    use std::os::windows::io::AsRawHandle;
    use std::ptr;

    let mut security_descriptor: *mut std::ffi::c_void = ptr::null_mut();
    let mut owner_sid: *mut std::ffi::c_void = ptr::null_mut();
    let mut dacl: *mut std::ffi::c_void = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_AND_DACL_SECURITY_INFORMATION,
            &mut owner_sid,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut security_descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(NodeError::InsecurePath(format!(
            "cannot read ACL for {} (error {status})",
            path.display()
        )));
    }
    validate_windows_security_descriptor(
        path,
        false,
        test_mode,
        owner_sid,
        dacl,
        security_descriptor,
    )
}

#[cfg(windows)]
fn validate_windows_security_descriptor(
    path: &Path,
    directory: bool,
    test_mode: bool,
    owner_sid: *mut std::ffi::c_void,
    dacl: *mut std::ffi::c_void,
    security_descriptor: *mut std::ffi::c_void,
) -> Result<(), NodeError> {
    const ACL_SIZE_INFORMATION_CLASS: u32 = 2;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const WIN_LOCAL_SYSTEM_SID: u32 = 22;
    const WIN_LOCAL_SERVICE_SID: u32 = 23;
    const SECURITY_MAX_SID_SIZE: usize = 68;
    use std::ptr;

    #[repr(C)]
    struct AceHeader {
        ace_type: u8,
        ace_flags: u8,
        ace_size: u16,
    }

    #[repr(C)]
    struct AclSizeInformation {
        ace_count: u32,
        acl_bytes_in_use: u32,
        acl_bytes_free: u32,
    }

    let result = (|| {
        if dacl.is_null() {
            return Err(NodeError::InsecurePath(format!(
                "{} has no explicit DACL",
                path.display()
            )));
        }
        if test_mode {
            return Ok(());
        }

        let mut acl_info = AclSizeInformation {
            ace_count: 0,
            acl_bytes_in_use: 0,
            acl_bytes_free: 0,
        };
        let acl_status = unsafe {
            GetAclInformation(
                dacl,
                &mut acl_info as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<AclSizeInformation>() as u32,
                ACL_SIZE_INFORMATION_CLASS,
            )
        };
        if acl_status == 0 || acl_info.ace_count == 0 {
            return Err(NodeError::InsecurePath(format!(
                "{} has an unreadable or empty DACL",
                path.display()
            )));
        }
        let mut allowed_system = [0u8; SECURITY_MAX_SID_SIZE];
        let mut allowed_service = [0u8; SECURITY_MAX_SID_SIZE];
        let mut system_size = allowed_system.len() as u32;
        let mut service_size = allowed_service.len() as u32;
        if unsafe {
            CreateWellKnownSid(
                WIN_LOCAL_SYSTEM_SID,
                ptr::null_mut(),
                allowed_system.as_mut_ptr() as *mut std::ffi::c_void,
                &mut system_size,
            ) == 0
                || CreateWellKnownSid(
                    WIN_LOCAL_SERVICE_SID,
                    ptr::null_mut(),
                    allowed_service.as_mut_ptr() as *mut std::ffi::c_void,
                    &mut service_size,
                ) == 0
        } {
            return Err(NodeError::InsecurePath(
                "cannot construct required Windows service SIDs".to_string(),
            ));
        }
        if owner_sid.is_null()
            || unsafe {
                EqualSid(
                    owner_sid,
                    allowed_system.as_mut_ptr() as *mut std::ffi::c_void,
                ) == 0
            }
        {
            return Err(NodeError::InsecurePath(format!(
                "{} has an unexpected owner",
                path.display()
            )));
        }
        let mut saw_system = false;
        let mut saw_service = false;
        for index in 0..acl_info.ace_count {
            let mut ace: *mut std::ffi::c_void = ptr::null_mut();
            if unsafe { GetAce(dacl, index, &mut ace) == 0 } || ace.is_null() {
                return Err(NodeError::InsecurePath(format!(
                    "cannot inspect ACL for {}",
                    path.display()
                )));
            }
            let header = unsafe { &*(ace as *const AceHeader) };
            if header.ace_type != ACCESS_ALLOWED_ACE_TYPE {
                return Err(NodeError::InsecurePath(format!(
                    "{} has a non-allow ACL entry",
                    path.display()
                )));
            }
            if header.ace_size
                < (std::mem::size_of::<AceHeader>() + std::mem::size_of::<u32>()) as u16
            {
                return Err(NodeError::InsecurePath(format!(
                    "{} has a malformed ACL entry",
                    path.display()
                )));
            }
            let sid = unsafe {
                (ace as *const u8)
                    .add(std::mem::size_of::<AceHeader>() + std::mem::size_of::<u32>())
                    as *mut std::ffi::c_void
            };
            let mask = unsafe {
                *((ace as *const u8).add(std::mem::size_of::<AceHeader>()) as *const u32)
            };
            let is_system =
                unsafe { EqualSid(sid, allowed_system.as_mut_ptr() as *mut std::ffi::c_void) != 0 };
            let is_service = unsafe {
                EqualSid(sid, allowed_service.as_mut_ptr() as *mut std::ffi::c_void) != 0
            };
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
    })();
    unsafe {
        LocalFree(security_descriptor);
    }
    result
}

#[cfg(windows)]
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

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetFileInformationByHandle(
        handle: *mut std::ffi::c_void,
        file_information: *mut ByHandleFileInformation,
    ) -> i32;
    fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[cfg(windows)]
#[link(name = "advapi32")]
extern "system" {
    fn GetSecurityInfo(
        handle: *mut std::ffi::c_void,
        object_type: u32,
        security_info: u32,
        owner: *mut *mut std::ffi::c_void,
        group: *mut *mut std::ffi::c_void,
        dacl: *mut *mut std::ffi::c_void,
        sacl: *mut *mut std::ffi::c_void,
        security_descriptor: *mut *mut std::ffi::c_void,
    ) -> u32;
    fn GetNamedSecurityInfoW(
        object_name: *const u16,
        object_type: u32,
        security_info: u32,
        owner: *mut *mut std::ffi::c_void,
        group: *mut *mut std::ffi::c_void,
        dacl: *mut *mut std::ffi::c_void,
        sacl: *mut *mut std::ffi::c_void,
        security_descriptor: *mut *mut std::ffi::c_void,
    ) -> u32;
    fn GetAclInformation(
        acl: *mut std::ffi::c_void,
        acl_information: *mut std::ffi::c_void,
        acl_information_length: u32,
        acl_information_class: u32,
    ) -> i32;
    fn GetAce(acl: *mut std::ffi::c_void, ace_index: u32, ace: *mut *mut std::ffi::c_void) -> i32;
    fn EqualSid(first: *mut std::ffi::c_void, second: *mut std::ffi::c_void) -> i32;
    fn CreateWellKnownSid(
        sid_type: u32,
        domain: *mut std::ffi::c_void,
        sid: *mut std::ffi::c_void,
        sid_size: *mut u32,
    ) -> i32;
}

// Keep the hand-written declarations tied to the Win32 ABI and documented
// parameter shapes when this module is compiled for Windows.
#[cfg(windows)]
const _: unsafe extern "system" fn(*const u16) -> u32 = GetFileAttributesW;

#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void, *mut ByHandleFileInformation) -> i32 =
    GetFileInformationByHandle;

#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void = LocalFree;

#[cfg(windows)]
const _: unsafe extern "system" fn(
    *mut std::ffi::c_void,
    u32,
    u32,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
) -> u32 = GetSecurityInfo;

#[cfg(windows)]
const _: unsafe extern "system" fn(
    *const u16,
    u32,
    u32,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
) -> u32 = GetNamedSecurityInfoW;

#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, u32, u32) -> i32 =
    GetAclInformation;

#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void, u32, *mut *mut std::ffi::c_void) -> i32 =
    GetAce;

#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32 = EqualSid;

#[cfg(windows)]
const _: unsafe extern "system" fn(
    u32,
    *mut std::ffi::c_void,
    *mut std::ffi::c_void,
    *mut u32,
) -> i32 = CreateWellKnownSid;
