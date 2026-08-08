//! Builds and validates external Slime dictionary packs without installing them.

use std::env;
use std::fmt::Write as FmtWrite;
use std::fs::{self, OpenOptions};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use sha2::{Digest, Sha256};

mod dictionary_pack_policy;
use dictionary_pack_policy::load_signed_pack_trust;

const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 4_096;
const MAX_ENTRIES: usize = 250_000;
const MAX_CONTEXT_RULES: usize = 100_000;
const DEFAULT_WORD_COST: i32 = 500;
const USAGE: &str = concat!(
    "usage:\n",
    "  slime-dictionary-pack validate <pack.slime-dict> [...]\n",
    "  slime-dictionary-pack build --id ID --name NAME --version VERSION --license LICENSE \\\n",
    "    --minimum-slime-version VERSION --published-at YYYY-MM-DD --provenance VALUE \\\n",
    "    [--entries INPUT.tsv] [--context-rules INPUT.tsv] --output OUTPUT.slime-dict [--json]\n",
    "  slime-dictionary-pack verify-signed --data-dir PATH --verification-keys KEYS.tsv \\\n",
    "    --version-floors FLOORS.tsv --expected-packs N [--json]"
);

static TEMPORARY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    match arguments.next().as_deref() {
        Some("validate") => validate_command(arguments),
        Some("build") => build_command(arguments),
        Some("verify-signed") => verify_signed_command(arguments),
        _ => Err(USAGE.to_owned()),
    }
}

fn validate_command(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let paths: Vec<PathBuf> = arguments.map(PathBuf::from).collect();
    if paths.is_empty() {
        return Err(USAGE.to_owned());
    }

    for path in paths {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let info = slime_core::validate_dictionary_pack(&source)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        println!(
            "v{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            info.format_version,
            info.id,
            info.name,
            info.version,
            info.license,
            info.minimum_slime_version.as_deref().unwrap_or("-"),
            info.published_at.as_deref().unwrap_or("-"),
            info.entry_count,
            info.context_rule_count
        );
    }
    Ok(())
}

#[derive(Debug)]
struct BuildOptions {
    id: String,
    name: String,
    version: String,
    license: String,
    minimum_slime_version: String,
    published_at: String,
    provenance: String,
    entries: Option<PathBuf>,
    context_rules: Option<PathBuf>,
    output: PathBuf,
    json: bool,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct BuildReport {
    format_version: u8,
    entry_count: usize,
    context_rule_count: usize,
    pack_bytes: usize,
    content_sha256: String,
    pack_sha256: String,
}

#[derive(Debug)]
struct VerifySignedOptions {
    data_directory: PathBuf,
    verification_keys: PathBuf,
    version_floors: PathBuf,
    expected_packs: usize,
    json: bool,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct VerifySignedReport {
    pack_count: usize,
    entry_count: usize,
    context_rule_count: usize,
    pack_sha256: Vec<String>,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Entry {
    reading: String,
    surface: String,
    cost: i32,
}

#[derive(Debug, Eq, PartialEq)]
struct ContextRule {
    previous_surface: String,
    reading: String,
    surface: String,
    priority: u16,
}

fn build_command(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let options = parse_build_options(arguments)?;
    let json = options.json;
    let report = build_pack(&options)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|_| "cannot serialize build report".to_owned())?
        );
    } else {
        println!(
            "v{}\tentries={}\tcontext-rules={}\tbytes={}\tcontent-sha256={}\tpack-sha256={}",
            report.format_version,
            report.entry_count,
            report.context_rule_count,
            report.pack_bytes,
            report.content_sha256,
            report.pack_sha256
        );
    }
    Ok(())
}

fn verify_signed_command(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let options = parse_verify_signed_options(arguments)?;
    let json = options.json;
    let report = verify_signed_packs(&options)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|_| "cannot serialize verification report".to_owned())?
        );
    } else {
        println!(
            "packs={}\tentries={}\tcontext-rules={}\tpack-sha256={}",
            report.pack_count,
            report.entry_count,
            report.context_rule_count,
            report.pack_sha256.join(",")
        );
    }
    Ok(())
}

