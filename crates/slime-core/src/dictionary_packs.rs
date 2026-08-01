use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use slime_converter::DictionaryLayer;

use crate::domain_dictionaries::{MAX_DOMAIN_WORD_COST, MIN_DOMAIN_WORD_COST, supplemental_entry};

const PACK_DIRECTORY_NAME: &str = "dictionary-packs";
const PACK_FILE_EXTENSION: &str = "slime-dict";
const PACK_HEADER_V1: &str = "# slime-dictionary-pack-v1";
const PACK_HEADER_V2: &str = "# slime-dictionary-pack-v2";
const PACK_ENTRIES_MARKER: &str = "# entries";
const DEFAULT_WORD_COST: i32 = 500;
const MAX_PACKS: usize = 64;
const MAX_PACK_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES_PER_PACK: usize = 250_000;
const MAX_LINE_BYTES: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryPackInfo {
    pub format_version: u8,
    pub id: String,
    pub name: String,
    pub version: String,
    pub license: String,
    pub minimum_slime_version: Option<String>,
    pub published_at: Option<String>,
    pub provenance: Option<String>,
    pub entries_sha256: Option<String>,
    pub entry_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryPackWord {
    pub reading: String,
    pub surface: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryPackLoadError {
    pub file: String,
    pub message: String,
}

/// Validates one complete UTF-8 dictionary pack and returns its public metadata.
///
/// This does not install the pack or retain its entries.
///
/// # Errors
///
/// Returns a format or validation error when metadata or entries violate the
/// versioned pack contract.
pub fn validate_dictionary_pack(source: &str) -> Result<DictionaryPackInfo, String> {
    parse_pack(source).map(|pack| pack.info)
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DictionaryPackStore {
    packs: Vec<DictionaryPack>,
    errors: Vec<DictionaryPackLoadError>,
}

#[derive(Clone, Debug)]
struct DictionaryPack {
    info: DictionaryPackInfo,
    entries: Vec<PackEntry>,
}

#[derive(Clone, Debug)]
struct PackEntry {
    reading: String,
    surface: String,
    word_cost: i32,
}

#[derive(Default)]
struct PackMetadata {
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    license: Option<String>,
    minimum_slime_version: Option<String>,
    published_at: Option<String>,
    provenance: Option<String>,
    entries_sha256: Option<String>,
}

impl PackMetadata {
    fn set(&mut self, key: &str, value: &str, line_number: usize) -> Result<(), String> {
        match key {
            "id" => set_once(&mut self.id, value, key, line_number),
            "name" => set_once(&mut self.name, value, key, line_number),
            "version" => set_once(&mut self.version, value, key, line_number),
            "license" => set_once(&mut self.license, value, key, line_number),
            "minimum-slime-version" => {
                set_once(&mut self.minimum_slime_version, value, key, line_number)
            }
            "published-at" => set_once(&mut self.published_at, value, key, line_number),
            "provenance" => set_once(&mut self.provenance, value, key, line_number),
            "entries-sha256" => set_once(&mut self.entries_sha256, value, key, line_number),
            _ => Err(format!("line {line_number} has unknown metadata {key:?}")),
        }
    }
}

impl DictionaryPackStore {
    pub(crate) fn load(data_directory: Option<&Path>) -> Self {
        let Some(data_directory) = data_directory else {
            return Self::default();
        };
        let directory = data_directory.join(PACK_DIRECTORY_NAME);
        let paths = match pack_paths(&directory) {
            Ok(paths) => paths,
            Err(message) => {
                return Self {
                    packs: Vec::new(),
                    errors: vec![DictionaryPackLoadError {
                        file: directory.display().to_string(),
                        message,
                    }],
                };
            }
        };

        let mut packs = Vec::with_capacity(paths.len().min(MAX_PACKS));
        let mut errors = Vec::new();
        let mut ids = HashSet::new();
        for path in paths.into_iter().take(MAX_PACKS) {
            match load_pack(&path) {
                Ok(pack) if ids.insert(pack.info.id.clone()) => packs.push(pack),
                Ok(pack) => errors.push(load_error(
                    &path,
                    format!("duplicate dictionary pack id {:?}", pack.info.id),
                )),
                Err(message) => errors.push(load_error(&path, message)),
            }
        }

        Self { packs, errors }
    }

    pub(crate) fn layers(&self) -> Vec<DictionaryLayer> {
        self.packs
            .iter()
            .map(|pack| {
                let entries = pack
                    .entries
                    .iter()
                    .map(|entry| {
                        supplemental_entry(&entry.reading, &entry.surface, entry.word_cost)
                    })
                    .collect();
                DictionaryLayer::new(&pack.info.id, &pack.info.name, entries)
            })
            .collect()
    }

    pub(crate) fn words(&self) -> impl Iterator<Item = (&str, &str)> {
        self.packs.iter().flat_map(|pack| {
            pack.entries
                .iter()
                .map(|entry| (entry.reading.as_str(), entry.surface.as_str()))
        })
    }

    pub(crate) fn infos(&self) -> impl Iterator<Item = &DictionaryPackInfo> {
        self.packs.iter().map(|pack| &pack.info)
    }

    pub(crate) fn pack_words(&self, id: &str) -> Option<Vec<DictionaryPackWord>> {
        let pack = self.packs.iter().find(|pack| pack.info.id == id)?;
        Some(
            pack.entries
                .iter()
                .map(|entry| DictionaryPackWord {
                    reading: entry.reading.clone(),
                    surface: entry.surface.clone(),
                })
                .collect(),
        )
    }

    pub(crate) fn errors(&self) -> &[DictionaryPackLoadError] {
        &self.errors
    }
}

fn pack_paths(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read dictionary pack directory: {error}")),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) == Some(PACK_FILE_EXTENSION) {
            paths.push(path);
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

fn load_pack(path: &Path) -> Result<DictionaryPack, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("cannot inspect file: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err("dictionary pack must be a regular file".to_owned());
    }
    if metadata.len() > MAX_PACK_BYTES {
        return Err(format!(
            "dictionary pack exceeds the {MAX_PACK_BYTES} byte limit"
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("cannot read file: {error}"))?;
    let source =
        std::str::from_utf8(&bytes).map_err(|_| "dictionary pack is not UTF-8".to_owned())?;
    parse_pack(source)
}

fn parse_pack(source: &str) -> Result<DictionaryPack, String> {
    let mut lines = source.lines().enumerate();
    let Some((_, header)) = lines.next() else {
        return Err("dictionary pack is empty".to_owned());
    };
    let format_version = match header {
        PACK_HEADER_V1 => 1,
        PACK_HEADER_V2 => 2,
        _ => {
            return Err(format!(
                "first line must be {PACK_HEADER_V1:?} or {PACK_HEADER_V2:?}"
            ));
        }
    };

    let mut metadata = PackMetadata::default();
    let mut entries = Vec::new();
    let mut pairs = HashSet::new();
    let mut entries_started = format_version == 1;

    for (line_index, line) in lines {
        let line_number = line_index + 2;
        if line.len() > MAX_LINE_BYTES {
            return Err(format!("line {line_number} exceeds the byte limit"));
        }
        if line.is_empty() {
            continue;
        }
        if format_version == 2 && line == PACK_ENTRIES_MARKER {
            if entries_started {
                return Err(format!("line {line_number} duplicates the entries marker"));
            }
            entries_started = true;
            continue;
        }
        if let Some(metadata_source) = line.strip_prefix("# ") {
            if entries_started && format_version == 2 {
                return Err(format!(
                    "line {line_number} has metadata after the entries marker"
                ));
            }
            let (key, value) = metadata_source
                .split_once(": ")
                .ok_or_else(|| format!("line {line_number} has malformed metadata"))?;
            metadata.set(key, value, line_number)?;
            continue;
        }
        if line.starts_with('#') {
            return Err(format!("line {line_number} has malformed metadata"));
        }
        if !entries_started {
            return Err(format!(
                "line {line_number} appears before {PACK_ENTRIES_MARKER:?}"
            ));
        }
        if entries.len() == MAX_ENTRIES_PER_PACK {
            return Err(format!(
                "dictionary pack exceeds the {MAX_ENTRIES_PER_PACK} entry limit"
            ));
        }
        let entry = parse_entry(line, line_number)?;
        if !pairs.insert((entry.reading.clone(), entry.surface.clone())) {
            return Err(format!("line {line_number} duplicates an earlier entry"));
        }
        entries.push(entry);
    }

    let id = required(metadata.id, "id")?;
    let name = required(metadata.name, "name")?;
    let version = required(metadata.version, "version")?;
    let license = required(metadata.license, "license")?;
    validate_metadata(&id, &name, &version, &license)?;
    let v2 = validate_v2_metadata(
        source,
        format_version,
        metadata.minimum_slime_version,
        metadata.published_at,
        metadata.provenance,
        metadata.entries_sha256,
    )?;
    if entries.is_empty() {
        return Err("dictionary pack has no entries".to_owned());
    }

    Ok(DictionaryPack {
        info: DictionaryPackInfo {
            format_version,
            id,
            name,
            version,
            license,
            minimum_slime_version: v2.minimum_slime_version,
            published_at: v2.published_at,
            provenance: v2.provenance,
            entries_sha256: v2.entries_sha256,
            entry_count: entries.len(),
        },
        entries,
    })
}

fn parse_entry(line: &str, line_number: usize) -> Result<PackEntry, String> {
    let mut columns = line.split('\t');
    let reading = columns.next().unwrap_or_default();
    let surface = columns.next().unwrap_or_default();
    let word_cost = columns.next().map_or(Ok(DEFAULT_WORD_COST), |value| {
        value
            .parse()
            .map_err(|_| format!("line {line_number} has a non-numeric cost"))
    })?;
    if columns.next().is_some() {
        return Err(format!("line {line_number} has too many columns"));
    }
    if reading.is_empty() || surface.is_empty() {
        return Err(format!(
            "line {line_number} has an empty reading or surface"
        ));
    }
    if !reading
        .chars()
        .all(|character| matches!(character, '\u{3041}'..='\u{3096}' | 'ー'))
    {
        return Err(format!("line {line_number} reading must be hiragana"));
    }
    if surface.chars().any(char::is_control) || surface.chars().count() > 128 {
        return Err(format!("line {line_number} has an invalid surface"));
    }
    if !(MIN_DOMAIN_WORD_COST..=MAX_DOMAIN_WORD_COST).contains(&word_cost) {
        return Err(format!(
            "line {line_number} cost must be between {MIN_DOMAIN_WORD_COST} and \
             {MAX_DOMAIN_WORD_COST}"
        ));
    }
    Ok(PackEntry {
        reading: reading.to_owned(),
        surface: surface.to_owned(),
        word_cost,
    })
}

fn set_once(
    field: &mut Option<String>,
    value: &str,
    key: &str,
    line_number: usize,
) -> Result<(), String> {
    if field.is_some() {
        return Err(format!("line {line_number} duplicates metadata {key:?}"));
    }
    *field = Some(value.to_owned());
    Ok(())
}

fn required(value: Option<String>, key: &str) -> Result<String, String> {
    value.ok_or_else(|| format!("dictionary pack is missing metadata {key:?}"))
}

fn validate_metadata(id: &str, name: &str, version: &str, license: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_".contains(&byte)
        })
        || !id.as_bytes()[0].is_ascii_alphanumeric()
    {
        return Err("dictionary pack id must be a lowercase ASCII identifier".to_owned());
    }
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("dictionary pack name is invalid".to_owned());
    }
    if version.is_empty()
        || version.len() > 32
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte))
    {
        return Err("dictionary pack version is invalid".to_owned());
    }
    if license.is_empty() || license.chars().count() > 64 || license.chars().any(char::is_control) {
        return Err("dictionary pack license is invalid".to_owned());
    }
    Ok(())
}

