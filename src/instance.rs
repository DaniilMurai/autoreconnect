use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

/// Two instances would each see the other's Wi-Fi power-cycle as the user turning Wi-Fi off.
/// The OS drops the lock when the process dies, so `kill -9` leaves nothing stale.
pub fn acquire(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join("autoreconnect.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => {
            Err(io::Error::new(io::ErrorKind::WouldBlock, "autoreconnect is already running"))
        }
        Err(TryLockError::Error(e)) => Err(e),
    }
}
