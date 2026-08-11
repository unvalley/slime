use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use compact_str::CompactString;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use slime_converter::{Dictionary, DictionaryLayer};

use crate::domain_dictionaries::{MAX_DOMAIN_WORD_COST, MIN_DOMAIN_WORD_COST, supplemental_entry};

const PACK_DIRECTORY_NAME: &str = "dictionary-packs";
const PACK_FILE_EXTENSION: &str = "slime-dict";
const PACK_HEADER_V1: &str = "# slime-dictionary-pack-v1";
const PACK_HEADER_V2: &str = "# slime-dictionary-pack-v2";
const PACK_HEADER_V3: &str = "# slime-dictionary-pack-v3";
const PACK_HEADER_V4: &str = "# slime-dictionary-pack-v4";
const PACK_HEADER_V5: &str = "# slime-dictionary-pack-v5";
const PACK_ENTRIES_MARKER: &str = "# entries";
const PACK_CONTEXT_RULES_MARKER: &str = "# context-rules";
const PACK_SIGNATURE_HEADER_V1: &str = "# slime-dictionary-signature-v1";
const PACK_SIGNATURE_KEY_PREFIX: &str = "# key-id: ";
const PACK_SIGNATURE_VALUE_PREFIX: &str = "# signature-ed25519: ";
const DEFAULT_WORD_COST: i32 = 500;
const MAX_PACKS: usize = 64;
const MAX_PACK_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES_PER_PACK: usize = 250_000;
const MAX_CONTEXT_RULES_PER_PACK: usize = 100_000;
const MAX_CONTEXT_SURFACE_CHARACTERS: usize = 128;
const MAX_LINE_BYTES: usize = 4_096;
const MAX_SIGNATURE_FILE_BYTES: u64 = 1_024;
const MAX_SIGNATURE_KEY_ID_BYTES: usize = 64;
const MAX_VERIFICATION_KEYS: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryPackVerificationKey {
    id: String,
    ed25519_public_key: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryPackVersionFloor {
    id: String,
    minimum_version: String,
    parsed_minimum_version: (u64, u64, u64),
}

impl DictionaryPackVersionFloor {
    /// Creates a minimum accepted version for one signed dictionary pack ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the ID is invalid or the version does not use
    /// `MAJOR.MINOR.PATCH` with unsigned integer components.
    pub fn new(id: impl Into<String>, minimum_version: impl Into<String>) -> Result<Self, String> {
        let id = id.into();
        validate_pack_id(&id)?;
        let minimum_version = minimum_version.into();
        if minimum_version.len() > 32 {
            return Err("dictionary pack version floor exceeds the metadata limit".to_owned());
        }
        let parsed_minimum_version = parse_semantic_version(&minimum_version)
            .ok_or_else(|| "dictionary pack version floor must use MAJOR.MINOR.PATCH".to_owned())?;
        Ok(Self {
            id,
            minimum_version,
            parsed_minimum_version,
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn minimum_version(&self) -> &str {
        &self.minimum_version
    }
}

impl DictionaryPackVerificationKey {
    /// Creates a named Ed25519 verification key for signed dictionary packs.
    ///
    /// # Errors
    ///
    /// Returns an error when the identifier or public key is invalid.
    pub fn new(id: impl Into<String>, ed25519_public_key: [u8; 32]) -> Result<Self, String> {
        let id = id.into();
        validate_signature_key_id(&id)?;
        let key = VerifyingKey::from_bytes(&ed25519_public_key)
            .map_err(|_| "dictionary pack verification key is invalid".to_owned())?;
        if key.is_weak() {
            return Err("dictionary pack verification key is weak".to_owned());
        }
        Ok(Self {
            id,
            ed25519_public_key,
        })
    }

    /// Creates a verification key from its canonical lowercase hexadecimal
    /// encoding.
    ///
    /// # Errors
    ///
    /// Returns an error when the identifier or 32-byte public key is invalid.
    pub fn from_lower_hex(id: impl Into<String>, encoded_public_key: &str) -> Result<Self, String> {
        let public_key = decode_lower_hex::<32>(encoded_public_key).ok_or_else(|| {
            "dictionary pack verification key must be 64 lowercase hex digits".to_owned()
        })?;
        Self::new(id, public_key)
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn ed25519_public_key(&self) -> &[u8; 32] {
        &self.ed25519_public_key
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DictionaryPackTrust {
    require_signatures: bool,
    keys: Vec<DictionaryPackVerificationKey>,
    version_floors: Vec<DictionaryPackVersionFloor>,
}

impl DictionaryPackTrust {
    /// Requires every installed dictionary pack to have a valid signature from
    /// one of the supplied keys. Multiple keys permit an explicit rotation
    /// window; removing a key revokes it for subsequent loads.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, duplicate, or oversized key set.
    pub fn signed_only(keys: Vec<DictionaryPackVerificationKey>) -> Result<Self, String> {
        Self::signed_only_with_version_floors(keys, Vec::new())
    }

    /// Requires valid signatures and restricts packs to the supplied IDs at or
    /// above their minimum accepted versions. The existing [`Self::signed_only`]
    /// constructor remains available when an ID allowlist is not desired.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, duplicate, or oversized key set,
    /// duplicate version floor IDs, or more floors than the loader can accept
    /// packs.
    pub fn signed_only_with_version_floors(
        mut keys: Vec<DictionaryPackVerificationKey>,
        mut version_floors: Vec<DictionaryPackVersionFloor>,
    ) -> Result<Self, String> {
        if keys.is_empty() {
            return Err("signed dictionary pack policy requires at least one key".to_owned());
        }
        if keys.len() > MAX_VERIFICATION_KEYS {
            return Err(format!(
                "signed dictionary pack policy exceeds the {MAX_VERIFICATION_KEYS} key limit"
            ));
        }
        keys.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        if keys.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err("dictionary pack verification key ids must be unique".to_owned());
        }
        if version_floors.len() > MAX_PACKS {
            return Err(format!(
                "dictionary pack version policy exceeds the {MAX_PACKS} pack limit"
            ));
        }
        version_floors.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        if version_floors
            .windows(2)
            .any(|pair| pair[0].id == pair[1].id)
        {
            return Err("dictionary pack version floor ids must be unique".to_owned());
        }
        Ok(Self {
            require_signatures: true,
            keys,
            version_floors,
        })
    }

    #[must_use]
    pub const fn requires_signatures(&self) -> bool {
        self.require_signatures
    }

    fn validate_version(&self, info: &DictionaryPackInfo) -> Result<(), String> {
        if self.version_floors.is_empty() {
            return Ok(());
        }
        let Ok(index) = self
            .version_floors
            .binary_search_by(|floor| floor.id.as_str().cmp(&info.id))
        else {
            return Err("dictionary pack id is not allowed by rollback policy".to_owned());
        };
        let floor = &self.version_floors[index];
        let version = parse_semantic_version(&info.version).ok_or_else(|| {
            "dictionary pack version is incompatible with rollback policy".to_owned()
        })?;
        if version < floor.parsed_minimum_version {
            return Err("dictionary pack version is below the configured minimum".to_owned());
        }
        Ok(())
    }
}

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
    pub payload_sha256: Option<String>,
    pub pack_sha256: String,
    pub entry_count: usize,
    pub context_rule_count: usize,
    pub candidate_mode: DictionaryPackCandidateMode,
}

/// Controls when entries from an installed dictionary pack join conversion.
///
/// Model-rescore-only packs are invisible to ordinary conversion. Their
/// entries join the candidate pool only after an optional local scorer is
/// ready. Explicit-search-only packs remain invisible until the user reaches
/// the end of the ordinary candidate list and asks for more alternatives.
/// Both modes keep large supplemental vocabularies away from the base winner.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DictionaryPackCandidateMode {
    #[default]
    Standard,
    ModelRescoreOnly,
    ExplicitSearchOnly,
}

impl DictionaryPackCandidateMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::ModelRescoreOnly => "model-rescore-only",
            Self::ExplicitSearchOnly => "explicit-search-only",
        }
    }
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
    context_rules: Vec<PackContextRule>,
    errors: Vec<DictionaryPackLoadError>,
}

#[derive(Clone, Debug)]
struct DictionaryPack {
    info: DictionaryPackInfo,
    entries: Vec<PackEntry>,
    explicit_search_index: Vec<u32>,
    context_rules: Vec<PackContextRule>,
}

#[derive(Clone, Debug)]
struct PackEntry {
    reading: CompactString,
    surface: CompactString,
    word_cost: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PackContextRule {
    previous_surface: String,
    reading: String,
    surface: String,
    priority: u16,
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
    payload_sha256: Option<String>,
    candidate_mode: Option<String>,
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
            "payload-sha256" => set_once(&mut self.payload_sha256, value, key, line_number),
            "candidate-mode" => set_once(&mut self.candidate_mode, value, key, line_number),
            _ => Err(format!("line {line_number} has unknown metadata {key:?}")),
        }
    }
}

