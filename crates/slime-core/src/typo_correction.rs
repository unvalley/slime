use slime_romaji::RomajiComposer;

const MIN_RAW_LENGTH: usize = 4;
const MAX_RAW_LENGTH: usize = 24;
const MIN_MISSING_CONSONANT_RAW_LENGTH: usize = 6;
const MAX_KEYBOARD_NEIGHBORS_PER_KEY: usize = 6;
const VOWELS: &[u8] = b"aiueo";
const MISSING_ONSET_CONSONANTS: &[u8] = b"bcdfghjkmnprstvwxyz";
const MAX_CORRECTED_READINGS: usize = maximum_variant_count(MAX_RAW_LENGTH);
const DUPLICATE_DELETION_PRIORITY: u8 = 0;
const TRANSPOSITION_PRIORITY: u8 = 1;
const MISSING_GEMINATE_PRIORITY: u8 = 2;
const MISSING_VOWEL_PRIORITY: u8 = 3;
const MISSING_CONSONANT_PRIORITY: u8 = 4;
const NEIGHBOR_SUBSTITUTION_PRIORITY: u8 = 5;
const GENERAL_DELETION_PRIORITY: u8 = 6;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CorrectedReading {
    pub(crate) reading: String,
    pub(crate) edit_priority: u8,
}

pub(crate) fn corrected_readings(raw: &str, original_reading: &str) -> Vec<CorrectedReading> {
    if !(MIN_RAW_LENGTH..=MAX_RAW_LENGTH).contains(&raw.len())
        || !raw.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return Vec::new();
    }

    let raw = raw.to_ascii_lowercase();
    let bytes = raw.as_bytes();
    let mut variants = Vec::with_capacity(maximum_variant_count(raw.len()));

    for index in 0..bytes.len() {
        let priority = if (index > 0 && bytes[index - 1] == bytes[index])
            || bytes.get(index + 1) == Some(&bytes[index])
        {
            DUPLICATE_DELETION_PRIORITY
        } else {
            GENERAL_DELETION_PRIORITY
        };
        let mut variant = raw.clone();
        variant.remove(index);
        push_variant(&mut variants, &variant, priority, original_reading);
    }

    for index in 0..bytes.len().saturating_sub(1) {
        if bytes[index] == bytes[index + 1] {
            continue;
        }
        let mut variant = bytes.to_vec();
        variant.swap(index, index + 1);
        if let Ok(variant) = String::from_utf8(variant) {
            push_variant(
                &mut variants,
                &variant,
                TRANSPOSITION_PRIORITY,
                original_reading,
            );
        }
    }

    for (index, byte) in bytes.iter().copied().enumerate() {
        if !supports_gemination(byte) {
            continue;
        }
        let mut variant = Vec::with_capacity(bytes.len() + 1);
        variant.extend_from_slice(&bytes[..index]);
        variant.push(byte);
        variant.extend_from_slice(&bytes[index..]);
        if let Ok(variant) = String::from_utf8(variant) {
            push_variant(
                &mut variants,
                &variant,
                MISSING_GEMINATE_PRIORITY,
                original_reading,
            );
        }
    }

    for (index, byte) in bytes.iter().copied().enumerate() {
        for replacement in keyboard_neighbors(byte).bytes() {
            let mut variant = bytes.to_vec();
            variant[index] = replacement;
            if let Ok(variant) = String::from_utf8(variant) {
                push_variant(
                    &mut variants,
                    &variant,
                    NEIGHBOR_SUBSTITUTION_PRIORITY,
                    original_reading,
                );
            }
        }
    }

    append_missing_vowel_variants(&mut variants, bytes, original_reading);
    append_missing_consonant_variants(&mut variants, bytes, original_reading);

    variants.sort_unstable_by(|left, right| {
        left.reading
            .cmp(&right.reading)
            .then_with(|| left.edit_priority.cmp(&right.edit_priority))
    });
    variants.dedup_by(|right, left| right.reading == left.reading);
    variants.sort_unstable_by(|left, right| {
        left.edit_priority
            .cmp(&right.edit_priority)
            .then_with(|| left.reading.cmp(&right.reading))
    });
    variants.truncate(MAX_CORRECTED_READINGS);
    variants
}

fn append_missing_vowel_variants(
    variants: &mut Vec<CorrectedReading>,
    raw: &[u8],
    original_reading: &str,
) {
    for index in 0..=raw.len() {
        for vowel in VOWELS {
            push_inserted_variant(
                variants,
                raw,
                index,
                *vowel,
                MISSING_VOWEL_PRIORITY,
                original_reading,
            );
        }
    }
}

fn append_missing_consonant_variants(
    variants: &mut Vec<CorrectedReading>,
    raw: &[u8],
    original_reading: &str,
) {
    if raw.len() < MIN_MISSING_CONSONANT_RAW_LENGTH {
        return;
    }
    for (index, byte) in raw.iter().copied().enumerate() {
        if !is_vowel(byte) {
            continue;
        }
        for consonant in MISSING_ONSET_CONSONANTS {
            push_inserted_variant(
                variants,
                raw,
                index,
                *consonant,
                MISSING_CONSONANT_PRIORITY,
                original_reading,
            );
        }
    }
}

