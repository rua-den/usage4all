//! Atomic file replacement with the Windows "someone has the file open" retry (HANDOFF trap 6).

use std::fs;
use std::io;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

/// ERROR_ACCESS_DENIED (5), ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33), or PermissionDenied:
/// what Python surfaces as `PermissionError` from `os.replace`.
pub fn is_retryable(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// Write `text` to `tmp`, then replace `dest` with it; on a retryable error try again up to `attempts` times
/// (`delay` apart). Last resort (all attempts failed): remove `tmp` and write `dest` directly, non-atomic but
/// the update is never lost. Any other error is returned (tmp removed).
pub fn atomic_write(
    dest: &Path,
    tmp: &Path,
    text: &str,
    attempts: u32,
    delay: Duration,
) -> io::Result<()> {
    fs::write(tmp, text)?;
    for _ in 0..attempts {
        match fs::rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(e) if is_retryable(&e) => sleep(delay),
            Err(e) => {
                let _ = fs::remove_file(tmp);
                return Err(e);
            }
        }
    }
    // Deviation (benign): Python's write_cache leaves the tmp file behind here; we always clean it up.
    let _ = fs::remove_file(tmp);
    fs::write(dest, text)
}
