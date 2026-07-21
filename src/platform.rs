#![allow(unsafe_code)]

use std::{fs, io, path::Path};

#[cfg(target_os = "linux")]
pub fn birth_time(path: &Path) -> io::Result<Option<(i64, u32)>> {
    use rustix::{
        fs::{AtFlags, CWD, StatxFlags, statx},
        io::Errno,
    };

    match statx(CWD, path, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::BTIME) {
        Ok(metadata) => Ok(((metadata.stx_mask & StatxFlags::BTIME.bits()) != 0)
            .then_some((metadata.stx_btime.tv_sec, metadata.stx_btime.tv_nsec))),
        Err(Errno::NOSYS) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn sync_file(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    #[cfg(not(windows))]
    let file = fs::File::open(path)?;
    file.sync_all()
}

pub fn clone_or_copy(source: &Path, destination: &Path) -> io::Result<&'static str> {
    #[cfg(target_os = "macos")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let src = CString::new(source.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let dst = CString::new(destination.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: both C strings are NUL-terminated and live for the duration of the call.
        if unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) } == 0 {
            return Ok("apfs_clonefile");
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::{fs::OpenOptions, os::fd::AsRawFd};
        let src = fs::File::open(source)?;
        let dst = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        // SAFETY: the ioctl receives valid file descriptors and does not outlive them.
        if unsafe { libc::ioctl(dst.as_raw_fd(), libc::FICLONE, src.as_raw_fd()) } == 0 {
            return Ok("linux_ficlone");
        }
        drop(dst);
        let _ = fs::remove_file(destination);
    }
    #[cfg(windows)]
    {
        let mut source = fs::File::open(source)?;
        let mut destination = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        io::copy(&mut source, &mut destination)?;
        Ok("windows_cas_copy")
    }
    #[cfg(not(windows))]
    {
        fs::copy(source, destination)?;
        Ok("cas_copy")
    }
}

#[cfg(unix)]
pub fn set_ownership(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: `path` is NUL-terminated, points to initialized memory, and remains
    // alive for the entire synchronous libc call.
    if unsafe { libc::chown(path.as_ptr(), uid, gid) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
pub fn set_symlink_ownership(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: `path` is a live NUL-terminated string and lchown does not follow
    // the final symlink component.
    if unsafe { libc::lchown(path.as_ptr(), uid, gid) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