impl DictionaryPackStore {
    pub(crate) fn load_with_trust(
        data_directory: Option<&Path>,
        trust: &DictionaryPackTrust,
    ) -> Self {
        let Some(data_directory) = data_directory else {
            return Self::default();
        };
        let directory = data_directory.join(PACK_DIRECTORY_NAME);
        let paths = match pack_paths(&directory) {
            Ok(paths) => paths,
            Err(message) => {
                return Self {
                    packs: Vec::new(),
                    context_rules: Vec::new(),
                    errors: vec![DictionaryPackLoadError {
                        file: directory.display().to_string(),
                        message,
                    }],
                };
            }
        };

        let exceeds_pack_limit = paths.len() > MAX_PACKS;
        let mut packs = Vec::with_capacity(paths.len().min(MAX_PACKS));
        let mut errors = Vec::new();
        if exceeds_pack_limit {
            errors.push(DictionaryPackLoadError {
                file: directory.display().to_string(),
                message: format!("dictionary pack directory exceeds the {MAX_PACKS} pack limit"),
            });
        }
        let mut ids = HashSet::new();
        for path in paths.into_iter().take(MAX_PACKS) {
            match load_pack(&path, trust) {
                Ok(pack) if ids.insert(pack.info.id.clone()) => packs.push(pack),
                Ok(pack) => errors.push(load_error(
                    &path,
                    format!("duplicate dictionary pack id {:?}", pack.info.id),
                )),
                Err(message) => errors.push(load_error(&path, message)),
            }
        }

        let context_rules = merge_context_rules(&packs);
        Self {
            packs,
            context_rules,
            errors,
        }
    }

    pub(crate) fn layers(&self) -> Vec<DictionaryLayer> {
        self.packs
            .iter()
            .filter(|pack| pack.info.candidate_mode == DictionaryPackCandidateMode::Standard)
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

    pub(crate) fn model_rescore_layers(&self, base: &Dictionary) -> Vec<DictionaryLayer> {
        self.packs
            .iter()
            .filter(|pack| {
                pack.info.candidate_mode == DictionaryPackCandidateMode::ModelRescoreOnly
            })
            .map(|pack| {
                let entries = pack
                    .entries
                    .iter()
                    .filter(|entry| !base.has_exact_entry(&entry.reading, &entry.surface))
                    .map(|entry| {
                        supplemental_entry(&entry.reading, &entry.surface, entry.word_cost)
                    })
                    .collect();
                DictionaryLayer::new(&pack.info.id, &pack.info.name, entries)
            })
            .filter(|layer| layer.entry_count() != 0)
            .collect()
    }

    pub(crate) fn explicit_search_surfaces(&self, reading: &str, limit: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }
        let mut entries = Vec::new();
        for pack in self.packs.iter().filter(|pack| {
            pack.info.candidate_mode == DictionaryPackCandidateMode::ExplicitSearchOnly
        }) {
            let entry_at = |index: &u32| &pack.entries[*index as usize];
            let start = pack
                .explicit_search_index
                .partition_point(|index| entry_at(index).reading.as_str() < reading);
            let end = pack
                .explicit_search_index
                .partition_point(|index| entry_at(index).reading.as_str() <= reading);
            entries.extend(
                pack.explicit_search_index[start..end]
                    .iter()
                    .take(limit)
                    .map(entry_at),
            );
        }
        entries.sort_unstable_by(|left, right| {
            left.word_cost
                .cmp(&right.word_cost)
                .then(left.surface.cmp(&right.surface))
        });
        let mut surfaces = Vec::with_capacity(limit.min(entries.len()));
        for entry in entries {
            if !surfaces
                .iter()
                .any(|surface: &String| surface == entry.surface.as_str())
            {
                surfaces.push(entry.surface.to_string());
                if surfaces.len() == limit {
                    break;
                }
            }
        }
        surfaces
    }