struct ValidatedV2Metadata {
    minimum_slime_version: Option<String>,
    published_at: Option<String>,
    provenance: Option<String>,
    entries_sha256: Option<String>,
}

fn validate_v2_metadata(
    source: &str,
    format_version: u8,
    minimum_slime_version: Option<String>,
    published_at: Option<String>,
    provenance: Option<String>,
    entries_sha256: Option<String>,
) -> Result<ValidatedV2Metadata, String> {
    if format_version == 1 {
        if minimum_slime_version.is_some()
            || published_at.is_some()
            || provenance.is_some()
            || entries_sha256.is_some()
        {
            return Err("v2 metadata requires the v2 pack header".to_owned());
        }
        return Ok(ValidatedV2Metadata {
            minimum_slime_version: None,
            published_at: None,
            provenance: None,
            entries_sha256: None,
        });
    }

    let minimum_slime_version = required(minimum_slime_version, "minimum-slime-version")?;
    let published_at = required(published_at, "published-at")?;
    let provenance = required(provenance, "provenance")?;
    let entries_sha256 = required(entries_sha256, "entries-sha256")?;
    let minimum = parse_semantic_version(&minimum_slime_version)
        .ok_or_else(|| "minimum-slime-version must use MAJOR.MINOR.PATCH".to_owned())?;
    let current = parse_semantic_version(env!("CARGO_PKG_VERSION"))
        .expect("workspace package version is semantic");
    if minimum > current {
        return Err(format!(
            "dictionary pack requires Slime {minimum_slime_version} or newer; current version is {}",
            env!("CARGO_PKG_VERSION")
        ));
    }
    if !is_iso_date(&published_at) {
        return Err("published-at must use YYYY-MM-DD".to_owned());
    }
    if provenance.is_empty()
        || provenance.chars().count() > 256
        || provenance.chars().any(char::is_control)
    {
        return Err("dictionary pack provenance is invalid".to_owned());
    }
    if entries_sha256.len() != 64 || !entries_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("entries-sha256 must be a 64-character hexadecimal digest".to_owned());
    }
    let marker = format!("{PACK_ENTRIES_MARKER}\n");
    let (_, entries_source) = source
        .split_once(&marker)
        .ok_or_else(|| format!("v2 dictionary pack is missing {PACK_ENTRIES_MARKER:?}"))?;
    let actual_sha256 = sha256_hex(entries_source.as_bytes());
    if !entries_sha256.eq_ignore_ascii_case(&actual_sha256) {
        return Err(format!(
            "entries SHA-256 mismatch: expected {entries_sha256}, got {actual_sha256}"
        ));
    }

    Ok(ValidatedV2Metadata {
        minimum_slime_version: Some(minimum_slime_version),
        published_at: Some(published_at),
        provenance: Some(provenance),
        entries_sha256: Some(actual_sha256),
    })
}

