//! Anonymous storage for published display frames shared by QEMU and the UI.
//! The backing descriptor is inherited by QEMU; there is no persistent path.
use memmap2::{Mmap, MmapOptions};
use std::{
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd, RawFd},
    sync::Arc,
};

pub struct SharedRam {
    file: File,
    map: Arc<Mmap>,
}

impl SharedRam {
    pub fn new(size: usize) -> io::Result<Self> {
        if size == 0 || size > libc::off_t::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid shared RAM size",
            ));
        }
        let file = anonymous_file()?;
        // SAFETY: the file owns a valid writable descriptor; size fits off_t.
        if unsafe { libc::ftruncate(file.as_raw_fd(), size as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the backing storage stays allocated for the mapping lifetime.
        // QEMU writes through a shared mapping in another process. Readers must
        // synchronize access using frame publication and consumption acknowledgments.
        let map = unsafe { MmapOptions::new().len(size).map(&file)? };
        Ok(Self {
            file,
            map: Arc::new(map),
        })
    }

    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn map(&self) -> Arc<Mmap> {
        self.map.clone()
    }
}

#[cfg(target_os = "linux")]
fn anonymous_file() -> io::Result<File> {
    let name = c"l6max-display";
    // SAFETY: the static name is NUL terminated. CLOEXEC is cleared only in the
    // specific QEMU child that needs to inherit the descriptor.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: memfd_create returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(not(target_os = "linux"))]
fn anonymous_file() -> io::Result<File> {
    use std::{
        ffi::CString,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    static NEXT_NAME: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            ^ NEXT_NAME.fetch_add(1, Ordering::Relaxed);
        // Keep the name below macOS's 31-byte shared-memory name limit.
        let name = CString::new(format!("/l6-{:x}-{nonce:x}", std::process::id())).unwrap();
        // SAFETY: name is NUL terminated, mode is supplied for O_CREAT.
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EEXIST) {
                continue;
            }
            return Err(error);
        }
        // SAFETY: shm_open returned a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        // Unlink immediately. Both mappings and inherited descriptors retain
        // storage ownership, so a crash cannot leave a named object behind.
        // SAFETY: name is the exact valid name just created.
        if unsafe { libc::shm_unlink(name.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: file owns a live descriptor. Preserve any existing flags.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(file);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate unique shared RAM object",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptor_and_separate_mapping_share_bytes() {
        let ram = SharedRam::new(4096).unwrap();
        let reader = ram.map();
        // SAFETY: this test keeps the backing file allocated and performs no
        // writes while borrowing slices from either mapping.
        let mut second = unsafe { MmapOptions::new().len(4096).map_mut(ram.file()).unwrap() };
        second[123..126].copy_from_slice(&[0x2a, 0x75, 0xff]);
        assert_eq!(&reader[123..126], &[0x2a, 0x75, 0xff]);
        assert_eq!(&second[123..126], &[0x2a, 0x75, 0xff]);
        // SAFETY: ram owns a live descriptor.
        let flags = unsafe { libc::fcntl(ram.fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        drop(ram);
        assert_eq!(reader[123], 0x2a);
    }
    #[test]
    fn rejects_empty_allocation() {
        assert!(
            matches!(SharedRam::new(0), Err(error) if error.kind() == io::ErrorKind::InvalidInput)
        );
    }
}