fn parse_verify_signed_options(
    arguments: impl Iterator<Item = String>,
) -> Result<VerifySignedOptions, String> {
    let mut data_directory = None;
    let mut verification_keys = None;
    let mut version_floors = None;
    let mut expected_packs = None;
    let mut json = false;
    let mut arguments = arguments;

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--data-dir" => set_once(
                &mut data_directory,
                PathBuf::from(next_value(&mut arguments)?),
                "--data-dir",
            )?,
            "--verification-keys" => set_once(
                &mut verification_keys,
                PathBuf::from(next_value(&mut arguments)?),
                "--verification-keys",
            )?,
            "--version-floors" => set_once(
                &mut version_floors,
                PathBuf::from(next_value(&mut arguments)?),
                "--version-floors",
            )?,
            "--expected-packs" => {
                let value = next_value(&mut arguments)?
                    .parse::<usize>()
                    .map_err(|_| "--expected-packs must be an integer".to_owned())?;
                if !(1..=64).contains(&value) {
                    return Err("--expected-packs must be between 1 and 64".to_owned());
                }
                set_once(&mut expected_packs, value, "--expected-packs")?;
            }
            "--json" if !json => json = true,
            "--json" => return Err("verify option --json is duplicated".to_owned()),
            _ => return Err("unknown verify-signed option\n".to_owned() + USAGE),
        }
    }

    Ok(VerifySignedOptions {
        data_directory: required(data_directory, "--data-dir")?,
        verification_keys: required(verification_keys, "--verification-keys")?,
        version_floors: required(version_floors, "--version-floors")?,
        expected_packs: required(expected_packs, "--expected-packs")?,
        json,
    })
}

fn parse_build_options(arguments: impl Iterator<Item = String>) -> Result<BuildOptions, String> {
    let mut id = None;
    let mut name = None;
    let mut version = None;
    let mut license = None;
    let mut minimum_slime_version = None;
    let mut published_at = None;
    let mut provenance = None;
    let mut entries = None;
    let mut context_rules = None;
    let mut output = None;
    let mut json = false;
    let mut arguments = arguments;

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--id" => set_once(&mut id, next_value(&mut arguments)?, "--id")?,
            "--name" => set_once(&mut name, next_value(&mut arguments)?, "--name")?,
            "--version" => set_once(&mut version, next_value(&mut arguments)?, "--version")?,
            "--license" => set_once(&mut license, next_value(&mut arguments)?, "--license")?,
            "--minimum-slime-version" => set_once(
                &mut minimum_slime_version,
                next_value(&mut arguments)?,
                "--minimum-slime-version",
            )?,
            "--published-at" => set_once(
                &mut published_at,
                next_value(&mut arguments)?,
                "--published-at",
            )?,
            "--provenance" => {
                set_once(&mut provenance, next_value(&mut arguments)?, "--provenance")?;
            }
            "--entries" => set_once(
                &mut entries,
                PathBuf::from(next_value(&mut arguments)?),
                "--entries",
            )?,
            "--context-rules" => set_once(
                &mut context_rules,
                PathBuf::from(next_value(&mut arguments)?),
                "--context-rules",
            )?,
            "--output" => set_once(
                &mut output,
                PathBuf::from(next_value(&mut arguments)?),
                "--output",
            )?,
            "--json" if !json => json = true,
            "--json" => return Err("build option --json is duplicated".to_owned()),
            _ => return Err("unknown build option\n".to_owned() + USAGE),
        }
    }

    Ok(BuildOptions {
        id: required(id, "--id")?,
        name: required(name, "--name")?,
        version: required(version, "--version")?,
        license: required(license, "--license")?,
        minimum_slime_version: required(minimum_slime_version, "--minimum-slime-version")?,
        published_at: required(published_at, "--published-at")?,
        provenance: required(provenance, "--provenance")?,
        entries,
        context_rules,
        output: required(output, "--output")?,
        json,
    })
}