    pub(crate) fn standard_words(&self) -> impl Iterator<Item = (&str, &str)> {
        self.packs
            .iter()
            .filter(|pack| pack.info.candidate_mode == DictionaryPackCandidateMode::Standard)
            .flat_map(|pack| {
                pack.entries
                    .iter()
                    .map(|entry| (entry.reading.as_str(), entry.surface.as_str()))
            })
    }

    pub(crate) fn visit_contextual_surfaces(
        &self,
        previous_surface: &str,
        reading: &str,
        mut visitor: impl FnMut(&str) -> bool,
    ) {
        let reading_start = self
            .context_rules
            .partition_point(|rule| rule.reading.as_str() < reading);
        let reading_end = self
            .context_rules
            .partition_point(|rule| rule.reading.as_str() <= reading);
        let rules = &self.context_rules[reading_start..reading_end];
        if rules.is_empty() {
            return;
        }
        let character_count = previous_surface.chars().count();
        for (character_index, (byte_index, _)) in previous_surface.char_indices().enumerate() {
            if character_count - character_index > MAX_CONTEXT_SURFACE_CHARACTERS {
                continue;
            }
            let suffix = &previous_surface[byte_index..];
            let start = rules.partition_point(|rule| rule.previous_surface.as_str() < suffix);
            let end = rules.partition_point(|rule| rule.previous_surface.as_str() <= suffix);
            for rule in &rules[start..end] {
                if !visitor(&rule.surface) {
                    return;
                }
            }
        }
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
                    reading: entry.reading.to_string(),
                    surface: entry.surface.to_string(),
                })
                .collect(),
        )
    }

    pub(crate) fn errors(&self) -> &[DictionaryPackLoadError] {
        &self.errors
    }
}

fn merge_context_rules(packs: &[DictionaryPack]) -> Vec<PackContextRule> {
    let mut rules: Vec<_> = packs
        .iter()
        .flat_map(|pack| pack.context_rules.iter().cloned())
        .collect();
    rules.sort_unstable_by(|left, right| {
        (
            &left.reading,
            &left.previous_surface,
            &left.surface,
            left.priority,
        )
            .cmp(&(
                &right.reading,
                &right.previous_surface,
                &right.surface,
                right.priority,
            ))
    });
    rules.dedup_by(|right, left| {
        right.previous_surface == left.previous_surface
            && right.reading == left.reading
            && right.surface == left.surface
    });
    rules.sort_unstable_by(|left, right| {
        (
            &left.reading,
            &left.previous_surface,
            left.priority,
            &left.surface,
        )
            .cmp(&(
                &right.reading,
                &right.previous_surface,
                right.priority,
                &right.surface,
            ))
    });
    rules
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

fn load_pack(path: &Path, trust: &DictionaryPackTrust) -> Result<DictionaryPack, String> {
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
    if trust.require_signatures {
        verify_pack_signature(path, &bytes, trust)?;
    }
    let source =
        std::str::from_utf8(&bytes).map_err(|_| "dictionary pack is not UTF-8".to_owned())?;
    let pack = parse_pack(source)?;
    trust.validate_version(&pack.info)?;
    Ok(pack)
}

fn verify_pack_signature(
    pack_path: &Path,
    pack_bytes: &[u8],
    trust: &DictionaryPackTrust,
) -> Result<(), String> {
    let signature_path = pack_signature_path(pack_path);
    let metadata = fs::symlink_metadata(&signature_path)
        .map_err(|_| "dictionary pack signature is required".to_owned())?;
    if !metadata.file_type().is_file() {
        return Err("dictionary pack signature must be a regular file".to_owned());
    }
    if metadata.len() > MAX_SIGNATURE_FILE_BYTES {
        return Err("dictionary pack signature exceeds the byte limit".to_owned());
    }
    let signature_bytes =
        fs::read(signature_path).map_err(|_| "cannot read dictionary pack signature".to_owned())?;
    let signature_source = std::str::from_utf8(&signature_bytes)
        .map_err(|_| "dictionary pack signature is not UTF-8".to_owned())?;
    let (key_id, signature) = parse_pack_signature(signature_source)?;
    let trusted_key = trust
        .keys
        .iter()
        .find(|key| key.id == key_id)
        .ok_or_else(|| "dictionary pack signature uses an unknown key".to_owned())?;
    let verifying_key = VerifyingKey::from_bytes(&trusted_key.ed25519_public_key)
        .map_err(|_| "dictionary pack verification key is invalid".to_owned())?;
    verifying_key
        .verify_strict(pack_bytes, &signature)
        .map_err(|_| "dictionary pack signature is invalid".to_owned())
}

fn pack_signature_path(pack_path: &Path) -> PathBuf {
    let mut path = pack_path.as_os_str().to_os_string();
    path.push(".sig");
    PathBuf::from(path)
}

fn parse_pack_signature(source: &str) -> Result<(&str, Signature), String> {
    let mut lines = source.lines();
    if lines.next() != Some(PACK_SIGNATURE_HEADER_V1) {
        return Err("dictionary pack signature has an invalid header".to_owned());
    }
    let key_id = lines
        .next()
        .and_then(|line| line.strip_prefix(PACK_SIGNATURE_KEY_PREFIX))
        .ok_or_else(|| "dictionary pack signature is missing its key id".to_owned())?;
    validate_signature_key_id(key_id)?;
    let encoded_signature = lines
        .next()
        .and_then(|line| line.strip_prefix(PACK_SIGNATURE_VALUE_PREFIX))
        .ok_or_else(|| "dictionary pack signature is missing its value".to_owned())?;
    if lines.any(|line| !line.is_empty()) {
        return Err("dictionary pack signature has unexpected content".to_owned());
    }
    let bytes = decode_lower_hex::<64>(encoded_signature)
        .ok_or_else(|| "dictionary pack signature has an invalid value".to_owned())?;
    Ok((key_id, Signature::from_bytes(&bytes)))
}

fn validate_signature_key_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > MAX_SIGNATURE_KEY_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("dictionary pack signature key id is invalid".to_owned());
    }
    Ok(())
}

fn decode_lower_hex<const N: usize>(source: &str) -> Option<[u8; N]> {
    if source.len() != N.checked_mul(2)? {
        return None;
    }
    let mut decoded = [0_u8; N];
    for (index, pair) in source.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_lower_hex_digit(pair[0])?;
        let low = decode_lower_hex_digit(pair[1])?;
        decoded[index] = high << 4 | low;
    }
    Some(decoded)
}

