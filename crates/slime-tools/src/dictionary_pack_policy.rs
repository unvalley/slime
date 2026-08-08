use std::fs;
use std::path::Path;

use slime_core::{DictionaryPackVerificationKey, DictionaryPackVersionFloor};

pub(crate) fn load_signed_pack_trust(
    verification_keys: &Path,
    version_floors: &Path,
) -> Result<slime_core::DictionaryPackTrust, String> {
    let key_source = read_policy_input(verification_keys, "verification keys")?;
    let floor_source = read_policy_input(version_floors, "version floors")?;
    let keys = parse_verification_keys(&key_source)?;
    let floors = parse_version_floors(&floor_source)?;
    slime_core::DictionaryPackTrust::signed_only_with_version_floors(keys, floors)
        .map_err(|_| "signed dictionary pack policy is invalid".to_owned())
}

fn parse_verification_keys(source: &str) -> Result<Vec<DictionaryPackVerificationKey>, String> {
    if source.is_empty() {
        return Err("verification key input has no records".to_owned());
    }
    let mut keys = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        let Some((id, encoded_key)) = line.split_once('\t') else {
            return Err(format!(
                "verification key input line {} is malformed",
                line_index + 1
            ));
        };
        if encoded_key.contains('\t') {
            return Err(format!(
                "verification key input line {} is malformed",
                line_index + 1
            ));
        }
        keys.push(
            DictionaryPackVerificationKey::from_lower_hex(id, encoded_key).map_err(|_| {
                format!("verification key input line {} is invalid", line_index + 1)
            })?,
        );
        if keys.len() > 16 {
            return Err("verification key input exceeds the record limit".to_owned());
        }
    }
    Ok(keys)
}

fn parse_version_floors(source: &str) -> Result<Vec<DictionaryPackVersionFloor>, String> {
    if source.is_empty() {
        return Err("version floor input has no records".to_owned());
    }
    let mut floors = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        let Some((id, minimum_version)) = line.split_once('\t') else {
            return Err(format!(
                "version floor input line {} is malformed",
                line_index + 1
            ));
        };
        if minimum_version.contains('\t') {
            return Err(format!(
                "version floor input line {} is malformed",
                line_index + 1
            ));
        }
        floors.push(
            DictionaryPackVersionFloor::new(id, minimum_version)
                .map_err(|_| format!("version floor input line {} is invalid", line_index + 1))?,
        );
        if floors.len() > 64 {
            return Err("version floor input exceeds the record limit".to_owned());
        }
    }
    Ok(floors)
}

fn read_policy_input(path: &Path, kind: &str) -> Result<String, String> {
    const MAX_POLICY_BYTES: u64 = 64 * 1024;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| format!("cannot inspect {kind} input"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{kind} input must be a regular file"));
    }
    if metadata.len() > MAX_POLICY_BYTES {
        return Err(format!("{kind} input exceeds the byte limit"));
    }
    let bytes = fs::read(path).map_err(|_| format!("cannot read {kind} input"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_POLICY_BYTES {
        return Err(format!("{kind} input exceeds the byte limit"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{kind} input must be UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::{parse_verification_keys, parse_version_floors};

    #[test]
    fn policy_errors_do_not_echo_records() {
        let private_key_id = "private-customer-key";
        let error = parse_verification_keys(&format!("{private_key_id}\tnot-hex"))
            .expect_err("malformed key must fail");
        assert!(!error.contains(private_key_id));

        let private_pack_id = "private-customer-pack";
        let error = parse_version_floors(&format!("{private_pack_id}\tnot-a-version"))
            .expect_err("malformed floor must fail");
        assert!(!error.contains(private_pack_id));
    }
}