fn next_value(arguments: &mut impl Iterator<Item = String>) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| "option is missing a value".to_owned())
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("option {option} is duplicated"));
    }
    Ok(())
}

fn required<T>(value: Option<T>, option: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("option {option} is required\n{USAGE}"))
}

fn verify_signed_packs(options: &VerifySignedOptions) -> Result<VerifySignedReport, String> {
    let data_metadata = fs::symlink_metadata(&options.data_directory)
        .map_err(|_| "cannot inspect signed pack data directory".to_owned())?;
    if !data_metadata.file_type().is_dir() {
        return Err("signed pack data directory must be a directory".to_owned());
    }
    let trust = load_signed_pack_trust(&options.verification_keys, &options.version_floors)?;
    let engine = slime_core::SlimeEngine::bundled_with_user_data_and_pack_trust(
        slime_core::UserData::load(&options.data_directory),
        trust,
    );
    let rejected = engine.dictionary_pack_load_errors().len();
    if rejected != 0 {
        return Err(format!(
            "signed dictionary pack verification rejected {rejected} file(s)"
        ));
    }
    let packs: Vec<_> = engine.installed_dictionary_packs().collect();
    if packs.len() != options.expected_packs {
        return Err(format!(
            "expected {} signed dictionary pack(s), accepted {}",
            options.expected_packs,
            packs.len()
        ));
    }
    let entry_count = packs
        .iter()
        .try_fold(0usize, |total, pack| total.checked_add(pack.entry_count))
        .ok_or_else(|| "dictionary pack entry total overflowed".to_owned())?;
    let context_rule_count = packs
        .iter()
        .try_fold(0usize, |total, pack| {
            total.checked_add(pack.context_rule_count)
        })
        .ok_or_else(|| "dictionary pack context rule total overflowed".to_owned())?;
    let mut pack_sha256: Vec<_> = packs.iter().map(|pack| pack.pack_sha256.clone()).collect();
    pack_sha256.sort_unstable();

    Ok(VerifySignedReport {
        pack_count: packs.len(),
        entry_count,
        context_rule_count,
        pack_sha256,
    })
}

