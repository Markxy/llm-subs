//! What the operating system can tell about a file that is still arriving. Torrent
//! clients create a file before its contents exist and fill it piece by piece, so its
//! size and modification time alone do not show when it is finished.

use std::fs::File;
use std::io;
use std::path::Path;

/// True while another program has the file open for writing. Downloaders and copy
/// tools hold their file open until it is complete. Only Windows reports this.
pub fn is_open_for_writing(path: &Path) -> bool {
    platform::is_open_for_writing(path)
}

/// True when parts of the file have never been written: the file system stores
/// nothing for them yet, which is how torrent clients leave pieces still to come.
pub fn has_unwritten_gaps(path: &Path) -> io::Result<bool> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size == 0 {
        return Ok(false);
    }
    platform::has_gaps(&file, size)
}

#[cfg(windows)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SHARING_VIOLATION};
    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_SPARSE_FILE, FILE_SHARE_READ};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{FILE_ALLOCATED_RANGE_BUFFER, FSCTL_QUERY_ALLOCATED_RANGES};

    /// Ranges asked for per call; a finished file has a single one.
    const RANGES_PER_QUERY: usize = 64;

    /// Opening the file while refusing to share it with writers fails exactly when
    /// another program already has it open for writing.
    pub fn is_open_for_writing(path: &Path) -> bool {
        match OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path) {
            Ok(_) => false,
            Err(error) => error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32),
        }
    }

    /// Only sparse files can have gaps. Their allocated ranges must cover the file
    /// from start to end without a break.
    pub fn has_gaps(file: &File, size: u64) -> io::Result<bool> {
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_SPARSE_FILE == 0 {
            return Ok(false);
        }
        let mut covered_until = 0u64;
        loop {
            let query = FILE_ALLOCATED_RANGE_BUFFER { FileOffset: covered_until as i64, Length: (size - covered_until) as i64 };
            let mut ranges = [FILE_ALLOCATED_RANGE_BUFFER { FileOffset: 0, Length: 0 }; RANGES_PER_QUERY];
            let mut returned_bytes = 0u32;
            // SAFETY: both buffers live for the whole call and their sizes are passed alongside.
            let succeeded = unsafe {
                DeviceIoControl(
                    file.as_raw_handle(),
                    FSCTL_QUERY_ALLOCATED_RANGES,
                    std::ptr::from_ref(&query).cast(),
                    size_of_val(&query) as u32,
                    ranges.as_mut_ptr().cast(),
                    size_of_val(&ranges) as u32,
                    &mut returned_bytes,
                    std::ptr::null_mut(),
                )
            } != 0;
            let more_to_come = !succeeded && io::Error::last_os_error().raw_os_error() == Some(ERROR_MORE_DATA as i32);
            if !succeeded && !more_to_come {
                return Err(io::Error::last_os_error());
            }
            let returned = returned_bytes as usize / size_of::<FILE_ALLOCATED_RANGE_BUFFER>();
            for range in &ranges[..returned] {
                if range.FileOffset as u64 > covered_until {
                    return Ok(true);
                }
                covered_until = covered_until.max((range.FileOffset + range.Length) as u64);
            }
            if !more_to_come || returned == 0 || covered_until >= size {
                return Ok(covered_until < size);
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    pub fn is_open_for_writing(_path: &Path) -> bool {
        false
    }

    /// The first hole after the start lies before the end exactly when the file has a
    /// gap; file systems without hole support report the end.
    pub fn has_gaps(file: &File, size: u64) -> io::Result<bool> {
        // SAFETY: lseek only reads the descriptor, which `file` keeps open.
        let first_hole = unsafe { libc::lseek(file.as_raw_fd(), 0, libc::SEEK_HOLE) };
        if first_hole < 0 {
            return Ok(false);
        }
        Ok((first_hole as u64) < size)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, SeekFrom, Write};

    use super::*;

    #[test]
    fn a_file_that_is_only_partly_written_has_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("download.mkv");
        let mut file = File::create(&path).unwrap();
        mark_sparse(&file);
        file.set_len(64 * 1024 * 1024).unwrap();
        file.seek(SeekFrom::Start(32 * 1024 * 1024)).unwrap();
        file.write_all(&[7u8; 1024 * 1024]).unwrap();
        file.sync_all().unwrap();
        if !file_system_reports_gaps(dir.path()) {
            eprintln!("skipped: this file system does not report gaps");
            return;
        }
        assert!(has_unwritten_gaps(&path).unwrap());
    }

    #[test]
    fn a_fully_written_file_has_no_gaps_and_no_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("finished.mkv");
        std::fs::write(&path, vec![7u8; 3 * 1024 * 1024]).unwrap();
        assert!(!has_unwritten_gaps(&path).unwrap());
        assert!(!is_open_for_writing(&path));
    }

    #[cfg(windows)]
    #[test]
    fn a_file_held_open_by_a_writer_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copying.mkv");
        let writer = File::create(&path).unwrap();
        assert!(is_open_for_writing(&path));
        drop(writer);
        assert!(!is_open_for_writing(&path));
    }

    /// Windows files only get gaps once they are marked sparse, as torrent clients do.
    #[cfg(windows)]
    fn mark_sparse(file: &File) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::IO::DeviceIoControl;
        use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
        let mut returned = 0u32;
        // SAFETY: no buffers are passed; the handle stays open for the call.
        let ok = unsafe { DeviceIoControl(file.as_raw_handle(), FSCTL_SET_SPARSE, std::ptr::null(), 0, std::ptr::null_mut(), 0, &mut returned, std::ptr::null_mut()) };
        assert_ne!(ok, 0, "could not mark the test file sparse");
    }

    #[cfg(unix)]
    fn mark_sparse(_file: &File) {}

    /// Some file systems (for example the Windows drives mounted in WSL) store every
    /// file in full and never report gaps.
    fn file_system_reports_gaps(dir: &Path) -> bool {
        let probe = dir.join("probe");
        let file = File::create(&probe).unwrap();
        mark_sparse(&file);
        file.set_len(16 * 1024 * 1024).unwrap();
        has_unwritten_gaps(&probe).unwrap_or(false)
    }
}
