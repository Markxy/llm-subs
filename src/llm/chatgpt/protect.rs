//! Keeps ChatGPT tokens out of plain sight on disk. Windows encrypts them for the
//! current user with DPAPI; elsewhere the file is readable by its owner only.

use std::path::Path;

use anyhow::{Context, Result};

#[cfg(windows)]
pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points at `plain`, which outlives the call; DPAPI allocates `output`.
    let succeeded = unsafe {
        CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error()).context("encrypting the ChatGPT sign-in");
    }
    Ok(take_blob(output))
}

#[cfg(windows)]
pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: sealed.len() as u32, pbData: sealed.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    let succeeded = unsafe {
        CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error()).context("decrypting the ChatGPT sign-in");
    }
    Ok(take_blob(output))
}

#[cfg(windows)]
fn take_blob(blob: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: DPAPI returned a buffer of `cbData` bytes that we own and must free.
    unsafe {
        let bytes = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        windows_sys::Win32::Foundation::LocalFree(blob.pbData as *mut core::ffi::c_void);
        bytes
    }
}

#[cfg(not(windows))]
pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
    Ok(plain.to_vec())
}

#[cfg(not(windows))]
pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>> {
    Ok(sealed.to_vec())
}

/// Replaces `path` atomically with a file only the current user can read.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temp_path).with_context(|| format!("writing {}", temp_path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp_path, path).with_context(|| format!("writing {}", path.display()))
}
