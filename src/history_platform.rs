//! Filesystem guarantees used by history mutation. Windows has no portable
//! directory fsync: files are flushed and publication uses write-through moves.
#[cfg(windows)]
use anyhow::Context;
use anyhow::{Result, ensure};
use std::{fs, path::Path};

#[cfg(unix)]
pub const DIRECTORY_SYNC: &str = "fsync";
#[cfg(windows)]
pub const DIRECTORY_SYNC: &str = "unavailable-windows-write-through-renames";

pub fn reject_redirect(metadata: &fs::Metadata, path: &Path) -> Result<()> {
    ensure!(
        !metadata.file_type().is_symlink(),
        "symlink rejected: {}",
        path.display()
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "reparse point rejected: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
pub fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
#[cfg(unix)]
pub fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(test)]
    injected_sync_failure(path)?;
    fs::File::open(path)?.sync_all()?;
    Ok(())
}
#[cfg(windows)]
pub fn sync_dir(_path: &Path) -> Result<()> {
    #[cfg(test)]
    injected_sync_failure(_path)?;
    // The manifest explicitly records this platform limitation. MoveFileExW
    // WRITE_THROUGH is used for both new registrations and journal replacement.
    Ok(())
}

pub fn publish(file: tempfile::NamedTempFile, path: &Path, replace: bool) -> Result<()> {
    #[cfg(unix)]
    {
        if replace {
            file.persist(path).map_err(|e| e.error)?;
        } else {
            file.persist_noclobber(path).map_err(|e| e.error)?;
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let temp = file.into_temp_path();
        let flags = MOVEFILE_WRITE_THROUGH
            | if replace {
                MOVEFILE_REPLACE_EXISTING
            } else {
                0
            };
        let ok = unsafe { MoveFileExW(wide(temp.as_ref()).as_ptr(), wide(path).as_ptr(), flags) };
        ensure!(
            ok != 0,
            "write-through publication failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(windows)]
pub fn private_dir(path: &Path) -> Result<()> {
    use std::ptr::null_mut;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree},
        Security::Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        Security::{
            DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
            SetFileSecurityW, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    // Each allocation/handle is released even on error. The protected DACL has
    // exactly one inheritable ACE: the current process token's user SID.
    unsafe {
        let mut token = null_mut();
        ensure!(
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) != 0,
            "cannot inspect current user token: {}",
            std::io::Error::last_os_error()
        );
        let result = (|| -> Result<()> {
            let mut size = 0;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
            ensure!(size > 0, "cannot size current user token");
            // usize storage provides alignment for TOKEN_USER on both architectures.
            let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            ensure!(
                GetTokenInformation(token, TokenUser, data.as_mut_ptr().cast(), size, &mut size)
                    != 0,
                "cannot read current user token: {}",
                std::io::Error::last_os_error()
            );
            let user = &*(data.as_ptr().cast::<TOKEN_USER>());
            let mut sid = null_mut();
            ensure!(
                ConvertSidToStringSidW(user.User.Sid, &mut sid) != 0,
                "cannot encode current user SID: {}",
                std::io::Error::last_os_error()
            );
            let mut length = 0;
            while *sid.add(length) != 0 {
                length += 1;
            }
            let sid_string = String::from_utf16(std::slice::from_raw_parts(sid, length));
            LocalFree(sid.cast());
            let sddl: Vec<u16> = format!(
                "D:P(A;OICI;FA;;;{})",
                sid_string.context("invalid SID encoding")?
            )
            .encode_utf16()
            .chain(Some(0))
            .collect();
            let mut descriptor = null_mut();
            ensure!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut()
                ) != 0,
                "cannot construct private DACL: {}",
                std::io::Error::last_os_error()
            );
            let ok = SetFileSecurityW(
                wide(path).as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            );
            let error = std::io::Error::last_os_error();
            LocalFree(descriptor);
            ensure!(ok != 0, "cannot secure backup directory: {error}");
            Ok(())
        })();
        CloseHandle(token);
        result
    }
}

// Per-thread fault injection exercises the critical publish -> directory fsync
// failure window without affecting other tests or production builds.
#[cfg(test)]
std::thread_local! {
    static FAIL_SYNC: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(crate) fn fail_next_sync(path: &Path) {
    FAIL_SYNC.with(|slot| *slot.borrow_mut() = Some(path.to_owned()));
}
#[cfg(test)]
fn injected_sync_failure(path: &Path) -> Result<()> {
    let fail = FAIL_SYNC.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_deref() == Some(path) {
            slot.take();
            true
        } else {
            false
        }
    });
    ensure!(!fail, "injected directory sync failure");
    Ok(())
}