const fn decode_lower_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn parse_pack(source: &str) -> Result<DictionaryPack, String> {
    let mut lines = source.lines().enumerate();
    let Some((_, header)) = lines.next() else {
        return Err("dictionary pack is empty".to_owned());
    };
    let format_version = match header {
        PACK_HEADER_V1 => 1,
        PACK_HEADER_V2 => 2,
        PACK_HEADER_V3 => 3,
        PACK_HEADER_V4 => 4,
        PACK_HEADER_V5 => 5,
        _ => {
            return Err(format!(
                "first line must be {PACK_HEADER_V1:?}, {PACK_HEADER_V2:?}, \
                 {PACK_HEADER_V3:?}, {PACK_HEADER_V4:?}, or {PACK_HEADER_V5:?}"
            ));
        }
    };

    let mut content = PackContent::new(format_version);

    for (line_index, line) in lines {
        content.parse_line(format_version, line, line_index + 2)?;
    }
    let explicit_search_index = validate_and_index_entries(&content.entries)?;

    let PackContent {
        metadata,
        entries,
        context_rules,
        ..
    } = content;
    let PackMetadata {
        id,
        name,
        version,
        license,
        minimum_slime_version,
        published_at,
        provenance,
        entries_sha256,
        payload_sha256,
        candidate_mode,
    } = metadata;
    let id = required(id, "id")?;
    let name = required(name, "name")?;
    let version = required(version, "version")?;
    let license = required(license, "license")?;
    validate_metadata(&id, &name, &version, &license)?;
    let validated = validate_versioned_metadata(
        source,
        format_version,
        VersionedPackMetadata {
            minimum_slime_version,
            published_at,
            provenance,
            entries_sha256,
            payload_sha256,
            candidate_mode,
        },
    )?;
    if entries.is_empty() && (format_version < 3 || context_rules.is_empty()) {
        return Err("dictionary pack has no entries".to_owned());
    }
    validate_candidate_content(validated.candidate_mode, &entries, &context_rules)?;

    Ok(DictionaryPack {
        info: DictionaryPackInfo {
            format_version,
            id,
            name,
            version,
            license,
            minimum_slime_version: validated.minimum_slime_version,
            published_at: validated.published_at,
            provenance: validated.provenance,
            entries_sha256: validated.entries_sha256,
            payload_sha256: validated.payload_sha256,
            pack_sha256: sha256_hex(source.as_bytes()),
            entry_count: entries.len(),
            context_rule_count: context_rules.len(),
            candidate_mode: validated.candidate_mode,
        },
        entries,
        explicit_search_index: if validated.candidate_mode
            == DictionaryPackCandidateMode::ExplicitSearchOnly
        {
            explicit_search_index
        } else {
            Vec::new()
        },
        context_rules,
    })
}