fn build_pack(options: &BuildOptions) -> Result<BuildReport, String> {
    validate_output_path(&options.output)?;
    if options.entries.is_none() && options.context_rules.is_none() {
        return Err("build requires --entries or --context-rules".to_owned());
    }
    let mut entries = options
        .entries
        .as_deref()
        .map(read_entries)
        .transpose()?
        .unwrap_or_default();
    entries.sort_unstable();

    let mut context_rules = options
        .context_rules
        .as_deref()
        .map(read_context_rules)
        .transpose()?
        .unwrap_or_default();
    context_rules.sort_unstable_by(|left, right| {
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

    let format_version = if options.context_rules.is_some() {
        3
    } else {
        2
    };
    let mut payload = String::new();
    for entry in &entries {
        writeln!(
            payload,
            "{}\t{}\t{}",
            entry.reading, entry.surface, entry.cost
        )
        .expect("writing to a String cannot fail");
    }
    if format_version == 3 {
        payload.push_str("# context-rules\n");
        for rule in &context_rules {
            writeln!(
                payload,
                "{}\t{}\t{}\t{}",
                rule.previous_surface, rule.reading, rule.surface, rule.priority
            )
            .expect("writing to a String cannot fail");
        }
    }

    let content_sha256 = sha256_hex(payload.as_bytes());
    let digest_key = if format_version == 3 {
        "payload-sha256"
    } else {
        "entries-sha256"
    };
    let source = format!(
        "# slime-dictionary-pack-v{format_version}\n\
         # id: {}\n\
         # name: {}\n\
         # version: {}\n\
         # license: {}\n\
         # minimum-slime-version: {}\n\
         # published-at: {}\n\
         # provenance: {}\n\
         # {digest_key}: {content_sha256}\n\
         # entries\n\
         {payload}",
        options.id,
        options.name,
        options.version,
        options.license,
        options.minimum_slime_version,
        options.published_at,
        options.provenance
    );
    if source.len() > usize::try_from(MAX_INPUT_BYTES).expect("input limit fits usize") {
        return Err("generated dictionary pack exceeds the byte limit".to_owned());
    }
    let info = slime_core::validate_dictionary_pack(&source)
        .map_err(|error| format!("generated dictionary pack is invalid: {error}"))?;
    let pack_sha256 = info.pack_sha256.clone();
    write_new_atomic(&options.output, source.as_bytes())?;

    Ok(BuildReport {
        format_version: info.format_version,
        entry_count: info.entry_count,
        context_rule_count: info.context_rule_count,
        pack_bytes: source.len(),
        content_sha256,
        pack_sha256,
    })
}

fn read_entries(path: &Path) -> Result<Vec<Entry>, String> {
    let source = read_input(path, "entries")?;
    let mut entries = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        validate_input_line(line, line_index + 1, "entry")?;
        let mut columns = line.split('\t');
        let reading = columns.next().unwrap_or_default();
        let surface = columns.next().unwrap_or_default();
        let cost = columns.next().map_or(Ok(DEFAULT_WORD_COST), |value| {
            value
                .parse::<i32>()
                .map_err(|_| format!("entry input line {} has a non-numeric cost", line_index + 1))
        })?;
        if columns.next().is_some() || reading.is_empty() || surface.is_empty() {
            return Err(format!("entry input line {} is malformed", line_index + 1));
        }
        entries.push(Entry {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            cost,
        });
        if entries.len() > MAX_ENTRIES {
            return Err("entries input exceeds the record limit".to_owned());
        }
    }
    Ok(entries)
}

fn read_context_rules(path: &Path) -> Result<Vec<ContextRule>, String> {
    let source = read_input(path, "context rules")?;
    let mut rules = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        validate_input_line(line, line_index + 1, "context rule")?;
        let mut columns = line.split('\t');
        let previous_surface = columns.next().unwrap_or_default();
        let reading = columns.next().unwrap_or_default();
        let surface = columns.next().unwrap_or_default();
        let priority = columns
            .next()
            .unwrap_or_default()
            .parse::<u16>()
            .map_err(|_| {
                format!(
                    "context rule input line {} has a non-numeric priority",
                    line_index + 1
                )
            })?;
        if columns.next().is_some()
            || previous_surface.is_empty()
            || reading.is_empty()
            || surface.is_empty()
        {
            return Err(format!(
                "context rule input line {} is malformed",
                line_index + 1
            ));
        }
        rules.push(ContextRule {
            previous_surface: previous_surface.to_owned(),
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            priority,
        });
        if rules.len() > MAX_CONTEXT_RULES {
            return Err("context rules input exceeds the record limit".to_owned());
        }
    }
    Ok(rules)
}

