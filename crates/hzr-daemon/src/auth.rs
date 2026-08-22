use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fs2::FileExt;
use hzr_protocol::ErrorResponse;
use uuid::Uuid;

#[derive(Clone)]
pub struct AuthToken(Arc<str>);

impl AuthToken {
    pub fn new(value: String) -> Result<Self, std::io::Error> {
        if value.len() < 64 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "daemon token must contain at least 64 characters",
            ));
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    fn accepts(&self, value: &str) -> bool {
        value
            .strip_prefix("Bearer ")
            .is_some_and(|candidate| constant_time_eq(candidate.as_bytes(), self.0.as_bytes()))
    }
}

impl std::fmt::Debug for AuthToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AuthToken([redacted])")
    }
}

pub fn load_or_create_token(data_root: &Path) -> Result<(AuthToken, PathBuf), std::io::Error> {
    let runtime = data_root.join("runtime");
    fs::create_dir_all(&runtime)?;
    // The daemon token lives here; create_dir_all alone leaves the directory at
    // the process umask even though the token file itself is 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let _ = fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700));
    }
    let token_path = runtime.join("hzrd.token");
    let lock_path = runtime.join("hzrd.token.lock");
    let mut lock_options = OpenOptions::new();
    lock_options
        .create(true)
        .truncate(false)
        .read(true)
        .write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        lock_options.mode(0o600);
    }
    let lock = lock_options.open(&lock_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    lock.lock_exclusive()?;
    let result = load_or_create_locked(&token_path);
    FileExt::unlock(&lock)?;
    result.map(|value| (value, token_path))
}

fn load_or_create_locked(path: &Path) -> Result<AuthToken, std::io::Error> {
    reject_symlink(path)?;
    let generated = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    match create_secret(path) {
        Ok(mut file) => {
            file.write_all(generated.as_bytes())?;
            file.sync_all()?;
            AuthToken::new(generated)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            secure_existing(path)?;
            let mut value = String::new();
            File::open(path)?.read_to_string(&mut value)?;
            AuthToken::new(value.trim().to_owned())
        }
        Err(error) => Err(error),
    }
}

fn reject_symlink(path: &Path) -> Result<(), std::io::Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("refusing symlinked daemon token at {}", path.display()),
        )),
        Ok(_) | Err(_) => Ok(()),
    }
}

#[cfg(unix)]
fn create_secret(path: &Path) -> Result<File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_secret(path: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
fn secure_existing(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn secure_existing(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

pub async fn authorize(
    State(token): State<AuthToken>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| token.accepts(value));
    if authorized {
        return next.run(request).await;
    }

    let payload = ErrorResponse {
        trace_id: None,
        code: "unauthorized".into(),
        message: "valid HZR bearer token required".into(),
        recoverable: true,
        details: serde_json::Value::Null,
    };
    (StatusCode::UNAUTHORIZED, Json(payload)).into_response()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::load_or_create_token;

    #[test]
    fn test_token_is_stable_and_not_short() {
        let directory = tempdir().expect("temporary directory");
        let (first, path) = load_or_create_token(directory.path()).expect("first token");
        let (second, _) = load_or_create_token(directory.path()).expect("second token");

        assert_eq!(first.expose(), second.expose());
        assert!(first.expose().len() >= 64);
        assert!(path.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn test_token_lock_is_created_and_repaired_with_private_permissions() {
        let directory = tempdir().expect("temporary directory");
        let lock_path = directory.path().join("runtime/hzrd.token.lock");

        load_or_create_token(directory.path()).expect("token creation");

        let created_mode = fs::metadata(&lock_path)
            .expect("created token lock metadata")
            .permissions()
            .mode();
        assert_eq!(created_mode & 0o777, 0o600);

        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o666))
            .expect("relax token lock permissions");

        load_or_create_token(directory.path()).expect("existing token load");

        let repaired_mode = fs::metadata(lock_path)
            .expect("repaired token lock metadata")
            .permissions()
            .mode();
        assert_eq!(repaired_mode & 0o777, 0o600);
    }
}