fn validate_candidate_content(
    candidate_mode: DictionaryPackCandidateMode,
    entries: &[PackEntry],
    context_rules: &[PackContextRule],
) -> Result<(), String> {
    if !matches!(
        candidate_mode,
        DictionaryPackCandidateMode::ModelRescoreOnly
            | DictionaryPackCandidateMode::ExplicitSearchOnly
    ) {
        return Ok(());
    }
    if entries.is_empty() {
        return Err(format!(
            "{} dictionary pack has no entries",
            candidate_mode.as_str()
        ));
    }
    if !context_rules.is_empty() {
        return Err(format!(
            "{} dictionary pack cannot contain context rules",
            candidate_mode.as_str()
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackSection {
    Metadata,
    Entries,
    ContextRules,
}

struct PackContent {
    metadata: PackMetadata,
    entries: Vec<PackEntry>,
    context_rules: Vec<PackContextRule>,
    context_keys: HashSet<(String, String, String)>,
    section: PackSection,
}

impl PackContent {
    fn new(format_version: u8) -> Self {
        Self {
            metadata: PackMetadata::default(),
            entries: Vec::new(),
            context_rules: Vec::new(),
            context_keys: HashSet::new(),
            section: if format_version == 1 {
                PackSection::Entries
            } else {
                PackSection::Metadata
            },
        }
    }

    fn parse_line(
        &mut self,
        format_version: u8,
        line: &str,
        line_number: usize,
    ) -> Result<(), String> {
        if line.len() > MAX_LINE_BYTES {
            return Err(format!("line {line_number} exceeds the byte limit"));
        }
        if line.is_empty() {
            return Ok(());
        }
        if line == PACK_ENTRIES_MARKER && format_version >= 2 {
            return self.start_entries(line_number);
        }
        if line == PACK_CONTEXT_RULES_MARKER {
            return self.start_context_rules(format_version, line_number);
        }
        if let Some(metadata_source) = line.strip_prefix("# ") {
            return self.parse_metadata(format_version, metadata_source, line_number);
        }
        if line.starts_with('#') {
            return Err(format!("line {line_number} has malformed metadata"));
        }
        match self.section {
            PackSection::Metadata => Err(format!(
                "line {line_number} appears before {PACK_ENTRIES_MARKER:?}"
            )),
            PackSection::Entries => self.parse_entry_line(line, line_number),
            PackSection::ContextRules => self.parse_context_rule_line(line, line_number),
        }
    }

    fn start_entries(&mut self, line_number: usize) -> Result<(), String> {
        if self.section != PackSection::Metadata {
            return Err(format!("line {line_number} duplicates the entries marker"));
        }
        self.section = PackSection::Entries;
        Ok(())
    }

    fn start_context_rules(
        &mut self,
        format_version: u8,
        line_number: usize,
    ) -> Result<(), String> {
        if format_version < 3 {
            return Err(format!(
                "line {line_number} context rules require a v3 or newer pack header"
            ));
        }
        if self.section != PackSection::Entries {
            return Err(format!(
                "line {line_number} has a misplaced context rules marker"
            ));
        }
        self.section = PackSection::ContextRules;
        Ok(())
    }

    fn parse_metadata(
        &mut self,
        format_version: u8,
        source: &str,
        line_number: usize,
    ) -> Result<(), String> {
        if format_version >= 2 && self.section != PackSection::Metadata {
            return Err(format!(
                "line {line_number} has metadata after the entries marker"
            ));
        }
        let (key, value) = source
            .split_once(": ")
            .ok_or_else(|| format!("line {line_number} has malformed metadata"))?;
        self.metadata.set(key, value, line_number)
    }

    fn parse_entry_line(&mut self, line: &str, line_number: usize) -> Result<(), String> {
        if self.entries.len() == MAX_ENTRIES_PER_PACK {
            return Err(format!(
                "dictionary pack exceeds the {MAX_ENTRIES_PER_PACK} entry limit"
            ));
        }
        let entry = parse_entry(line, line_number)?;
        self.entries.push(entry);
        Ok(())
    }

    fn parse_context_rule_line(&mut self, line: &str, line_number: usize) -> Result<(), String> {
        if self.context_rules.len() == MAX_CONTEXT_RULES_PER_PACK {
            return Err(format!(
                "dictionary pack exceeds the {MAX_CONTEXT_RULES_PER_PACK} context rule limit"
            ));
        }
        let rule = parse_context_rule(line, line_number)?;
        let key = (
            rule.previous_surface.clone(),
            rule.reading.clone(),
            rule.surface.clone(),
        );
        if !self.context_keys.insert(key) {
            return Err(format!(
                "line {line_number} duplicates an earlier context rule"
            ));
        }
        self.context_rules.push(rule);
        Ok(())
    }
}

fn validate_and_index_entries(entries: &[PackEntry]) -> Result<Vec<u32>, String> {
    let mut indices =
        (0..u32::try_from(entries.len()).expect("pack entry limit fits u32")).collect::<Vec<_>>();
    indices.sort_unstable_by(|left, right| {
        let left = &entries[*left as usize];
        let right = &entries[*right as usize];
        (&left.reading, left.word_cost, &left.surface).cmp(&(
            &right.reading,
            right.word_cost,
            &right.surface,
        ))
    });
    let mut reading = "";
    let mut surfaces = HashSet::new();
    for index in &indices {
        let entry = &entries[*index as usize];
        if entry.reading.as_str() != reading {
            reading = entry.reading.as_str();
            surfaces.clear();
        }
        if !surfaces.insert(entry.surface.as_str()) {
            return Err("dictionary pack duplicates an entry".to_owned());
        }
    }
    Ok(indices)
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
        reading: reading.into(),
        surface: surface.into(),
        word_cost,
    })
}

fn parse_context_rule(line: &str, line_number: usize) -> Result<PackContextRule, String> {
    let mut columns = line.split('\t');
    let previous_surface = columns.next().unwrap_or_default();
    let reading = columns.next().unwrap_or_default();
    let surface = columns.next().unwrap_or_default();
    let priority = columns
        .next()
        .unwrap_or_default()
        .parse::<u16>()
        .map_err(|_| format!("line {line_number} has a non-numeric context priority"))?;
    if columns.next().is_some() {
        return Err(format!(
            "line {line_number} context rule has too many columns"
        ));
    }
    if !valid_surface(previous_surface) || !valid_surface(surface) {
        return Err(format!(
            "line {line_number} context rule has an invalid surface"
        ));
    }
    if !valid_reading(reading) {
        return Err(format!(
            "line {line_number} context rule reading must be hiragana"
        ));
    }
    Ok(PackContextRule {
        previous_surface: previous_surface.to_owned(),
        reading: reading.to_owned(),
        surface: surface.to_owned(),
        priority,
    })
}

fn valid_reading(reading: &str) -> bool {
    !reading.is_empty()
        && reading
            .chars()
            .all(|character| matches!(character, '\u{3041}'..='\u{3096}' | 'ー'))
}

fn valid_surface(surface: &str) -> bool {
    !surface.is_empty()
        && surface.chars().count() <= MAX_CONTEXT_SURFACE_CHARACTERS
        && !surface.chars().any(char::is_control)
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
    validate_pack_id(id)?;
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

fn validate_pack_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_".contains(&byte)
        })
        || !id.as_bytes()[0].is_ascii_alphanumeric()
    {
        return Err("dictionary pack id must be a lowercase ASCII identifier".to_owned());
    }
    Ok(())
}

struct ValidatedPackMetadata {
    minimum_slime_version: Option<String>,
    published_at: Option<String>,
    provenance: Option<String>,
    entries_sha256: Option<String>,
    payload_sha256: Option<String>,
    candidate_mode: DictionaryPackCandidateMode,
}

struct VersionedPackMetadata {
    minimum_slime_version: Option<String>,
    published_at: Option<String>,
    provenance: Option<String>,
    entries_sha256: Option<String>,
    payload_sha256: Option<String>,
    candidate_mode: Option<String>,
}

fn validate_versioned_metadata(
    source: &str,
    format_version: u8,
    metadata: VersionedPackMetadata,
) -> Result<ValidatedPackMetadata, String> {
    let VersionedPackMetadata {
        minimum_slime_version,
        published_at,
        provenance,
        entries_sha256,
        payload_sha256,
        candidate_mode,
    } = metadata;
    if format_version == 1 {
        if minimum_slime_version.is_some()
            || published_at.is_some()
            || provenance.is_some()
            || entries_sha256.is_some()
            || payload_sha256.is_some()
            || candidate_mode.is_some()
        {
            return Err("versioned metadata requires a v2 or newer pack header".to_owned());
        }
        return Ok(ValidatedPackMetadata {
            minimum_slime_version: None,
            published_at: None,
            provenance: None,
            entries_sha256: None,
            payload_sha256: None,
            candidate_mode: DictionaryPackCandidateMode::Standard,
        });
    }

    let candidate_mode = validate_candidate_mode(format_version, candidate_mode)?;

    let minimum_slime_version = required(minimum_slime_version, "minimum-slime-version")?;
    let published_at = required(published_at, "published-at")?;
    let provenance = required(provenance, "provenance")?;
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
    let marker = format!("{PACK_ENTRIES_MARKER}\n");
    let (_, payload_source) = source
        .split_once(&marker)
        .ok_or_else(|| format!("dictionary pack is missing {PACK_ENTRIES_MARKER:?}"))?;
    let actual_sha256 = sha256_hex(payload_source.as_bytes());
    let (entries_sha256, payload_sha256) = match format_version {
        2 => {
            if payload_sha256.is_some() {
                return Err("payload-sha256 requires the v3 pack header".to_owned());
            }
            let expected = required(entries_sha256, "entries-sha256")?;
            validate_digest("entries-sha256", &expected, &actual_sha256)?;
            (Some(actual_sha256), None)
        }
        3..=5 => {
            if entries_sha256.is_some() {
                return Err(
                    "entries-sha256 is replaced by payload-sha256 in v3 and newer".to_owned(),
                );
            }
            let expected = required(payload_sha256, "payload-sha256")?;
            validate_digest("payload-sha256", &expected, &actual_sha256)?;
            (None, Some(actual_sha256))
        }
        _ => return Err("unsupported dictionary pack version".to_owned()),
    };

    Ok(ValidatedPackMetadata {
        minimum_slime_version: Some(minimum_slime_version),
        published_at: Some(published_at),
        provenance: Some(provenance),
        entries_sha256,
        payload_sha256,
        candidate_mode,
    })
}

