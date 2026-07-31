use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;

pub fn read_text(
    inline: Option<String>,
    file: Option<&Path>,
    limit: usize,
) -> Result<String, InputError> {
    if let Some(inline) = inline {
        return validate_size(inline, limit);
    }
    let mut bytes = Vec::new();
    match file {
        Some(path) => {
            let source = File::open(path).map_err(|source| InputError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
            source
                .take(limit.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|source| InputError::ReadFile {
                    path: path.to_path_buf(),
                    source,
                })?;
        }
        None => {
            io::stdin()
                .lock()
                .take(limit.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
                .map_err(InputError::ReadStdin)?;
        }
    }
    if bytes.len() > limit {
        return Err(InputError::TooLarge { limit });
    }
    String::from_utf8(bytes).map_err(InputError::Utf8)
}

fn validate_size(value: String, limit: usize) -> Result<String, InputError> {
    if value.len() > limit {
        Err(InputError::TooLarge { limit })
    } else {
        Ok(value)
    }
}

#[derive(Debug, Error)]
pub enum InputError {
    #[error("failed to read {path}: {source}")]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("failed to read stdin: {0}")]
    ReadStdin(io::Error),
    #[error("input exceeds the {limit}-byte request limit")]
    TooLarge { limit: usize },
    #[error("input is not valid UTF-8: {0}")]
    Utf8(std::string::FromUtf8Error),
}

#[cfg(test)]
mod tests {
    use super::read_text;

    #[test]
    fn test_read_text_rejects_oversized_inline_input() {
        assert!(read_text(Some("four".into()), None, 3).is_err());
    }

    #[test]
    fn test_read_text_accepts_input_at_limit() {
        assert_eq!(
            read_text(Some("four".into()), None, 4).expect("input at limit"),
            "four"
        );
    }
}
