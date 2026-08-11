use crate::{Conversion, tuning_parameter};
use std::collections::HashSet;
use std::sync::OnceLock;

pub(crate) const MAX_READING_CHARACTERS: usize = 24;
pub(crate) const MAX_VARIANTS: usize = 16;
pub(crate) const PATHS_PER_VARIANT: usize = 6;
pub(crate) const MAX_ADDED_CONVERSIONS: usize = 12;

const MAX_MARKS: usize = 4;

#[derive(Clone, Debug)]
pub(crate) struct ReadingVariant {
    pub(crate) reading: String,
    pub(crate) substituted_offsets: Vec<usize>,
}

#[derive(Clone, Debug)]
struct VariantState {
    replacements: Vec<Option<char>>,
    substitutions: usize,
    secondary_options: usize,
}

/// Produces a bounded set of orthographic readings for pronunciation-style
/// long marks. Japanese text sometimes spells a long vowel with `ー` even
/// where dictionaries use `う`, `い`, `お`, or `え` (for example
/// `ちゅーごく` versus `ちゅうごく`). The original reading is searched
/// separately and these alternatives carry an explicit cost penalty.
pub(crate) fn orthographic_long_vowel_variants(reading: &str) -> Vec<ReadingVariant> {
    if !reading.contains('ー')
        || reading.chars().take(MAX_READING_CHARACTERS + 1).count() > MAX_READING_CHARACTERS
    {
        return Vec::new();
    }
    let mut marks = Vec::new();
    let mut previous = None;
    for (offset, character) in reading.char_indices() {
        if character == 'ー'
            && let Some(options) = previous.and_then(orthographic_long_vowel_options)
        {
            marks.push((offset, options));
        }
        previous = Some(character);
    }
    if marks.is_empty() || marks.len() > MAX_MARKS {
        return Vec::new();
    }

    let mut states = vec![VariantState {
        replacements: Vec::with_capacity(marks.len()),
        substitutions: 0,
        secondary_options: 0,
    }];
    for (_, options) in &marks {
        let mut next = Vec::with_capacity(states.len().saturating_mul(options.len() + 1));
        for state in states {
            let mut unchanged = state.clone();
            unchanged.replacements.push(None);
            next.push(unchanged);
            for (option_index, &replacement) in options.iter().enumerate() {
                let mut replaced = state.clone();
                replaced.replacements.push(Some(replacement));
                replaced.substitutions += 1;
                replaced.secondary_options += option_index;
                next.push(replaced);
            }
        }
        states = next;
    }
    states.retain(|state| state.substitutions > 0);
    states.sort_by_key(|state| (state.substitutions, state.secondary_options));
    states.truncate(MAX_VARIANTS);

    states
        .into_iter()
        .map(|state| {
            let mut variant = String::with_capacity(reading.len());
            let mut mark_index = 0;
            let mut substituted_offsets = Vec::with_capacity(state.substitutions);
            for (offset, character) in reading.char_indices() {
                if marks
                    .get(mark_index)
                    .is_some_and(|(mark_offset, _)| *mark_offset == offset)
                {
                    if let Some(replacement) = state.replacements[mark_index] {
                        variant.push(replacement);
                        substituted_offsets.push(offset);
                    } else {
                        variant.push(character);
                    }
                    mark_index += 1;
                } else {
                    variant.push(character);
                }
            }
            ReadingVariant {
                reading: variant,
                substituted_offsets,
            }
        })
        .collect()
}

fn orthographic_long_vowel_options(character: char) -> Option<&'static [char]> {
    match character {
        'ぁ' | 'あ' | 'か' | 'が' | 'さ' | 'ざ' | 'た' | 'だ' | 'な' | 'は' | 'ば' | 'ぱ'
        | 'ま' | 'ゃ' | 'や' | 'ら' | 'ゎ' | 'わ' => Some(&['あ']),
        'ぃ' | 'い' | 'き' | 'ぎ' | 'し' | 'じ' | 'ち' | 'ぢ' | 'に' | 'ひ' | 'び' | 'ぴ'
        | 'み' | 'り' | 'ゐ' => Some(&['い']),
        'ぅ' | 'う' | 'く' | 'ぐ' | 'す' | 'ず' | 'つ' | 'づ' | 'ぬ' | 'ふ' | 'ぶ' | 'ぷ'
        | 'む' | 'ゅ' | 'ゆ' | 'る' | 'ゔ' => Some(&['う']),
        'ぇ' | 'え' | 'け' | 'げ' | 'せ' | 'ぜ' | 'て' | 'で' | 'ね' | 'へ' | 'べ' | 'ぺ'
        | 'め' | 'れ' | 'ゑ' => Some(&['い', 'え']),
        'ぉ' | 'お' | 'こ' | 'ご' | 'そ' | 'ぞ' | 'と' | 'ど' | 'の' | 'ほ' | 'ぼ' | 'ぽ'
        | 'も' | 'ょ' | 'よ' | 'ろ' | 'を' => Some(&['う', 'お']),
        _ => None,
    }
}

pub(crate) fn remap_conversion(
    mut conversion: Conversion,
    original_reading: &str,
    substituted_offsets: &[usize],
) -> Option<Conversion> {
    let penalty_per_mark = substitution_penalty();
    let mut start = 0_usize;
    let mut substituted_segments = Vec::new();
    for (segment_index, segment) in conversion.segments.iter_mut().enumerate() {
        let end = start.checked_add(segment.reading.len())?;
        let original_segment = original_reading.get(start..end)?;
        let substitutions = substituted_offsets
            .iter()
            .filter(|&&offset| (start..end).contains(&offset))
            .count();
        if substitutions > 0 {
            substituted_segments.push(segment_index);
        }
        let penalty = i32::try_from(substitutions)
            .unwrap_or(i32::MAX)
            .saturating_mul(penalty_per_mark);
        original_segment.clone_into(&mut segment.reading);
        segment.cost = segment.cost.saturating_add(penalty);
        start = end;
    }
    if start != original_reading.len() {
        return None;
    }
    if substituted_segments.iter().all(|&index| {
        !conversion.segments[index]
            .surface
            .chars()
            .any(is_ideographic_or_numeric)
    }) {
        return None;
    }
    if substituted_segments.iter().any(|&index| {
        index > 0
            && index + 1 < conversion.segments.len()
            && is_katakana_surface(&conversion.segments[index - 1].surface)
            && is_katakana_surface(&conversion.segments[index + 1].surface)
    }) {
        return None;
    }
    let total_penalty = i32::try_from(substituted_offsets.len())
        .unwrap_or(i32::MAX)
        .saturating_mul(penalty_per_mark);
    conversion.cost = conversion.cost.saturating_add(total_penalty);
    Some(conversion)
}

fn is_ideographic_or_numeric(character: char) -> bool {
    matches!(
        character,
        '0'..='9'
            | '\u{ff10}'..='\u{ff19}'
            | '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
    )
}

fn is_katakana_surface(surface: &str) -> bool {
    !surface.is_empty()
        && surface
            .chars()
            .all(|character| matches!(character, '\u{30a0}'..='\u{30ff}'))
}

pub(crate) fn sort_and_deduplicate(conversions: &mut Vec<Conversion>, limit: usize) {
    conversions.sort_by_key(|conversion| conversion.cost);
    let mut surfaces = HashSet::with_capacity(conversions.len());
    conversions.retain(|conversion| surfaces.insert(conversion.surface.clone()));
    conversions.truncate(limit);
}

fn substitution_penalty() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| tuning_parameter("SLIME_LONG_VOWEL_PENALTY", 2_000))
}