fn validate_candidate_mode(
    format_version: u8,
    candidate_mode: Option<String>,
) -> Result<DictionaryPackCandidateMode, String> {
    if format_version < 4 {
        if candidate_mode.is_some() {
            return Err("candidate-mode requires the v4 or newer pack header".to_owned());
        }
        return Ok(DictionaryPackCandidateMode::Standard);
    }
    match required(candidate_mode, "candidate-mode")?.as_str() {
        "standard" => Ok(DictionaryPackCandidateMode::Standard),
        "model-rescore-only" => Ok(DictionaryPackCandidateMode::ModelRescoreOnly),
        "explicit-search-only" if format_version >= 5 => {
            Ok(DictionaryPackCandidateMode::ExplicitSearchOnly)
        }
        _ if format_version >= 5 => Err(
            "candidate-mode must be standard, model-rescore-only, or explicit-search-only"
                .to_owned(),
        ),
        _ => Err("candidate-mode must be standard or model-rescore-only".to_owned()),
    }
}

fn validate_digest(key: &str, expected: &str, actual: &str) -> Result<(), String> {
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{key} must be a 64-character hexadecimal digest"));
    }
    if !expected.eq_ignore_ascii_case(actual) {
        return Err(format!("{key} mismatch: expected {expected}, got {actual}"));
    }
    Ok(())
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
        DictionaryPackCandidateMode, DictionaryPackStore, DictionaryPackTrust,
        DictionaryPackVerificationKey, DictionaryPackVersionFloor, MAX_PACKS,
        MAX_VERIFICATION_KEYS, PACK_DIRECTORY_NAME, pack_signature_path, parse_pack, sha256_hex,
        validate_dictionary_pack,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use slime_converter::{Dictionary, DictionaryEntry};
    use std::fs;

    const VALID_PACK: &str = "\
# slime-dictionary-pack-v1
# id: sample-general
# name: 一般語彙サンプル
# version: 2026.07.1
# license: Example-Test-Only
てすとようご\t試験用語
こまわり\t専門小回り\t6000
";

    fn lower_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }

    #[test]
    fn parses_versioned_pack_metadata_and_entries() {
        let pack = parse_pack(VALID_PACK).unwrap();
        assert_eq!(pack.info.format_version, 1);
        assert_eq!(pack.info.id, "sample-general");
        assert_eq!(pack.info.name, "一般語彙サンプル");
        assert_eq!(pack.info.version, "2026.07.1");
        assert_eq!(pack.info.license, "Example-Test-Only");
        assert_eq!(pack.info.entry_count, 2);
        assert_eq!(pack.info.context_rule_count, 0);
        assert_eq!(pack.info.minimum_slime_version, None);
        assert_eq!(pack.info.payload_sha256, None);
        assert_eq!(pack.entries[1].word_cost, 6000);
        assert_eq!(validate_dictionary_pack(VALID_PACK).unwrap(), pack.info);
    }

    #[test]
    fn validates_v4_model_rescore_only_pack_and_separates_its_layer() {
        let payload = "しんたく\t神託\t500\n";
        let digest = sha256_hex(payload.as_bytes());
        let source = format!(
            "# slime-dictionary-pack-v4\n\
             # id: supplemental-general\n\
             # name: 補助一般語彙\n\
             # version: 2026.08.1\n\
             # license: Apache-2.0\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-11\n\
             # provenance: fixture/generated/supplemental-general\n\
             # candidate-mode: model-rescore-only\n\
             # payload-sha256: {digest}\n\
             # entries\n\
             {payload}"
        );
        let pack = parse_pack(&source).unwrap();
        assert_eq!(pack.info.format_version, 4);
        assert_eq!(
            pack.info.candidate_mode,
            DictionaryPackCandidateMode::ModelRescoreOnly
        );
        let store = DictionaryPackStore {
            packs: vec![pack],
            context_rules: Vec::new(),
            errors: Vec::new(),
        };
        assert!(store.layers().is_empty());
        assert_eq!(store.standard_words().count(), 0);
        assert_eq!(
            store
                .model_rescore_layers(&Dictionary::new(Vec::new()))
                .len(),
            1
        );
        assert!(
            store
                .model_rescore_layers(&Dictionary::new(vec![DictionaryEntry::new(
                    "しんたく",
                    "神託",
                    500,
                )]))
                .is_empty()
        );
        assert!(
            validate_dictionary_pack(&source.replace(
                "# candidate-mode: model-rescore-only\n",
                "# candidate-mode: invalid\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn validates_v5_explicit_search_only_pack_and_separates_its_dictionary() {
        let payload = "あさぼらけ\t朝朗け\t500\nあさぼらけ\t麻幌家\t300\n";
        let digest = sha256_hex(payload.as_bytes());
        let source = format!(
            "# slime-dictionary-pack-v5\n\
             # id: explicit-general\n\
             # name: 明示探索語彙\n\
             # version: 2026.08.1\n\
             # license: Apache-2.0\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-11\n\
             # provenance: fixture/generated/explicit-general\n\
             # candidate-mode: explicit-search-only\n\
             # payload-sha256: {digest}\n\
             # entries\n\
             {payload}"
        );
        let pack = parse_pack(&source).unwrap();
        assert_eq!(pack.info.format_version, 5);
        assert_eq!(
            pack.info.candidate_mode,
            DictionaryPackCandidateMode::ExplicitSearchOnly
        );
        let store = DictionaryPackStore {
            packs: vec![pack],
            context_rules: Vec::new(),
            errors: Vec::new(),
        };
        assert!(store.layers().is_empty());
        assert!(
            store
                .model_rescore_layers(&Dictionary::new(Vec::new()))
                .is_empty()
        );
        assert_eq!(
            store.explicit_search_surfaces("あさぼらけ", 64),
            vec!["麻幌家", "朝朗け"]
        );

        let v4 = source.replacen(
            "# slime-dictionary-pack-v5",
            "# slime-dictionary-pack-v4",
            1,
        );
        assert!(validate_dictionary_pack(&v4).is_err());
    }

    #[test]
    fn validates_v2_compatibility_provenance_and_entries_digest() {
        let entries = "てすとようご\t試験用語\nこまわり\t専門小回り\t6000\n";
        let digest = sha256_hex(entries.as_bytes());
        let source = format!(
            "# slime-dictionary-pack-v2\n\
             # id: sample-general\n\
             # name: 一般語彙サンプル\n\
             # version: 2026.08.1\n\
             # license: Example-Test-Only\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-01\n\
             # provenance: fixture/generated/sample-general\n\
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
            Some("fixture/generated/sample-general")
        );
        assert_eq!(info.entries_sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(info.payload_sha256, None);
        assert_eq!(info.pack_sha256, sha256_hex(source.as_bytes()));
        assert_eq!(info.context_rule_count, 0);

        assert!(validate_dictionary_pack(&source.replace("試験用語", "試験用語改")).is_err());
        assert!(
            validate_dictionary_pack(&source.replace(
                "# minimum-slime-version: 0.1.0",
                "# minimum-slime-version: 99.0.0"
            ))
            .is_err()
        );
    }

    #[test]
    fn validates_v3_payload_and_orders_context_rules() {
        let payload = "てすとようご\t試験用語\n\
# context-rules\n\
長い前文\tきかん\t器官\t999\n\
前文\tきかん\t期間\t20\n\
前文\tきかん\t機関\t10\n";
        let digest = sha256_hex(payload.as_bytes());
        let source = format!(
            "# slime-dictionary-pack-v3\n\
             # id: sample-context\n\
             # name: 文脈サンプル\n\
             # version: 2026.08.1\n\
             # license: Example-Test-Only\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-08\n\
             # provenance: fixture/generated/sample-context\n\
             # payload-sha256: {digest}\n\
             # entries\n\
             {payload}"
        );
        let pack = parse_pack(&source).unwrap();
        assert_eq!(pack.info.format_version, 3);
        assert_eq!(pack.info.entries_sha256, None);
        assert_eq!(pack.info.payload_sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(pack.info.entry_count, 1);
        assert_eq!(pack.info.context_rule_count, 3);
        let store = DictionaryPackStore {
            context_rules: super::merge_context_rules(std::slice::from_ref(&pack)),
            packs: vec![pack],
            errors: Vec::new(),
        };
        let mut surfaces = Vec::new();
        store.visit_contextual_surfaces("より長い前文", "きかん", |surface| {
            surfaces.push(surface.to_owned());
            true
        });
        assert_eq!(surfaces, ["器官", "機関", "期間"]);

        assert!(validate_dictionary_pack(&source.replace("機関", "器官")).is_err());
        assert!(
            validate_dictionary_pack(&source.replace(
                "前文\tきかん\t期間\t20\n",
                "前文\tきかん\t期間\t20\n前文\tきかん\t期間\t30\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn validates_context_only_v3_but_not_entryless_legacy_packs() {
        let payload = "# context-rules\n文章\tかんじ\t漢字\t0\n";
        let digest = sha256_hex(payload.as_bytes());
        let source = format!(
            "# slime-dictionary-pack-v3\n\
             # id: sample-context-only\n\
             # name: 文脈のみのサンプル\n\
             # version: 2026.08.1\n\
             # license: Example-Test-Only\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-08\n\
             # provenance: fixture/generated/sample-context-only\n\
             # payload-sha256: {digest}\n\
             # entries\n\
             {payload}"
        );
        let info = validate_dictionary_pack(&source).unwrap();
        assert_eq!(info.entry_count, 0);
        assert_eq!(info.context_rule_count, 1);

        let legacy_payload = "";
        let legacy_digest = sha256_hex(legacy_payload.as_bytes());
        let legacy = format!(
            "# slime-dictionary-pack-v2\n\
             # id: sample-empty\n\
             # name: 空のサンプル\n\
             # version: 2026.08.1\n\
             # license: Example-Test-Only\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-08\n\
             # provenance: fixture/generated/sample-empty\n\
             # entries-sha256: {legacy_digest}\n\
             # entries\n"
        );
        assert!(validate_dictionary_pack(&legacy).is_err());
    }

    #[test]
    fn rejects_malformed_or_duplicate_data() {
        assert!(parse_pack("てすとようご\t試験用語\n").is_err());
        assert!(parse_pack(&VALID_PACK.replace("sample-general", "Sample Pack")).is_err());
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

        let store =
            DictionaryPackStore::load_with_trust(Some(&directory), &DictionaryPackTrust::default());
        let infos: Vec<_> = store.infos().collect();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, "sample-general");
        assert_eq!(store.standard_words().count(), 2);
        assert_eq!(store.errors().len(), 1);
        assert_eq!(store.errors()[0].file, "broken.slime-dict");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn store_reports_pack_directories_above_the_scan_limit() {
        let directory = std::env::temp_dir().join(format!(
            "slime-too-many-dictionary-packs-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let pack_directory = directory.join(PACK_DIRECTORY_NAME);
        fs::create_dir_all(&pack_directory).unwrap();
        for index in 0..=MAX_PACKS {
            fs::write(
                pack_directory.join(format!("sample-{index:02}.slime-dict")),
                VALID_PACK,
            )
            .unwrap();
        }

        let store =
            DictionaryPackStore::load_with_trust(Some(&directory), &DictionaryPackTrust::default());
        assert!(store.errors().iter().any(|error| {
            error.message == format!("dictionary pack directory exceeds the {MAX_PACKS} pack limit")
        }));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn signed_only_policy_verifies_exact_pack_bytes_and_supports_key_rotation() {
        let directory = std::env::temp_dir().join(format!(
            "slime-signed-dictionary-packs-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let pack_directory = directory.join(PACK_DIRECTORY_NAME);
        fs::create_dir_all(&pack_directory).unwrap();
        let pack_path = pack_directory.join("sample.slime-dict");
        fs::write(&pack_path, VALID_PACK).unwrap();

        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let signature = signing_key.sign(VALID_PACK.as_bytes());
        fs::write(
            pack_signature_path(&pack_path),
            format!(
                "# slime-dictionary-signature-v1\n\
                 # key-id: fixture-2026-a\n\
                 # signature-ed25519: {}\n",
                lower_hex(&signature.to_bytes())
            ),
        )
        .unwrap();
        let active_key = DictionaryPackVerificationKey::new(
            "fixture-2026-a",
            signing_key.verifying_key().to_bytes(),
        )
        .unwrap();
        let future_signing_key = SigningKey::from_bytes(&[8_u8; 32]);
        let future_key = DictionaryPackVerificationKey::new(
            "fixture-2026-b",
            future_signing_key.verifying_key().to_bytes(),
        )
        .unwrap();
        let trust = DictionaryPackTrust::signed_only(vec![future_key, active_key]).unwrap();

        let verified = DictionaryPackStore::load_with_trust(Some(&directory), &trust);
        assert_eq!(verified.infos().count(), 1);
        assert!(verified.errors().is_empty());

        fs::write(&pack_path, format!("{VALID_PACK}\n")).unwrap();
        let tampered = DictionaryPackStore::load_with_trust(Some(&directory), &trust);
        assert_eq!(tampered.infos().count(), 0);
        assert_eq!(
            tampered.errors()[0].message,
            "dictionary pack signature is invalid"
        );

        fs::write(&pack_path, VALID_PACK).unwrap();
        let signature_path = pack_signature_path(&pack_path);
        let sidecar = fs::read_to_string(&signature_path).unwrap();
        fs::write(
            &signature_path,
            sidecar.replace("fixture-2026-a", "fixture-unknown"),
        )
        .unwrap();
        let unknown = DictionaryPackStore::load_with_trust(Some(&directory), &trust);
        assert_eq!(unknown.infos().count(), 0);
        assert_eq!(
            unknown.errors()[0].message,
            "dictionary pack signature uses an unknown key"
        );

        fs::remove_file(pack_signature_path(&pack_path)).unwrap();
        let unsigned = DictionaryPackStore::load_with_trust(Some(&directory), &trust);
        assert_eq!(unsigned.infos().count(), 0);
        assert_eq!(
            unsigned.errors()[0].message,
            "dictionary pack signature is required"
        );
        let compatible =
            DictionaryPackStore::load_with_trust(Some(&directory), &DictionaryPackTrust::default());
        assert_eq!(compatible.infos().count(), 1);
        assert!(compatible.errors().is_empty());

        assert!(DictionaryPackVerificationKey::new("invalid key", [7_u8; 32]).is_err());
        assert!(DictionaryPackVerificationKey::new("fixture-weak", [0_u8; 32]).is_err());
        assert!(
            DictionaryPackVerificationKey::from_lower_hex(
                "fixture-2026-a",
                &lower_hex(signing_key.verifying_key().as_bytes()).to_uppercase(),
            )
            .is_err()
        );
        let duplicate = DictionaryPackVerificationKey::new(
            "fixture-2026-a",
            signing_key.verifying_key().to_bytes(),
        )
        .unwrap();
        assert!(DictionaryPackTrust::signed_only(vec![duplicate.clone(), duplicate]).is_err());
        let too_many_keys: Vec<_> = (0..=MAX_VERIFICATION_KEYS)
            .map(|index| {
                DictionaryPackVerificationKey::new(
                    format!("fixture-key-{index}"),
                    signing_key.verifying_key().to_bytes(),
                )
                .unwrap()
            })
            .collect();
        assert!(DictionaryPackTrust::signed_only(too_many_keys).is_err());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn signed_version_floor_rejects_rollback_and_accepts_the_minimum() {
        let directory = std::env::temp_dir().join(format!(
            "slime-versioned-dictionary-packs-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let pack_directory = directory.join(PACK_DIRECTORY_NAME);
        fs::create_dir_all(&pack_directory).unwrap();
        let pack_path = pack_directory.join("sample.slime-dict");
        let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
        let verification_key = DictionaryPackVerificationKey::new(
            "fixture-2026-a",
            signing_key.verifying_key().to_bytes(),
        )
        .unwrap();
        let floor = DictionaryPackVersionFloor::new("sample-general", "2026.08.1").unwrap();
        let trust = DictionaryPackTrust::signed_only_with_version_floors(
            vec![verification_key.clone()],
            vec![floor.clone()],
        )
        .unwrap();

        let write_signed = |source: &str| {
            fs::write(&pack_path, source).unwrap();
            let signature = signing_key.sign(source.as_bytes());
            fs::write(
                pack_signature_path(&pack_path),
                format!(
                    "# slime-dictionary-signature-v1\n\
                     # key-id: fixture-2026-a\n\
                     # signature-ed25519: {}\n",
                    lower_hex(&signature.to_bytes())
                ),
            )
            .unwrap();
        };

        write_signed(VALID_PACK);
        let rollback = DictionaryPackStore::load_with_trust(Some(&directory), &trust);
        assert_eq!(rollback.infos().count(), 0);
        assert_eq!(
            rollback.errors()[0].message,
            "dictionary pack version is below the configured minimum"
        );

        let current = VALID_PACK.replace("2026.07.1", "2026.08.1");
        write_signed(&current);
        let accepted = DictionaryPackStore::load_with_trust(Some(&directory), &trust);
        assert_eq!(accepted.infos().count(), 1);
        assert!(accepted.errors().is_empty());

        let unknown_id_trust = DictionaryPackTrust::signed_only_with_version_floors(
            vec![verification_key.clone()],
            vec![DictionaryPackVersionFloor::new("other-pack", "1.0.0").unwrap()],
        )
        .unwrap();
        let unknown_id = DictionaryPackStore::load_with_trust(Some(&directory), &unknown_id_trust);
        assert_eq!(unknown_id.infos().count(), 0);
        assert_eq!(
            unknown_id.errors()[0].message,
            "dictionary pack id is not allowed by rollback policy"
        );

        assert_eq!(floor.id(), "sample-general");
        assert_eq!(floor.minimum_version(), "2026.08.1");
        assert!(DictionaryPackVersionFloor::new("sample-general", "2026.08").is_err());
        assert!(
            DictionaryPackTrust::signed_only_with_version_floors(
                vec![verification_key],
                vec![floor.clone(), floor],
            )
            .is_err()
        );

        fs::remove_dir_all(directory).unwrap();
    }
}