fn read_input(path: &Path, kind: &str) -> Result<String, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| format!("cannot inspect {kind} input"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{kind} input must be a regular file"));
    }
    if metadata.len() > MAX_INPUT_BYTES {
        return Err(format!("{kind} input exceeds the byte limit"));
    }
    let bytes = fs::read(path).map_err(|_| format!("cannot read {kind} input"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_INPUT_BYTES {
        return Err(format!("{kind} input exceeds the byte limit"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{kind} input must be UTF-8"))
}

fn validate_input_line(line: &str, line_number: usize, kind: &str) -> Result<(), String> {
    if line.len() > MAX_LINE_BYTES {
        return Err(format!(
            "{kind} input line {line_number} exceeds the byte limit"
        ));
    }
    Ok(())
}

fn validate_output_path(path: &Path) -> Result<(), String> {
    if path.extension().and_then(std::ffi::OsStr::to_str) != Some("slime-dict") {
        return Err("output must use the .slime-dict extension".to_owned());
    }
    match fs::symlink_metadata(path) {
        Ok(_) => Err("output already exists".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("cannot inspect output".to_owned()),
    }
}

fn write_new_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let metadata =
        fs::symlink_metadata(parent).map_err(|_| "cannot inspect output directory".to_owned())?;
    if !metadata.file_type().is_dir() {
        return Err("output parent must be a directory".to_owned());
    }

    let mut temporary_path = None;
    for _ in 0..32 {
        let counter = TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".slime-dictionary-pack-{}-{counter}.tmp",
            std::process::id()
        ));
        let mut open_options = OpenOptions::new();
        open_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            open_options.mode(0o600);
        }
        match open_options.open(&candidate) {
            Ok(mut file) => {
                if file
                    .write_all(bytes)
                    .and_then(|()| file.sync_all())
                    .is_err()
                {
                    let _ = fs::remove_file(&candidate);
                    return Err("cannot write output".to_owned());
                }
                temporary_path = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("cannot create temporary output".to_owned()),
        }
    }
    let temporary_path =
        temporary_path.ok_or_else(|| "cannot reserve temporary output".to_owned())?;
    let link_result = fs::hard_link(&temporary_path, path);
    let _ = fs::remove_file(&temporary_path);
    match link_result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err("output already exists".to_owned())
        }
        Err(_) => Err("cannot publish output atomically".to_owned()),
    }
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

#[cfg(test)]
mod tests {
    use super::{BuildOptions, VerifySignedOptions, build_pack, sha256_hex, verify_signed_packs};
    use ed25519_dalek::{Signer, SigningKey};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "slime-dictionary-pack-builder-{}-{counter}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn options(directory: &TestDirectory, output_name: &str) -> BuildOptions {
        BuildOptions {
            id: "sample-context".to_owned(),
            name: "文脈サンプル".to_owned(),
            version: "2026.08.1".to_owned(),
            license: "Example-Test-Only".to_owned(),
            minimum_slime_version: "0.1.0".to_owned(),
            published_at: "2026-08-08".to_owned(),
            provenance: "fixture/generated/sample-context".to_owned(),
            entries: Some(directory.path().join("entries.tsv")),
            context_rules: Some(directory.path().join("context.tsv")),
            output: directory.path().join(output_name),
            json: false,
        }
    }

    fn lower_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }

    fn signed_options(
        directory: &TestDirectory,
        pack_version: &str,
        minimum_version: &str,
        expected_packs: usize,
    ) -> (VerifySignedOptions, String) {
        let data_directory = directory.path().join("data");
        let pack_directory = data_directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        let payload = "てすとようご\tPRIVATE_FIXTURE_SURFACE\t500\n";
        let payload_sha256 = sha256_hex(payload.as_bytes());
        let source = format!(
            "# slime-dictionary-pack-v2\n\
             # id: sample-general\n\
             # name: 一般語彙サンプル\n\
             # version: {pack_version}\n\
             # license: Example-Test-Only\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-08\n\
             # provenance: fixture/generated/sample-general\n\
             # entries-sha256: {payload_sha256}\n\
             # entries\n\
             {payload}"
        );
        let pack_path = pack_directory.join("sample-general.slime-dict");
        fs::write(&pack_path, &source).unwrap();

        let signing_key = SigningKey::from_bytes(&[17_u8; 32]);
        let signature = signing_key.sign(source.as_bytes());
        fs::write(
            pack_path.with_extension("slime-dict.sig"),
            format!(
                "# slime-dictionary-signature-v1\n\
                 # key-id: fixture-2026-a\n\
                 # signature-ed25519: {}\n",
                lower_hex(&signature.to_bytes())
            ),
        )
        .unwrap();
        let verification_keys = directory.path().join("verification-keys.tsv");
        fs::write(
            &verification_keys,
            format!(
                "fixture-2026-a\t{}\n",
                lower_hex(signing_key.verifying_key().as_bytes())
            ),
        )
        .unwrap();
        let version_floors = directory.path().join("version-floors.tsv");
        fs::write(
            &version_floors,
            format!("sample-general\t{minimum_version}\n"),
        )
        .unwrap();

        (
            VerifySignedOptions {
                data_directory,
                verification_keys,
                version_floors,
                expected_packs,
                json: false,
            },
            source,
        )
    }

    #[test]
    fn builds_byte_identical_v3_packs_from_different_input_order() {
        let first = TestDirectory::new();
        let second = TestDirectory::new();
        fs::write(
            first.path().join("entries.tsv"),
            "# ignored\nこまわり\t専門小回り\t6000\nてすとようご\t試験用語\n",
        )
        .unwrap();
        fs::write(
            second.path().join("entries.tsv"),
            "てすとようご\t試験用語\t500\r\nこまわり\t専門小回り\t6000\r\n",
        )
        .unwrap();
        fs::write(
            first.path().join("context.tsv"),
            "文章\tきかん\t期間\t20\n文章\tきかん\t機関\t10\n",
        )
        .unwrap();
        fs::write(
            second.path().join("context.tsv"),
            "文章\tきかん\t機関\t10\r\n文章\tきかん\t期間\t20\r\n",
        )
        .unwrap();

        let first_report = build_pack(&options(&first, "first.slime-dict")).unwrap();
        let second_report = build_pack(&options(&second, "second.slime-dict")).unwrap();
        assert_eq!(first_report, second_report);
        let first_source = fs::read_to_string(first.path().join("first.slime-dict")).unwrap();
        let second_source = fs::read_to_string(second.path().join("second.slime-dict")).unwrap();
        assert_eq!(first_source, second_source);
        assert!(first_source.contains("てすとようご\t試験用語\t500\n"));
        assert!(
            first_source.find("文章\tきかん\t機関\t10").unwrap()
                < first_source.find("文章\tきかん\t期間\t20").unwrap()
        );
        let info = slime_core::validate_dictionary_pack(&first_source).unwrap();
        assert_eq!(info.format_version, 3);
        assert_eq!(info.entry_count, 2);
        assert_eq!(info.context_rule_count, 2);
    }

    #[test]
    fn builds_v2_pack_without_context_input() {
        let directory = TestDirectory::new();
        fs::write(
            directory.path().join("entries.tsv"),
            "てすとようご\t試験用語\n",
        )
        .unwrap();
        let mut build_options = options(&directory, "words.slime-dict");
        build_options.context_rules = None;

        let report = build_pack(&build_options).unwrap();
        let source = fs::read_to_string(&build_options.output).unwrap();
        assert_eq!(report.format_version, 2);
        assert_eq!(report.context_rule_count, 0);
        assert!(source.starts_with("# slime-dictionary-pack-v2\n"));
        assert!(source.contains("# entries-sha256: "));
        assert!(!source.contains("# context-rules"));
    }

    #[test]
    fn builds_context_only_v3_pack_without_a_dummy_entry() {
        let directory = TestDirectory::new();
        fs::write(
            directory.path().join("context.tsv"),
            "文章\tかんじ\t漢字\t0\n",
        )
        .unwrap();
        let mut build_options = options(&directory, "context-only.slime-dict");
        build_options.entries = None;

        let report = build_pack(&build_options).unwrap();
        let source = fs::read_to_string(&build_options.output).unwrap();
        assert_eq!(report.format_version, 3);
        assert_eq!(report.entry_count, 0);
        assert_eq!(report.context_rule_count, 1);
        assert!(source.contains("# entries\n# context-rules\n"));
        assert_eq!(
            slime_core::validate_dictionary_pack(&source)
                .unwrap()
                .entry_count,
            0
        );
    }

    #[test]
    fn never_overwrites_an_existing_output() {
        let directory = TestDirectory::new();
        fs::write(
            directory.path().join("entries.tsv"),
            "てすとようご\t試験用語\n",
        )
        .unwrap();
        fs::write(directory.path().join("context.tsv"), "").unwrap();
        let build_options = options(&directory, "existing.slime-dict");
        fs::write(&build_options.output, "keep-this-content").unwrap();

        assert_eq!(
            build_pack(&build_options).unwrap_err(),
            "output already exists"
        );
        assert_eq!(
            fs::read_to_string(&build_options.output).unwrap(),
            "keep-this-content"
        );
    }

    #[cfg(unix)]
    #[test]
    fn generated_pack_is_private_by_default() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = TestDirectory::new();
        fs::write(
            directory.path().join("entries.tsv"),
            "てすとようご\t試験用語\n",
        )
        .unwrap();
        fs::write(directory.path().join("context.tsv"), "").unwrap();
        let build_options = options(&directory, "private.slime-dict");

        build_pack(&build_options).unwrap();
        let mode = fs::metadata(&build_options.output)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn errors_do_not_disclose_input_paths_or_records() {
        let directory = TestDirectory::new();
        let private_record = "PRIVATE_FIXTURE_TOKEN\t非公開の固有語";
        fs::write(
            directory.path().join("private-input.tsv"),
            format!("{private_record}\n"),
        )
        .unwrap();
        let mut build_options = options(&directory, "invalid.slime-dict");
        build_options.entries = Some(directory.path().join("private-input.tsv"));
        build_options.context_rules = None;

        let error = build_pack(&build_options).unwrap_err();
        assert!(!error.contains(private_record));
        assert!(!error.contains("PRIVATE_FIXTURE_TOKEN"));
        assert!(!error.contains(&directory.path().display().to_string()));
        assert!(!build_options.output.exists());
    }

    #[test]
    fn verifies_a_complete_signed_pack_set_without_vocabulary_output() {
        let directory = TestDirectory::new();
        let (verify_options, source) = signed_options(&directory, "2026.08.1", "2026.08.1", 1);

        let report = verify_signed_packs(&verify_options).unwrap();
        assert_eq!(report.pack_count, 1);
        assert_eq!(report.entry_count, 1);
        assert_eq!(report.context_rule_count, 0);
        assert_eq!(report.pack_sha256, [sha256_hex(source.as_bytes())]);
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains("PRIVATE_FIXTURE_SURFACE"));
        assert!(!serialized.contains("てすとようご"));
        assert!(!serialized.contains(&directory.path().display().to_string()));
    }

    #[test]
    fn signed_pack_gate_rejects_rollback_and_count_mismatch_without_disclosure() {
        let directory = TestDirectory::new();
        let (verify_options, _) = signed_options(&directory, "2026.07.1", "2026.08.1", 1);

        let error = verify_signed_packs(&verify_options).unwrap_err();
        assert_eq!(
            error,
            "signed dictionary pack verification rejected 1 file(s)"
        );
        assert!(!error.contains("PRIVATE_FIXTURE_SURFACE"));
        assert!(!error.contains(&directory.path().display().to_string()));

        let (count_options, _) = signed_options(&directory, "2026.08.1", "2026.08.1", 2);
        assert_eq!(
            verify_signed_packs(&count_options).unwrap_err(),
            "expected 2 signed dictionary pack(s), accepted 1"
        );

        fs::remove_dir_all(count_options.data_directory.join("dictionary-packs")).unwrap();
        fs::create_dir(count_options.data_directory.join("dictionary-packs")).unwrap();
        assert_eq!(
            verify_signed_packs(&VerifySignedOptions {
                expected_packs: 1,
                ..count_options
            })
            .unwrap_err(),
            "expected 1 signed dictionary pack(s), accepted 0"
        );
    }
}