fn parse_semantic_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut components = value.split('.');
    let major = components.next()?.parse().ok()?;
    let minor = components.next()?.parse().ok()?;
    let patch = components.next()?.parse().ok()?;
    if components.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let year = value[0..4].parse::<u16>().ok();
    let month = value[5..7].parse::<u8>().ok();
    let day = value[8..10].parse::<u8>().ok();
    year.is_some_and(|year| year >= 2000)
        && month.is_some_and(|month| (1..=12).contains(&month))
        && day.is_some_and(|day| (1..=31).contains(&day))
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn load_error(path: &Path, message: String) -> DictionaryPackLoadError {
    DictionaryPackLoadError {
        file: path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into(),
        ),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DictionaryPackStore, PACK_DIRECTORY_NAME, parse_pack, sha256_hex, validate_dictionary_pack,
    };
    use std::fs;

    const VALID_PACK: &str = "\
# slime-dictionary-pack-v1
# id: sample-pro
# name: サンプル Pro
# version: 2026.07.1
# license: Proprietary
すらいむぷろ\tSlime Pro
こまわり\t専門小回り\t6000
";

    #[test]
    fn parses_versioned_pack_metadata_and_entries() {
        let pack = parse_pack(VALID_PACK).unwrap();
        assert_eq!(pack.info.format_version, 1);
        assert_eq!(pack.info.id, "sample-pro");
        assert_eq!(pack.info.name, "サンプル Pro");
        assert_eq!(pack.info.version, "2026.07.1");
        assert_eq!(pack.info.license, "Proprietary");
        assert_eq!(pack.info.entry_count, 2);
        assert_eq!(pack.info.minimum_slime_version, None);
        assert_eq!(pack.entries[1].word_cost, 6000);
        assert_eq!(validate_dictionary_pack(VALID_PACK).unwrap(), pack.info);
    }

    #[test]
    fn validates_v2_compatibility_provenance_and_entries_digest() {
        let entries = "すらいむぷろ\tSlime Pro\nこまわり\t専門小回り\t6000\n";
        let digest = sha256_hex(entries.as_bytes());
        let source = format!(
            "# slime-dictionary-pack-v2\n\
             # id: sample-pro\n\
             # name: サンプル Pro\n\
             # version: 2026.08.1\n\
             # license: Proprietary\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-01\n\
             # provenance: unvalley/context-packs/sample-pro\n\
             # entries-sha256: {digest}\n\
             # entries\n\
             {entries}"
        );
        let info = validate_dictionary_pack(&source).unwrap();
        assert_eq!(info.format_version, 2);
        assert_eq!(info.minimum_slime_version.as_deref(), Some("0.1.0"));
        assert_eq!(info.published_at.as_deref(), Some("2026-08-01"));
        assert_eq!(
            info.provenance.as_deref(),
            Some("unvalley/context-packs/sample-pro")
        );
        assert_eq!(info.entries_sha256.as_deref(), Some(digest.as_str()));

        assert!(validate_dictionary_pack(&source.replace("Slime Pro", "Slime Plus")).is_err());
        assert!(
            validate_dictionary_pack(&source.replace(
                "# minimum-slime-version: 0.1.0",
                "# minimum-slime-version: 99.0.0"
            ))
            .is_err()
        );
    }

    #[test]
    fn rejects_malformed_or_duplicate_data() {
        assert!(parse_pack("すらいむぷろ\tSlime Pro\n").is_err());
        assert!(parse_pack(&VALID_PACK.replace("sample-pro", "Sample Pack")).is_err());
        assert!(parse_pack(&format!("{VALID_PACK}こまわり\t専門小回り\t6000\n")).is_err());
        assert!(parse_pack(&VALID_PACK.replace("6000", "99")).is_err());
    }

    #[test]
    fn store_loads_valid_packs_and_reports_invalid_siblings() {
        let directory = std::env::temp_dir().join(format!(
            "slime-dictionary-packs-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let pack_directory = directory.join(PACK_DIRECTORY_NAME);
        fs::create_dir_all(&pack_directory).unwrap();
        fs::write(pack_directory.join("sample.slime-dict"), VALID_PACK).unwrap();
        fs::write(pack_directory.join("broken.slime-dict"), "not a pack\n").unwrap();
        fs::write(pack_directory.join("ignored.txt"), "not a pack\n").unwrap();

        let store = DictionaryPackStore::load(Some(&directory));
        let infos: Vec<_> = store.infos().collect();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, "sample-pro");
        assert_eq!(store.words().count(), 2);
        assert_eq!(store.errors().len(), 1);
        assert_eq!(store.errors()[0].file, "broken.slime-dict");

        fs::remove_dir_all(directory).unwrap();
    }
}