fn push_inserted_variant(
    variants: &mut Vec<CorrectedReading>,
    raw: &[u8],
    index: usize,
    inserted: u8,
    priority: u8,
    original_reading: &str,
) {
    let mut variant = Vec::with_capacity(raw.len() + 1);
    variant.extend_from_slice(&raw[..index]);
    variant.push(inserted);
    variant.extend_from_slice(&raw[index..]);
    if let Ok(variant) = String::from_utf8(variant) {
        push_variant(variants, &variant, priority, original_reading);
    }
}

fn push_variant(
    variants: &mut Vec<CorrectedReading>,
    raw: &str,
    priority: u8,
    original_reading: &str,
) {
    let Some(reading) = compose(raw) else {
        return;
    };
    if reading == original_reading {
        return;
    }
    variants.push(CorrectedReading {
        reading,
        edit_priority: priority,
    });
}

fn compose(raw: &str) -> Option<String> {
    let mut composer = RomajiComposer::new();
    let mut reading = String::with_capacity(raw.len());
    for character in raw.chars() {
        reading.push_str(&composer.push(character).ok()?);
    }
    reading.push_str(&composer.flush());
    (!reading.is_empty()
        && !reading
            .chars()
            .any(|character| character.is_ascii_alphabetic()))
    .then_some(reading)
}

fn keyboard_neighbors(byte: u8) -> &'static str {
    match byte {
        b'q' => "wa",
        b'w' => "qesa",
        b'e' => "wrsd",
        b'r' => "etdf",
        b't' => "ryfg",
        b'y' => "tugh",
        b'u' => "yihj",
        b'i' => "uojk",
        b'o' => "ipkl",
        b'p' => "ol",
        b'a' => "qwsz",
        b's' => "awedxz",
        b'd' => "serfcx",
        b'f' => "drtgvc",
        b'g' => "ftyhbv",
        b'h' => "gyujnb",
        b'j' => "huikmn",
        b'k' => "jiolm",
        b'l' => "kop",
        b'z' => "asx",
        b'x' => "zsdc",
        b'c' => "xdfv",
        b'v' => "cfgb",
        b'b' => "vghn",
        b'n' => "bhjm",
        b'm' => "njk",
        _ => "",
    }
}

const fn supports_gemination(byte: u8) -> bool {
    matches!(
        byte,
        b'b' | b'c' | b'd' | b'f' | b'g' | b'j' | b'k' | b'p' | b's' | b't' | b'z'
    )
}

const fn is_vowel(byte: u8) -> bool {
    matches!(byte, b'a' | b'i' | b'u' | b'e' | b'o')
}

const fn maximum_variant_count(raw_length: usize) -> usize {
    let deletions = raw_length;
    let transpositions = raw_length.saturating_sub(1);
    let geminations = raw_length;
    let neighbor_substitutions = raw_length * MAX_KEYBOARD_NEIGHBORS_PER_KEY;
    let vowel_insertions = (raw_length + 1) * VOWELS.len();
    let consonant_insertions = if raw_length >= MIN_MISSING_CONSONANT_RAW_LENGTH {
        raw_length * MISSING_ONSET_CONSONANTS.len()
    } else {
        0
    };
    deletions
        + transpositions
        + geminations
        + neighbor_substitutions
        + vowel_insertions
        + consonant_insertions
}

#[cfg(test)]
mod tests {
    use super::corrected_readings;

    #[test]
    fn generates_supported_single_key_edits() {
        let duplicate = corrected_readings("kannji", "かんんじ");
        assert!(
            duplicate
                .iter()
                .any(|candidate| candidate.reading == "かんじ" && candidate.edit_priority == 0)
        );

        let transposed = corrected_readings("niohn", "におhn");
        assert!(
            transposed
                .iter()
                .any(|candidate| candidate.reading == "にほん" && candidate.edit_priority == 1)
        );

        let neighboring = corrected_readings("nihpn", "にhpn");
        assert!(
            neighboring
                .iter()
                .any(|candidate| candidate.reading == "にほん" && candidate.edit_priority == 5)
        );

        let missing_vowel = corrected_readings("nihn", "にhn");
        assert!(
            missing_vowel
                .iter()
                .any(|candidate| candidate.reading == "にほん" && candidate.edit_priority == 3)
        );
        assert!(
            corrected_readings("nihn", "にhん")
                .iter()
                .any(|candidate| candidate.reading == "にほん" && candidate.edit_priority == 3)
        );

        let missing_geminate = corrected_readings("keka", "けか");
        assert!(
            missing_geminate
                .iter()
                .any(|candidate| candidate.reading == "けっか" && candidate.edit_priority == 2)
        );

        let missing_consonant = corrected_readings("paokon", "ぱおこん");
        assert!(
            missing_consonant
                .iter()
                .any(|candidate| candidate.reading == "ぱそこん" && candidate.edit_priority == 4)
        );
    }

    #[test]
    fn skips_short_non_ascii_and_already_equivalent_inputs() {
        assert!(corrected_readings("kan", "かん").is_empty());
        assert!(corrected_readings("かんじ", "かんじ").is_empty());
        assert!(
            corrected_readings("kannji", "かんじ")
                .iter()
                .all(|candidate| candidate.reading != "かんじ")
        );
    }

    #[test]
    fn long_input_keeps_a_general_extra_character_correction() {
        let corrections = corrected_readings("kakikukekosashisusesonnx", "かきくけこさしすせそんx");

        assert!(
            corrections
                .iter()
                .any(|candidate| candidate.reading == "かきくけこさしすせそん")
        );
    }
}
