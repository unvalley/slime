//! Shared privacy and file-integrity boundary for offline private-data tools.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

const MAX_TOTAL_INPUT_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_TOKENS_PER_LINE: usize = 512;
pub(crate) const MAX_TOKEN_CHARACTERS: usize = 128;

static TEMPORARY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Token {
    pub(crate) surface: String,
    pub(crate) reading: String,
}

pub(crate) fn ignorable_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with(";;")
}

pub(crate) fn parse_annotated_line(line: &str, line_number: usize) -> Result<Vec<Token>, String> {
    if line.len() > MAX_LINE_BYTES {
        return Err(format!(
            "annotated corpus line {line_number} exceeds the byte limit"
        ));
    }
    let mut tokens = Vec::new();
    for encoded in line.split_whitespace() {
        let Some((surface, reading)) = encoded.rsplit_once('/') else {
            return Err(format!(
                "annotated corpus line {line_number} has a malformed token"
            ));
        };
        if !valid_token_field(surface) || !valid_token_field(reading) {
            return Err(format!(
                "annotated corpus line {line_number} has an invalid token"
            ));
        }
        tokens.push(Token {
            surface: surface.to_owned(),
            reading: reading.to_owned(),
        });
        if tokens.len() > MAX_TOKENS_PER_LINE {
            return Err(format!(
                "annotated corpus line {line_number} exceeds the token limit"
            ));
        }
    }
    if tokens.is_empty() {
        return Err(format!("annotated corpus line {line_number} has no tokens"));
    }
    Ok(tokens)
}

pub(crate) fn valid_token_field(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_TOKEN_CHARACTERS
        && !value.chars().any(char::is_control)
}

pub(crate) fn normalize_phonetic_reading(reading: &str) -> Option<String> {
    let normalized: String = reading
        .chars()
        .map(|character| match character {
            'ァ'..='ヶ' | 'ヽ' | 'ヾ' => {
                char::from_u32(u32::from(character) - 0x60).expect("valid hiragana scalar")
            }
            _ => character,
        })
        .collect();
    (!normalized.is_empty()
        && normalized
            .chars()
            .all(|character| matches!(character, 'ぁ'..='ゖ' | 'ゝ' | 'ゞ' | 'ー')))
    .then_some(normalized)
}

pub(crate) fn hash_tokens(tokens: &[Token]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for token in tokens {
        hasher.update(token.surface.as_bytes());
        hasher.update([0]);
        hasher.update(
            normalize_phonetic_reading(&token.reading)
                .unwrap_or_else(|| token.reading.clone())
                .as_bytes(),
        );
        hasher.update([0xff]);
    }
    hasher.finalize().into()
}

pub(crate) fn read_private_input(
    path: &Path,
    kind: &str,
    total_bytes: &mut u64,
) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| format!("cannot inspect {kind}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{kind} must be a regular file"));
    }
    *total_bytes = total_bytes
        .checked_add(metadata.len())
        .ok_or_else(|| "input byte total overflowed".to_owned())?;
    if *total_bytes > MAX_TOTAL_INPUT_BYTES {
        return Err(format!(
            "inputs exceed the {MAX_TOTAL_INPUT_BYTES} byte limit"
        ));
    }
    let bytes = fs::read(path).map_err(|_| format!("cannot read {kind}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len() {
        return Err(format!("{kind} changed while it was being read"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{kind} must be UTF-8"))
}

pub(crate) fn validate_tsv_output(path: &Path) -> Result<(), String> {
    if path.extension().and_then(std::ffi::OsStr::to_str) != Some("tsv") {
        return Err("output must use the .tsv extension".to_owned());
    }
    match fs::symlink_metadata(path) {
        Ok(_) => Err("output already exists".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("cannot inspect output".to_owned()),
    }
}

pub(crate) fn write_new_atomic(
    path: &Path,
    bytes: &[u8],
    temporary_prefix: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let metadata =
        fs::symlink_metadata(parent).map_err(|_| "cannot inspect output directory".to_owned())?;
    if !metadata.file_type().is_dir() {
        return Err("output parent must be a directory".to_owned());
    }
    for _ in 0..32 {
        let counter = TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{temporary_prefix}-{}-{counter}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&temporary) {
            Ok(mut file) => {
                if file
                    .write_all(bytes)
                    .and_then(|()| file.sync_all())
                    .is_err()
                {
                    let _ = fs::remove_file(&temporary);
                    return Err("cannot write output".to_owned());
                }
                drop(file);
                let published = fs::hard_link(&temporary, path);
                let _ = fs::remove_file(&temporary);
                match published {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Err("output already exists".to_owned());
                    }
                    Err(_) => return Err("cannot publish output".to_owned()),
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("cannot create output".to_owned()),
        }
    }
    Err("cannot allocate a temporary output file".to_owned())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(all(test, unix))]
mod tests {
    use super::write_new_atomic;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn generated_output_is_private() {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "slime-private-generation-test-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let output = directory.join("output.tsv");
        write_new_atomic(&output, b"private\n", "slime-private-test").unwrap();
        let mode = fs::metadata(&output).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
        fs::remove_dir_all(directory).unwrap();
    }
}
