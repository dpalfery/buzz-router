//! The admin token file, `<data-dir>/admin.token` (design section 8, R43.4).

use std::io::Write;
use std::path::Path;

use super::ApiError;

/// The file name under the data directory.
pub const FILE_NAME: &str = "admin.token";

/// Returns the admin token in `data_dir`, creating it first if it is missing: 32 random bytes,
/// hex-encoded, readable only by the user (mode 0600 on Unix; the per-user ACL of the data
/// directory on Windows). An existing file is never overwritten.
pub fn ensure(data_dir: &Path) -> Result<String, ApiError> {
    let path = data_dir.join(FILE_NAME);
    let io = |source| ApiError::Io {
        path: path.clone(),
        source,
    };
    match std::fs::read_to_string(&path) {
        Ok(token) => return Ok(token.trim().to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io(error)),
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| ApiError::Random(error.to_string()))?;
    let token = hex::encode(bytes);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return std::fs::read_to_string(&path)
                .map(|token| token.trim().to_owned())
                .map_err(io);
        }
        Err(error) => return Err(io(error)),
    };
    file.write_all(token.as_bytes()).map_err(io)?;
    Ok(token)
}

/// Reads the admin token in `data_dir` without creating it, for the CLI.
pub fn read(data_dir: &Path) -> Result<String, ApiError> {
    let path = data_dir.join(FILE_NAME);
    std::fs::read_to_string(&path)
        .map(|token| token.trim().to_owned())
        .map_err(|source| ApiError::Io { path, source })
}
