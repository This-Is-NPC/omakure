use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;

pub(crate) fn open_existing_file_read(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowsFileIdentity {
    volume_serial_number: u32,
    file_index: u64,
    pub(crate) reparse_point: bool,
}

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

pub(crate) fn file_identity(file: &File) -> io::Result<WindowsFileIdentity> {
    let mut information = ByHandleFileInformation::default();
    let success = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(WindowsFileIdentity {
        volume_serial_number: information.volume_serial_number,
        file_index: (u64::from(information.file_index_high) << 32)
            | u64::from(information.file_index_low),
        reparse_point: information.file_attributes & 0x400 != 0,
    })
}

pub(crate) fn has_reparse_point(path: &Path) -> io::Result<bool> {
    const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let wide = crate::util::windows::wide_path(path);
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        Err(io::Error::last_os_error())
    } else {
        Ok(attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
}

const OWNER_AND_DACL_SECURITY_INFORMATION: u32 = 0x0000_0005;
const SE_FILE_OBJECT: u32 = 1;

pub(crate) struct SecurityDescriptor {
    descriptor: *mut std::ffi::c_void,
    owner: *mut std::ffi::c_void,
    dacl: *mut std::ffi::c_void,
}

impl SecurityDescriptor {
    pub(crate) fn named(path: &Path) -> Result<Self, u32> {
        let wide = crate::util::windows::wide_path(path);
        let mut result = Self {
            descriptor: ptr::null_mut(),
            owner: ptr::null_mut(),
            dacl: ptr::null_mut(),
        };
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_AND_DACL_SECURITY_INFORMATION,
                &mut result.owner,
                ptr::null_mut(),
                &mut result.dacl,
                ptr::null_mut(),
                &mut result.descriptor,
            )
        };
        if status == 0 { Ok(result) } else { Err(status) }
    }

    pub(crate) fn from_handle(file: &File) -> Result<Self, u32> {
        let mut result = Self {
            descriptor: ptr::null_mut(),
            owner: ptr::null_mut(),
            dacl: ptr::null_mut(),
        };
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_AND_DACL_SECURITY_INFORMATION,
                &mut result.owner,
                ptr::null_mut(),
                &mut result.dacl,
                ptr::null_mut(),
                &mut result.descriptor,
            )
        };
        if status == 0 { Ok(result) } else { Err(status) }
    }

    pub(crate) fn has_dacl(&self) -> bool {
        !self.dacl.is_null()
    }

    pub(crate) fn ace_count(&self) -> Option<u32> {
        #[repr(C)]
        struct AclSizeInformation {
            ace_count: u32,
            acl_bytes_in_use: u32,
            acl_bytes_free: u32,
        }
        const ACL_SIZE_INFORMATION_CLASS: u32 = 2;
        let mut info = AclSizeInformation {
            ace_count: 0,
            acl_bytes_in_use: 0,
            acl_bytes_free: 0,
        };
        let status = unsafe {
            GetAclInformation(
                self.dacl,
                &mut info as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<AclSizeInformation>() as u32,
                ACL_SIZE_INFORMATION_CLASS,
            )
        };
        (status != 0).then_some(info.ace_count)
    }

    pub(crate) fn allowed_sids() -> Option<AllowedSids> {
        const WIN_LOCAL_SYSTEM_SID: u32 = 22;
        const WIN_LOCAL_SERVICE_SID: u32 = 23;
        let mut result = AllowedSids {
            system: [0; 68],
            service: [0; 68],
        };
        let mut system_size = result.system.len() as u32;
        let mut service_size = result.service.len() as u32;
        let success = unsafe {
            CreateWellKnownSid(
                WIN_LOCAL_SYSTEM_SID,
                ptr::null_mut(),
                result.system.as_mut_ptr().cast(),
                &mut system_size,
            ) != 0
                && CreateWellKnownSid(
                    WIN_LOCAL_SERVICE_SID,
                    ptr::null_mut(),
                    result.service.as_mut_ptr().cast(),
                    &mut service_size,
                ) != 0
        };
        success.then_some(result)
    }

    pub(crate) fn owner_is_system(&self, allowed: &mut AllowedSids) -> bool {
        !self.owner.is_null()
            && unsafe { EqualSid(self.owner, allowed.system.as_mut_ptr().cast()) != 0 }
    }

    pub(crate) fn ace(&self, index: u32, allowed: &mut AllowedSids) -> AceRead {
        #[repr(C)]
        struct AceHeader {
            ace_type: u8,
            ace_flags: u8,
            ace_size: u16,
        }
        let mut ace: *mut std::ffi::c_void = ptr::null_mut();
        if unsafe { GetAce(self.dacl, index, &mut ace) == 0 } || ace.is_null() {
            return AceRead::Unreadable;
        }
        let header = unsafe { &*(ace as *const AceHeader) };
        if header.ace_type != 0 {
            return AceRead::NonAllow;
        }
        if header.ace_size < (std::mem::size_of::<AceHeader>() + std::mem::size_of::<u32>()) as u16
        {
            return AceRead::Malformed;
        }
        let sid = unsafe {
            (ace as *const u8).add(std::mem::size_of::<AceHeader>() + std::mem::size_of::<u32>())
                as *mut std::ffi::c_void
        };
        let mask =
            unsafe { *((ace as *const u8).add(std::mem::size_of::<AceHeader>()) as *const u32) };
        AceRead::Entry {
            mask,
            is_system: unsafe { EqualSid(sid, allowed.system.as_mut_ptr().cast()) != 0 },
            is_service: unsafe { EqualSid(sid, allowed.service.as_mut_ptr().cast()) != 0 },
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}

pub(crate) struct AllowedSids {
    system: [u8; 68],
    service: [u8; 68],
}

pub(crate) enum AceRead {
    Unreadable,
    NonAllow,
    Malformed,
    Entry {
        mask: u32,
        is_system: bool,
        is_service: bool,
    },
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetFileInformationByHandle(
        handle: *mut std::ffi::c_void,
        file_information: *mut ByHandleFileInformation,
    ) -> i32;
    fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[cfg(windows)]
#[link(name = "advapi32")]
unsafe extern "system" {
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
