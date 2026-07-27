use crate::impact_context::cache::file_facts::{
    create_private_directory, set_private_file_permissions, CacheLayout,
};
use std::fs::{File, OpenOptions, TryLockError};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct WriterLock {
    _file: File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriterLockError {
    pub code: &'static str,
    pub message: String,
}

pub(crate) fn acquire_writer_lock(
    layout: &CacheLayout,
    generation_key: &str,
    deadline: Duration,
) -> Result<WriterLock, WriterLockError> {
    create_private_directory(&layout.locks_dir).map_err(|error| WriterLockError {
        code: "writer-lock-directory-failed",
        message: error.to_string(),
    })?;
    let path = layout.locks_dir.join(format!("{generation_key}.lock"));
    let file = open_lock_file_no_follow(&path).map_err(|error| WriterLockError {
        code: "writer-lock-failed",
        message: format!("cannot open writer lock {}: {error}", path.display()),
    })?;
    set_private_file_permissions(&file).map_err(|error| WriterLockError {
        code: "writer-lock-permission-failed",
        message: error.to_string(),
    })?;

    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(WriterLock { _file: file }),
            Err(TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(WriterLockError {
                        code: "writer-busy",
                        message: "writer lock deadline exhausted".to_string(),
                    });
                }
                std::thread::sleep(remaining.min(Duration::from_millis(5)));
            }
            Err(TryLockError::Error(error)) => {
                return Err(WriterLockError {
                    code: "writer-lock-failed",
                    message: format!("cannot acquire writer lock: {error}"),
                });
            }
        }
    }
}

#[cfg(unix)]
fn open_lock_file_no_follow(path: &std::path::Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "writer lock is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn open_lock_file_no_follow(path: &std::path::Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "writer lock is not a regular file",
        ));
    }
    Ok(file)
}
