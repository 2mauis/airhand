//! No security(1) subprocess: credentials never enter argv or environment.
use airhand_link::direct::Secret;
use anyhow::{Result, anyhow};

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use std::{ffi::c_void, ptr};
    type Item = *mut c_void;
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainFindGenericPassword(
            keychain: *const c_void,
            service_len: u32,
            service: *const u8,
            account_len: u32,
            account: *const u8,
            length: *mut u32,
            data: *mut *mut c_void,
            item: *mut Item,
        ) -> i32;
        fn SecKeychainAddGenericPassword(
            keychain: *const c_void,
            service_len: u32,
            service: *const u8,
            account_len: u32,
            account: *const u8,
            length: u32,
            data: *const c_void,
            item: *mut Item,
        ) -> i32;
        fn SecKeychainItemModifyAttributesAndData(
            item: Item,
            attributes: *const c_void,
            length: u32,
            data: *const c_void,
        ) -> i32;
        fn SecKeychainItemFreeContent(attributes: *mut c_void, data: *mut c_void) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }
    const SERVICE: &[u8] = b"app.airhand.client.tailcatdev.pairing.v1";
    const ACCOUNT: &[u8] = b"single-pair";
    pub fn read() -> Result<Option<Secret<Vec<u8>>>> {
        let (mut length, mut data, mut item) = (0, ptr::null_mut(), ptr::null_mut());
        let status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null(),
                SERVICE.len() as u32,
                SERVICE.as_ptr(),
                ACCOUNT.len() as u32,
                ACCOUNT.as_ptr(),
                &mut length,
                &mut data,
                &mut item,
            )
        };
        if status == -25300 {
            return Ok(None);
        }
        if status != 0 {
            return Err(anyhow!("Keychain access denied or unavailable"));
        }
        let value = if length <= 512 * 1024 && !data.is_null() {
            Some(Secret::new(
                unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length as usize) }.to_vec(),
            ))
        } else {
            None
        };
        unsafe {
            SecKeychainItemFreeContent(ptr::null_mut(), data);
            if !item.is_null() {
                CFRelease(item)
            }
        };
        value
            .map(Some)
            .ok_or_else(|| anyhow!("invalid Keychain record"))
    }
    pub fn write(value: &Secret<Vec<u8>>) -> Result<()> {
        if value.expose().len() > 512 * 1024 {
            return Err(anyhow!("pairing record too large"));
        }
        let mut item = ptr::null_mut();
        let find = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null(),
                SERVICE.len() as u32,
                SERVICE.as_ptr(),
                ACCOUNT.len() as u32,
                ACCOUNT.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut item,
            )
        };
        let data = value.expose();
        let status = if find == -25300 {
            unsafe {
                SecKeychainAddGenericPassword(
                    ptr::null(),
                    SERVICE.len() as u32,
                    SERVICE.as_ptr(),
                    ACCOUNT.len() as u32,
                    ACCOUNT.as_ptr(),
                    data.len() as u32,
                    data.as_ptr().cast(),
                    ptr::null_mut(),
                )
            }
        } else if find == 0 {
            let result = unsafe {
                SecKeychainItemModifyAttributesAndData(
                    item,
                    ptr::null(),
                    data.len() as u32,
                    data.as_ptr().cast(),
                )
            };
            unsafe { CFRelease(item) };
            result
        } else {
            find
        };
        if status != 0 {
            return Err(anyhow!("Keychain update failed"));
        };
        Ok(())
    }
}
#[cfg(target_os = "macos")]
pub use native::{read, write};
#[cfg(not(target_os = "macos"))]
pub fn read() -> Result<Option<Secret<Vec<u8>>>> {
    Err(anyhow!("Tailcat pairing requires macOS Keychain"))
}
#[cfg(not(target_os = "macos"))]
pub fn write(_: &Secret<Vec<u8>>) -> Result<()> {
    Err(anyhow!("Tailcat pairing requires macOS Keychain"))
}
