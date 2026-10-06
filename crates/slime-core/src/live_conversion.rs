use slime_converter::{Conversion, Dictionary};

use crate::UserData;

/// Live conversion leaves readings shorter than this untouched.
pub(crate) const MINIMUM_READING_CHARACTERS: usize = 2;

/// The best lattice path must clearly beat the runner-up before live
/// conversion changes the text under the user's cursor. Dictionary costs are
/// approximately negative log probabilities scaled by 500, so this requires
/// roughly a 2.7:1 advantage. Explicit Space conversion remains unrestricted.
const MINIMUM_COST_MARGIN: i32 = 500;

/// Incomplete short paths are especially volatile while the next kana is
/// still arriving. Require a wider gap only for structurally suspicious
/// paths, rather than delaying every ordinary LIVE conversion.
const MINIMUM_INCOMPLETE_PATH_COST_MARGIN: i32 = 1_000;
const MINIMUM_PENDING_PREFIX_REWRITE_COST_MARGIN: i32 = 1_100;
const MINIMUM_RARE_KATAKANA_PATH_COST_MARGIN: i32 = 3_000;
const MINIMUM_SINGLE_IDEOGRAPH_COST_MARGIN: i32 = 2_000;
const MINIMUM_AMBIGUOUS_NUMERIC_UNIT_COST_MARGIN: i32 = 2_000;
const MAXIMUM_INCOMPLETE_KATAKANA_READING_CHARACTERS: usize = 4;
const MINIMUM_RARE_KATAKANA_ENTRY_COST: i32 = 5_000;
const MINIMUM_PREFIX_FRAGILE_KANJI_ENTRY_COST: i32 = 5_000;
const MAXIMUM_FRAGMENTED_READING_CHARACTERS: usize = 5;
const MAXIMUM_INCOMPLETE_PATH_READING_CHARACTERS: usize = 10;
const MINIMUM_UNMARKED_LONG_READING_CHARACTERS: usize = 24;
const MAXIMUM_TRAILING_FRAGMENT_READING_CHARACTERS: usize = 4;
const MAXIMUM_LOCAL_NUMERIC_READING_CHARACTERS: usize = 8;
const MINIMUM_PREVIOUS_PATH_CONTINUITY_CHARACTERS: usize = 8;
const MINIMUM_PREVIOUS_PATH_CONTINUITY_ADVANTAGE: usize = 4;
const MINIMUM_LITERAL_EXTENSION_CONTINUITY_ADVANTAGE: usize = 2;
const MAXIMUM_PREVIOUS_PATH_REWRITE_CHARACTERS: usize = 4;
const MINIMUM_CONTINUITY_LOCAL_COST_MARGIN: i32 = 1_000;

/// A close homophone before a plain topic `は` remains rankable until more
/// right-hand context arrives. Display confidence and stable-prefix confidence
/// are intentionally distinct: freezing the earlier phrase is harder to undo
/// than showing the current dictionary winner in marked text.
const MINIMUM_STABLE_TOPIC_COST_MARGIN: i32 = 1_000;
const MAXIMUM_UNSTABLE_TOPIC_READING_CHARACTERS: usize = 32;
const MAXIMUM_WORKER_TOPIC_REOPEN_CHARACTERS: usize = 64;
const WORKER_TOPIC_REOPEN_PATH_LIMIT: usize = 4;
const WORKER_STABLE_REPAIR_PATH_LIMIT: usize = 15;
const MAXIMUM_WORKER_STABLE_REPAIR_CHARACTERS: usize = 2;
const MAXIMUM_WORKER_STABLE_REPAIR_COST_GAP: i32 = 2_000;

/// Repeated explicit correction can personalize a sufficiently specific full
/// phrase, but only while the selected surface remains a near neighbor of the
/// dictionary best path. This keeps history from inventing a live candidate or
/// reviving a context-specific short-word selection globally.
pub(crate) const MINIMUM_PERSONALIZED_READING_CHARACTERS: usize = 5;
const PERSONALIZED_PATH_LIMIT: usize = 10;
const MAXIMUM_PERSONALIZED_COST_GAP: i32 = 2_500;
const LATTICE_FALLBACK_PATH_LIMIT: usize = 15;
const IMPLICIT_NUMERIC_CHALLENGER_PATH_LIMIT: usize = 10;

/// Live confidence compares visible surfaces, not internal lattice paths.
/// The common path starts with two, then expands when that sample is not
/// sufficient to establish a stable visible runner-up.
const INITIAL_PATH_LIMIT: usize = 2;
const EXPANDED_PATH_LIMIT: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    Confident(Surface),
    StableExtension(Surface),
    LatticeFallback(Surface),
    ProtectedLiteral(String),
    Continuity(String),
    /// The dictionary winner is a complete but prefix-fragile short word.
    /// Keep it literal until delayed ranking runs, and remember that the next
    /// synchronous extension must not be treated as a stable boundary.
    DeferredFragile(String),
    Ambiguous(String),
    Literal,
}

#[derive(Clone, Copy)]
pub(crate) struct PreferredSurfaces<'a> {
    pub(crate) recent: Option<&'a str>,
    pub(crate) contextual: Option<&'a str>,
    pub(crate) neural: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub(crate) struct DisplaySurfaces<'a> {
    pub(crate) previous: Option<&'a str>,
    pub(crate) previous_is_continuity_checkpoint: bool,
    pub(crate) stable: Option<&'a str>,
    pub(crate) marked_prefix: Option<&'a str>,
    pub(crate) protected_pending_prefix: Option<&'a str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Surface {
    pub(crate) text: String,
    pub(crate) ends_bunsetsu: bool,
    pub(crate) prefix_fragile: bool,
}

impl Surface {
    fn from_conversion(conversion: &Conversion) -> Self {
        Self {
            text: conversion.surface.clone(),
            ends_bunsetsu: conversion_ends_bunsetsu(conversion),
            prefix_fragile: conversion_is_prefix_fragile_exact_word(conversion),
        }
    }

    fn from_dictionary_winner(
        conversion: &Conversion,
        runner_up: Option<&Conversion>,
        marked_prefix_surface: Option<&str>,
    ) -> Self {
        let close_runner_changes_topic = conversion
            .segments
            .iter()
            .map(|segment| segment.reading.chars().count())
            .sum::<usize>()
            <= MAXIMUM_UNSTABLE_TOPIC_READING_CHARACTERS
            && runner_up.is_some_and(|runner_up| {
                let Some((reading_end, winner_prefix)) =
                    plain_topic_prefix(conversion, marked_prefix_surface)
                else {
                    return false;
                };
                surface_prefix_at_reading_end(runner_up, reading_end)
                    .is_none_or(|runner_prefix| runner_prefix != winner_prefix)
            });
        let required_margin = if close_runner_changes_topic {
            MINIMUM_STABLE_TOPIC_COST_MARGIN
        } else {
            MINIMUM_COST_MARGIN
        };
        let has_stable_margin = runner_up.is_none_or(|runner_up| {
            runner_up.cost.saturating_sub(conversion.cost) >= required_margin
        });
        Self {
            text: conversion.surface.clone(),
            ends_bunsetsu: conversion_ends_bunsetsu(conversion) && has_stable_margin,
            prefix_fragile: conversion_is_prefix_fragile_exact_word(conversion),
        }
    }
}

fn conversion_is_prefix_fragile_exact_word(conversion: &Conversion) -> bool {
    let [segment] = conversion.segments.as_slice() else {
        return false;
    };
    segment.reading != segment.surface
        && segment.surface.chars().all(is_kanji)
        && ((segment.surface.chars().count() == 1 && segment.reading.chars().count() <= 2)
            || segment.cost >= MINIMUM_PREFIX_FRAGILE_KANJI_ENTRY_COST)
}

fn plain_topic_prefix(
    conversion: &Conversion,
    marked_prefix_surface: Option<&str>,
) -> Option<(usize, String)> {
    let mut reading_end = 0;
    let mut surface = String::new();
    for (index, segment) in conversion.segments.iter().enumerate() {
        reading_end += segment.reading.len();
        surface.push_str(&segment.surface);
        if segment.reading != "は" || segment.surface != "は" {
            continue;
        }
        let follows_compound_ni = if index == 0 {
            marked_prefix_surface.is_some_and(|prefix| prefix.ends_with('に'))
        } else {
            conversion.segments[index - 1].reading == "に"
        };
        if !follows_compound_ni {
            return Some((reading_end, surface));
        }
    }
    None
}

fn surface_prefix_at_reading_end(conversion: &Conversion, expected_end: usize) -> Option<String> {
    let mut reading_end = 0;
    let mut surface = String::new();
    for segment in &conversion.segments {
        reading_end += segment.reading.len();
        surface.push_str(&segment.surface);
        if reading_end == expected_end {
            return Some(surface);
        }
        if reading_end > expected_end {
            return None;
        }
    }
    None
}

/// Recovers the surface that an exact full-reading path assigns to a reading
/// boundary. LIVE punctuation checkpoints use this to follow a confident
/// correction without forgetting where the completed clause ends.
pub(crate) fn surface_prefix_at_reading_boundary(
    dictionary: &Dictionary,
    reading: &str,
    surface: &str,
    boundary_reading: &str,
) -> Option<String> {
    if !reading.starts_with(boundary_reading) {
        return None;
    }
    // The surface under test is nearly always the best path, which the key
    // has usually searched already. Otherwise ask the lattice for that exact
    // surface instead of hoping a wide search over a long reading lists it.
    dictionary
        .convert_n_best(reading, INITIAL_PATH_LIMIT)
        .into_iter()
        .find(|conversion| conversion.surface == surface)
        .or_else(|| exact_surface_path(dictionary, reading, surface))
        .and_then(|conversion| surface_prefix_at_reading_end(&conversion, boundary_reading.len()))
}

/// Cheapest lattice path that spells exactly `surface`.
fn exact_surface_path(dictionary: &Dictionary, reading: &str, surface: &str) -> Option<Conversion> {
    dictionary
        .convert_n_best_with_surface_prefix(reading, surface, 1)
        .into_iter()
        .find(|conversion| conversion.surface == surface)
}

/// Finds `surface` among the wide fallback paths, no further than the
/// personalized cost gap from `best_cost`.
///
/// The wide search dominates the key path on a long reading and usually
/// finds nothing. The cheapest path that spells the surface is far cheaper
/// to get, and when it is missing or already too costly the wide search
/// cannot hold an acceptable one. The rank bound of the wide search stays:
/// a literal tail far down the ranking is not worth displaying.
fn lattice_fallback_path(
    dictionary: &Dictionary,
    reading: &str,
    surface: &str,
    best_cost: i32,
) -> Option<Conversion> {
    let within_gap = |conversion: &Conversion| {
        conversion.cost.saturating_sub(best_cost) <= MAXIMUM_PERSONALIZED_COST_GAP
    };
    exact_surface_path(dictionary, reading, surface).filter(within_gap)?;
    dictionary
        .convert_n_best(reading, LATTICE_FALLBACK_PATH_LIMIT)
        .into_iter()
        .find(|conversion| conversion.surface == surface && within_gap(conversion))
}

/// Reports whether a displayed stable prefix still has a close dictionary
/// alternative at a plain topic boundary. This wider check is reserved for
/// the delayed worker; per-key LIVE confidence keeps its smaller path bound.
pub(crate) fn stable_prefix_has_close_topic_challenger(
    dictionary: &Dictionary,
    reading: &str,
    displayed_surface: &str,
) -> bool {
    if reading.chars().count() > MAXIMUM_WORKER_TOPIC_REOPEN_CHARACTERS {
        return false;
    }
    let conversions = dictionary.convert_n_best(reading, WORKER_TOPIC_REOPEN_PATH_LIMIT);
    let Some(winner) = conversions
        .iter()
        .find(|conversion| conversion.surface == displayed_surface)
    else {
        return false;
    };
    let Some((reading_end, winner_prefix)) = plain_topic_prefix(winner, None) else {
        return false;
    };
    conversions.iter().any(|candidate| {
        candidate.surface != winner.surface
            && candidate.cost.saturating_sub(winner.cost) < MINIMUM_STABLE_TOPIC_COST_MARGIN
            && surface_prefix_at_reading_end(candidate, reading_end)
                .is_none_or(|candidate_prefix| candidate_prefix != winner_prefix)
    })
}

/// Reports whether a full-reading lattice path can make a small correction
/// inside the stable prefix while preserving the current right-hand target
/// byte-for-byte. Unlike broad full-sentence reopening, this cannot trade a
/// stable-prefix repair for a new suffix rewrite.
pub(crate) fn stable_prefix_has_bounded_surface_challenger(
    dictionary: &Dictionary,
    reading: &str,
    displayed_surface: &str,
    stable_surface: &str,
    target_reading: &str,
    target_surface: &str,
    target_left_context: &str,
) -> bool {
    if reading.chars().count() > MAXIMUM_WORKER_TOPIC_REOPEN_CHARACTERS
        || contains_decimal_digit(target_surface)
    {
        return false;
    }
    let conversion_paths = dictionary.convert_n_best(reading, WORKER_STABLE_REPAIR_PATH_LIMIT);
    let Some(best_cost) = conversion_paths.first().map(|candidate| candidate.cost) else {
        return false;
    };
    let converted_literal_targets: Vec<_> = if target_surface == target_reading {
        let candidates = if target_left_context.is_empty() {
            dictionary.candidates_with_limit(target_reading, WORKER_STABLE_REPAIR_PATH_LIMIT)
        } else {
            dictionary.candidates_with_context_limit(
                target_reading,
                target_left_context,
                WORKER_STABLE_REPAIR_PATH_LIMIT,
            )
        };
        candidates
            .into_iter()
            .filter(|candidate| candidate.surface != target_surface)
            .map(|candidate| candidate.surface)
            .collect()
    } else {
        Vec::new()
    };
    let shifted_boundary_targets = shifted_boundary_target_surfaces(
        dictionary,
        reading,
        stable_surface,
        target_reading,
        target_left_context,
    );
    conversion_paths.iter().any(|candidate| {
        candidate.surface != displayed_surface
            && candidate.cost.saturating_sub(best_cost) < MAXIMUM_WORKER_STABLE_REPAIR_COST_GAP
            && (candidate
                .surface
                .strip_suffix(target_surface)
                .is_some_and(|candidate_prefix| {
                    bounded_surface_difference(stable_surface, candidate_prefix)
                        || single_segment_kanji_surface_difference(
                            &conversion_paths,
                            displayed_surface,
                            &candidate.surface,
                        )
                })
                || converted_literal_targets.iter().any(|converted_target| {
                    candidate
                        .surface
                        .strip_suffix(converted_target)
                        .is_some_and(|candidate_prefix| {
                            bounded_surface_difference(stable_surface, candidate_prefix)
                        })
                })
                || shifted_boundary_targets.as_ref().is_some_and(
                    |(shortened_stable_surface, targets)| {
                        candidate
                            .surface
                            .strip_prefix(shortened_stable_surface)
                            .is_some_and(|candidate_target| {
                                targets.iter().any(|target| target == candidate_target)
                            })
                    },
                ))
    })
}

fn shifted_boundary_target_surfaces(
    dictionary: &Dictionary,
    reading: &str,
    stable_surface: &str,
    target_reading: &str,
    target_left_context: &str,
) -> Option<(String, Vec<String>)> {
    let stable_reading = reading.strip_suffix(target_reading)?;
    let (_, shifted_reading) = stable_reading.char_indices().next_back()?;
    let (surface_boundary, shifted_surface) = stable_surface.char_indices().next_back()?;
    // Only `に` is opened by this additional worker scope. Other particles may
    // still be repaired after an independently justified full-reading reopen,
    // but using them to create that scope destabilizes unrelated coordination
    // and connective boundaries.
    if shifted_reading != shifted_surface || shifted_reading != 'に' {
        return None;
    }
    let shortened_stable_surface = stable_surface[..surface_boundary].to_owned();
    let external_left_context = target_left_context.strip_suffix(stable_surface)?;
    let mut widened_left_context = external_left_context.to_owned();
    widened_left_context.push_str(&shortened_stable_surface);
    let mut widened_reading = String::with_capacity(
        shifted_reading
            .len_utf8()
            .saturating_add(target_reading.len()),
    );
    widened_reading.push(shifted_reading);
    widened_reading.push_str(target_reading);
    let candidates = if widened_left_context.is_empty() {
        dictionary.candidates_with_limit(&widened_reading, WORKER_STABLE_REPAIR_PATH_LIMIT)
    } else {
        dictionary.candidates_with_context_limit(
            &widened_reading,
            &widened_left_context,
            WORKER_STABLE_REPAIR_PATH_LIMIT,
        )
    };
    let targets = candidates
        .into_iter()
        .filter(|candidate| {
            !candidate.surface.starts_with(shifted_surface)
                && starts_with_nominal_compound(candidate.surface.as_str())
        })
        .map(|candidate| candidate.surface)
        .collect();
    Some((shortened_stable_surface, targets))
}

fn starts_with_nominal_compound(surface: &str) -> bool {
    let mut characters = surface.chars();
    if !characters.next().is_some_and(is_kanji) || !characters.next().is_some_and(is_kanji) {
        return false;
    }
    characters
        .find(|character| !is_kanji(*character))
        .is_none_or(|character| {
            matches!(
                character,
                'は' | 'が' | 'や' | 'を' | 'に' | 'へ' | 'と' | 'で' | 'も' | 'の' | '、' | '。'
            )
        })
}

pub(crate) fn bounded_surface_difference(current: &str, candidate: &str) -> bool {
    let mut current = current.chars();
    let mut candidate = candidate.chars();
    let mut differences = 0;
    loop {
        match (current.next(), candidate.next()) {
            (Some(left), Some(right)) => {
                if left != right && !(is_kanji(left) && is_kanji(right)) {
                    return false;
                }
                differences += usize::from(left != right);
                if differences > MAXIMUM_WORKER_STABLE_REPAIR_CHARACTERS {
                    return false;
                }
            }
            (None, None) => return differences > 0,
            _ => return false,
        }
    }
}

pub(crate) fn bounded_kanji_run_length_difference(current: &str, candidate: &str) -> bool {
    let current: Vec<_> = current.chars().collect();
    let candidate: Vec<_> = candidate.chars().collect();
    let common_prefix = current
        .iter()
        .zip(&candidate)
        .take_while(|(left, right)| left == right)
        .count();
    let maximum_suffix = current
        .len()
        .saturating_sub(common_prefix)
        .min(candidate.len().saturating_sub(common_prefix));
    let common_suffix = current
        .iter()
        .rev()
        .zip(candidate.iter().rev())
        .take(maximum_suffix)
        .take_while(|(left, right)| left == right)
        .count();
    let current_difference = &current[common_prefix..current.len().saturating_sub(common_suffix)];
    let candidate_difference =
        &candidate[common_prefix..candidate.len().saturating_sub(common_suffix)];
    !current_difference.is_empty()
        && !candidate_difference.is_empty()
        && current_difference.len() <= MAXIMUM_WORKER_STABLE_REPAIR_CHARACTERS
        && candidate_difference.len() <= MAXIMUM_WORKER_STABLE_REPAIR_CHARACTERS
        && current_difference.iter().copied().all(is_kanji)
        && candidate_difference.iter().copied().all(is_kanji)
}

pub(crate) fn single_segment_kanji_surface_difference(
    conversions: &[Conversion],
    current_surface: &str,
    candidate_surface: &str,
) -> bool {
    single_segment_surface_difference(conversions, current_surface, candidate_surface, |a, b| {
        a.chars().all(is_kanji) && b.chars().all(is_kanji)
    })
}

pub(crate) fn bounded_inflected_surface_difference(current: &str, candidate: &str) -> bool {
    let mut current = current.chars();
    let mut candidate = candidate.chars();
    let mut differences = 0;
    let mut converts_kana = false;
    loop {
        match (current.next(), candidate.next()) {
            (Some(left), Some(right)) if left != right => {
                if !is_kanji(right) || !(is_kanji(left) || matches!(left, 'ぁ'..='ゖ')) {
                    return false;
                }
                differences += 1;
                converts_kana |= matches!(left, 'ぁ'..='ゖ');
                if differences > MAXIMUM_WORKER_STABLE_REPAIR_CHARACTERS {
                    return false;
                }
            }
            (Some(_), Some(_)) => {}
            (None, None) => return converts_kana,
            _ => return false,
        }
    }
}

pub(crate) fn single_segment_inflected_surface_difference(
    conversions: &[Conversion],
    current_surface: &str,
    candidate_surface: &str,
) -> bool {
    single_segment_surface_difference(conversions, current_surface, candidate_surface, |a, b| {
        let mut characters = a.chars();
        characters.next().is_some_and(is_kanji)
            && characters.next().is_some_and(|c| matches!(c, 'ぁ'..='ゖ'))
            && characters.next().is_none()
            && b.chars().count() == 2
            && b.chars().all(is_kanji)
    })
}

// A larger stable-prefix correction must still preserve every kana and be
// limited to two independently aligned dictionary words.
pub(crate) fn bounded_two_word_kanji_difference(current: &str, candidate: &str) -> bool {
    let mut current = current.chars();
    let mut candidate = candidate.chars();
    let mut differences = 0;
    loop {
        match (current.next(), candidate.next()) {
            (Some(a), Some(b)) if a != b => {
                if !is_kanji(a) || !is_kanji(b) {
                    return false;
                }
                differences += 1;
                if differences > 4 {
                    return false;
                }
            }
            (Some(_), Some(_)) => {}
            (None, None) => return differences >= 3,
            _ => return false,
        }
    }
}

pub(crate) fn two_word_kanji_surface_difference(
    conversions: &[Conversion],
    current_surface: &str,
    candidate_surface: &str,
) -> bool {
    let Some(current) = conversions.iter().find(|c| c.surface == current_surface) else {
        return false;
    };
    let Some(candidate) = conversions.iter().find(|c| c.surface == candidate_surface) else {
        return false;
    };
    if current.segments.len() != candidate.segments.len() {
        return false;
    }
    let mut differences = 0;
    for (a, b) in current.segments.iter().zip(&candidate.segments) {
        if a.reading != b.reading {
            return false;
        }
        if a.surface == b.surface {
            continue;
        }
        if !a.surface.chars().all(is_kanji)
            || !b.surface.chars().all(is_kanji)
            || !bounded_surface_difference(&a.surface, &b.surface)
        {
            return false;
        }
        differences += 1;
        if differences > 2 {
            return false;
        }
    }
    differences == 2
}

fn single_segment_surface_difference(
    conversions: &[Conversion],
    current_surface: &str,
    candidate_surface: &str,
    accepts_difference: impl Fn(&str, &str) -> bool,
) -> bool {
    let Some(current) = conversions
        .iter()
        .find(|conversion| conversion.surface == current_surface)
    else {
        return false;
    };
    let Some(candidate) = conversions
        .iter()
        .find(|conversion| conversion.surface == candidate_surface)
    else {
        return false;
    };
    if current.segments.len() != candidate.segments.len() {
        return false;
    }
    let mut differences = 0;
    for (current, candidate) in current.segments.iter().zip(&candidate.segments) {
        if current.reading != candidate.reading {
            return false;
        }
        if current.surface == candidate.surface {
            continue;
        }
        if !accepts_difference(&current.surface, &candidate.surface) {
            return false;
        }
        differences += 1;
        if differences > 1 {
            return false;
        }
    }
    differences == 1
}

// The snapshot bounds character changes first. This check additionally requires
// one changed dictionary word on each side of the existing scope boundary.
pub(crate) fn two_scope_kanji_surface_difference(
    conversions: &[Conversion],
    current_surface: &str,
    candidate_surface: &str,
    prefix_reading: &str,
    prefix_surface: &str,
) -> bool {
    let Some(current) = conversions.iter().find(|c| c.surface == current_surface) else {
        return false;
    };
    let Some(candidate) = conversions.iter().find(|c| c.surface == candidate_surface) else {
        return false;
    };
    if current.segments.len() != candidate.segments.len() {
        return false;
    }
    let mut reading_bytes = 0;
    let mut surface_bytes = 0;
    let mut crossed_boundary = false;
    let mut differences = [0_u8; 2];
    for (current, candidate) in current.segments.iter().zip(&candidate.segments) {
        if current.reading != candidate.reading {
            return false;
        }
        reading_bytes += current.reading.len();
        surface_bytes += current.surface.len();
        if !crossed_boundary && reading_bytes > prefix_reading.len() {
            return false;
        }
        if current.surface != candidate.surface {
            if !current.surface.chars().all(is_kanji) || !candidate.surface.chars().all(is_kanji) {
                return false;
            }
            let scope = usize::from(crossed_boundary);
            differences[scope] += 1;
            if differences[scope] > 1 {
                return false;
            }
        }
        if reading_bytes == prefix_reading.len() {
            if surface_bytes != prefix_surface.len() {
                return false;
            }
            crossed_boundary = true;
        }
    }
    crossed_boundary && differences == [1, 1]
}

fn is_kanji(character: char) -> bool {
    matches!(character, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}

pub(crate) fn decide(
    dictionary: &Dictionary,
    user_data: &UserData,
    reading: &str,
    use_history: bool,
    defer_fragile_exact_words: bool,
    preferred: PreferredSurfaces<'_>,
    display: DisplaySurfaces<'_>,
) -> Decision {
    if let Some(surface) = user_data.exact_dictionary_surfaces(reading).next() {
        return if surface == reading {
            Decision::Literal
        } else if display.stable == Some(surface) {
            Decision::StableExtension(Surface {
                text: surface.to_owned(),
                ends_bunsetsu: false,
                prefix_fragile: false,
            })
        } else {
            Decision::Confident(Surface {
                text: surface.to_owned(),
                ends_bunsetsu: false,
                prefix_fragile: false,
            })
        };
    }

    let conversions = dictionary.convert_n_best(reading, INITIAL_PATH_LIMIT);
    let Some(best) = conversions.first() else {
        return Decision::Literal;
    };
    if best.surface == reading && preferred.contextual.is_none() {
        return Decision::Literal;
    }

    let personalized_surface =
        if use_history && reading.chars().count() >= MINIMUM_PERSONALIZED_READING_CHARACTERS {
            preferred
                .recent
                .or_else(|| user_data.repeated_live_phrase_surface(reading))
        } else {
            None
        };
    let preferred_surface = personalized_surface
        .or(preferred.contextual)
        .or(preferred.neural);
    let initial_runner_up = conversions
        .iter()
        .find(|conversion| conversion.surface != best.surface);
    let ambiguous_implicit_numeric = !contains_decimal_digit(reading)
        && contains_decimal_digit(&best.surface)
        && implicit_numeric_cost_margin(best) > MINIMUM_COST_MARGIN;
    let long_unmarked_implicit_numeric = !contains_decimal_digit(reading)
        && contains_decimal_digit(&best.surface)
        && display
            .previous
            .is_none_or(|surface| !contains_decimal_digit(surface))
        && reading.chars().count() >= MINIMUM_UNMARKED_LONG_READING_CHARACTERS
        && display.stable.is_none()
        && display.marked_prefix.is_none()
        && display.protected_pending_prefix.is_none();
    let expanded_limit = if ambiguous_implicit_numeric {
        IMPLICIT_NUMERIC_CHALLENGER_PATH_LIMIT
    } else if preferred_surface.is_some() {
        PERSONALIZED_PATH_LIMIT
    } else {
        EXPANDED_PATH_LIMIT
    };
    let expanded =
        (initial_runner_up.is_none() || preferred_surface.is_some() || ambiguous_implicit_numeric)
            .then(|| dictionary.convert_n_best(reading, expanded_limit));
    let ranked = expanded.as_ref().unwrap_or(&conversions);
    let runner_up = ranked
        .iter()
        .find(|conversion| conversion.surface != best.surface);
    if let Some(preferred) = preferred_surface.and_then(|surface| {
        ranked.iter().find(|conversion| {
            conversion.surface == surface
                && conversion.cost.saturating_sub(best.cost) <= MAXIMUM_PERSONALIZED_COST_GAP
        })
    }) {
        return Decision::Confident(Surface::from_conversion(preferred));
    }
    if best.surface == reading {
        return Decision::Literal;
    }
    if !implicit_numeric_surface_is_supported(best, ranked, reading, display.marked_prefix) {
        return rejected_implicit_numeric_decision(display.stable, reading);
    }
    if long_unmarked_implicit_numeric
        && implicit_numeric_phrase_has_close_local_challenger(dictionary, best)
    {
        return Decision::Ambiguous(best.surface.clone());
    }
    if display.stable.is_none()
        && display.marked_prefix.is_none()
        && display.protected_pending_prefix.is_none()
        && trailing_fragment_has_close_local_challenger(dictionary, best)
    {
        return Decision::Ambiguous(best.surface.clone());
    }
    decide_ranked_surface(
        dictionary,
        reading,
        ranked,
        best,
        runner_up,
        display,
        defer_fragile_exact_words,
    )
}

fn decide_ranked_surface(
    dictionary: &Dictionary,
    reading: &str,
    ranked: &[Conversion],
    best: &Conversion,
    runner_up: Option<&Conversion>,
    display: DisplaySurfaces<'_>,
    defer_fragile_exact_words: bool,
) -> Decision {
    let protects_pending_prefix = display.protected_pending_prefix.is_some_and(|prefix| {
        !best.surface.starts_with(prefix)
            && !rewrites_pending_boundary_as_katakana(&best.surface, prefix)
    });
    let mut required_margin = live_display_cost_margin(best, display.marked_prefix);
    if protects_pending_prefix && display.stable.is_some() {
        required_margin = required_margin.max(MINIMUM_PENDING_PREFIX_REWRITE_COST_MARGIN);
    }
    if let Some(runner_up) = runner_up
        && runner_up.cost.saturating_sub(best.cost) < required_margin
    {
        // A ranking can briefly flip at an incomplete suffix boundary. Keep
        // the previous literal extension only when that exact complete surface
        // exists in the already-fetched lattice and remains inside the same
        // confidence margin. No surface is synthesized here.
        if let Some(stable_surface) = display.stable
            && let Some(stable) = ranked.iter().find(|conversion| {
                conversion.surface == stable_surface
                    && conversion.cost.saturating_sub(best.cost) < required_margin
            })
        {
            return if protects_pending_prefix {
                Decision::LatticeFallback(Surface::from_conversion(stable))
            } else {
                Decision::StableExtension(Surface::from_conversion(stable))
            };
        }
        if display.previous_is_continuity_checkpoint
            && protects_pending_prefix
            && let Some(stable_surface) = display.stable
            && let Some(stable) =
                lattice_fallback_path(dictionary, reading, stable_surface, best.cost)
        {
            return Decision::LatticeFallback(Surface::from_conversion(&stable));
        }
        if let Some(fallback) = stable_aligned_literal_suffix_fallback(
            dictionary,
            reading,
            display.stable,
            best,
            runner_up,
        ) {
            return Decision::LatticeFallback(Surface::from_conversion(&fallback));
        }
        if display.previous.is_some_and(|previous| {
            previous_display_strongly_prefers_best(
                dictionary,
                previous,
                display.previous_is_continuity_checkpoint,
                best,
                runner_up,
            )
        }) {
            return Decision::Continuity(best.surface.clone());
        }
        return Decision::Ambiguous(best.surface.clone());
    }
    if defer_fragile_exact_words
        && display.stable.is_none()
        && display.marked_prefix.is_none()
        && display.protected_pending_prefix.is_none()
        && conversion_is_prefix_fragile_exact_word(best)
    {
        return Decision::DeferredFragile(best.surface.clone());
    }
    let best_surface = Surface::from_dictionary_winner(best, runner_up, display.marked_prefix);
    if display.stable == Some(best.surface.as_str()) {
        Decision::StableExtension(best_surface)
    } else {
        Decision::Confident(best_surface)
    }
}

fn previous_display_strongly_prefers_best(
    dictionary: &Dictionary,
    previous: &str,
    previous_is_continuity_checkpoint: bool,
    best: &Conversion,
    runner_up: &Conversion,
) -> bool {
    if contains_decimal_digit(&best.surface) {
        return false;
    }
    let previous_katakana = previous
        .chars()
        .filter(|&character| is_katakana_letter(character))
        .count();
    let best_katakana = best
        .surface
        .chars()
        .filter(|&character| is_katakana_letter(character))
        .count();
    if (1..=2).contains(&best_katakana.saturating_sub(previous_katakana)) {
        return false;
    }
    let best_prefix = common_prefix_characters(previous, &best.surface);
    let runner_up_prefix = common_prefix_characters(previous, &runner_up.surface);
    let required_advantage =
        if previous_is_continuity_checkpoint && best.surface.starts_with(previous) {
            MINIMUM_LITERAL_EXTENSION_CONTINUITY_ADVANTAGE
        } else {
            MINIMUM_PREVIOUS_PATH_CONTINUITY_ADVANTAGE
        };
    if best_prefix < MINIMUM_PREVIOUS_PATH_CONTINUITY_CHARACTERS
        || previous.chars().count().saturating_sub(best_prefix)
            > MAXIMUM_PREVIOUS_PATH_REWRITE_CHARACTERS
        || best_prefix.saturating_sub(runner_up_prefix) < required_advantage
    {
        return false;
    }
    if !best.surface.starts_with(previous)
        && !changed_segment_has_local_continuity_margin(dictionary, best, best_prefix)
    {
        return false;
    }
    true
}

fn changed_segment_has_local_continuity_margin(
    dictionary: &Dictionary,
    conversion: &Conversion,
    unchanged_surface_characters: usize,
) -> bool {
    let mut surface_end = 0;
    let Some(changed) = conversion.segments.iter().find(|segment| {
        surface_end += segment.surface.chars().count();
        surface_end > unchanged_surface_characters
    }) else {
        return false;
    };
    let candidates = dictionary.convert_n_best(&changed.reading, EXPANDED_PATH_LIMIT);
    let Some(selected) = candidates.first() else {
        return false;
    };
    selected.surface == changed.surface
        && candidates
            .iter()
            .find(|candidate| candidate.surface != selected.surface)
            .is_none_or(|runner_up| {
                runner_up.cost.saturating_sub(selected.cost) >= MINIMUM_CONTINUITY_LOCAL_COST_MARGIN
            })
}

fn rejected_implicit_numeric_decision(stable_surface: Option<&str>, reading: &str) -> Decision {
    if let Some(stable_surface) = stable_surface
        && !contains_decimal_digit(stable_surface)
        && stable_surface != reading
    {
        // Reject the numeric parse without throwing away the already displayed
        // conversion to its left. This display-only surface stays unsealable.
        Decision::ProtectedLiteral(stable_surface.to_owned())
    } else {
        Decision::Literal
    }
}

fn rewrites_pending_boundary_as_katakana(surface: &str, pending_prefix: &str) -> bool {
    let Some((boundary, character)) = pending_prefix.char_indices().next_back() else {
        return false;
    };
    matches!(character, 'ぁ'..='ゖ' | 'ー')
        && surface
            .strip_prefix(&pending_prefix[..boundary])
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(is_katakana_letter)
}

fn live_display_cost_margin(conversion: &Conversion, marked_prefix_surface: Option<&str>) -> i32 {
    let reading_characters = conversion
        .segments
        .iter()
        .map(|segment| segment.reading.chars().count())
        .sum::<usize>();
    if reading_characters > MAXIMUM_INCOMPLETE_PATH_READING_CHARACTERS
        || contains_decimal_digit(&conversion.surface)
    {
        return MINIMUM_COST_MARGIN;
    }
    let converted = conversion
        .segments
        .iter()
        .filter(|segment| segment.reading != segment.surface)
        .collect::<Vec<_>>();
    if converted.len() == 1 {
        let segment = converted[0];
        if segment.surface.chars().count() == 1
            && segment.surface.chars().all(is_kanji)
            && conversion.segments.len() > 1
            && !conversion_ends_bunsetsu(conversion)
        {
            // A lone ideograph injected into an otherwise literal phrase is
            // usually an unfinished suffix (`とにかくめ -> とにかく目`).
            // Delay it unless the lattice advantage is overwhelming.
            return MINIMUM_SINGLE_IDEOGRAPH_COST_MARGIN;
        }
        if segment.reading.chars().count() <= MAXIMUM_INCOMPLETE_KATAKANA_READING_CHARACTERS
            && segment.surface.chars().all(is_katakana_letter)
        {
            // Short katakana paths frequently represent a prefix of a longer
            // word (`しんせ -> シンセ`, `とにか -> トニカ`). Weak dictionary
            // entries need a wider gap, while established words such as マジ,
            // ファン, and テスト retain their lower lexical cost and pass the
            // ordinary incomplete-path margin.
            let repeats_in_candidate = conversion
                .surface
                .match_indices(&segment.surface)
                .nth(1)
                .is_some()
                || marked_prefix_surface.is_some_and(|prefix| prefix.contains(&segment.surface));
            return if segment.cost >= MINIMUM_RARE_KATAKANA_ENTRY_COST && !repeats_in_candidate {
                MINIMUM_RARE_KATAKANA_PATH_COST_MARGIN
            } else {
                MINIMUM_INCOMPLETE_PATH_COST_MARGIN
            };
        }
    }
    let converted_reading_characters = converted
        .iter()
        .map(|segment| segment.reading.chars().count())
        .sum::<usize>();
    if converted.len() >= 2
        && converted_reading_characters <= MAXIMUM_FRAGMENTED_READING_CHARACTERS
        && converted
            .iter()
            .all(|segment| segment.surface.chars().all(is_kanji))
        && conversion
            .segments
            .iter()
            .skip_while(|segment| segment.reading == segment.surface)
            .all(|segment| segment.reading != segment.surface)
    {
        // Several tiny converted nodes are a characteristic incomplete-word
        // parse (`めちゃく -> 目着`, `めちゃくち -> 目着地`).
        return MINIMUM_SINGLE_IDEOGRAPH_COST_MARGIN;
    }
    MINIMUM_COST_MARGIN
}

fn trailing_fragment_has_close_local_challenger(
    dictionary: &Dictionary,
    conversion: &Conversion,
) -> bool {
    let reading_characters = conversion
        .segments
        .iter()
        .map(|segment| segment.reading.chars().count())
        .sum::<usize>();
    let Some(last) = conversion.segments.last() else {
        return false;
    };
    let one_kana_converted_segment = |segment: &slime_converter::Segment| {
        segment.reading != segment.surface
            && segment.reading.chars().count() == 1
            && segment.surface.chars().count() == 1
    };
    let short_converted_segment = |segment: &slime_converter::Segment| {
        segment.reading != segment.surface
            && segment.reading.chars().count() <= 2
            && segment.surface.chars().count() == 1
    };
    let ends_in_short_converted_fragment = one_kana_converted_segment(last);
    let ends_in_short_converted_then_literal = last.reading == last.surface
        && last.reading.chars().count() == 1
        && conversion
            .segments
            .get(conversion.segments.len().saturating_sub(2))
            .is_some_and(short_converted_segment);
    if reading_characters < MINIMUM_UNMARKED_LONG_READING_CHARACTERS
        || conversion.segments.len() < 2
        || (!ends_in_short_converted_fragment && !ends_in_short_converted_then_literal)
    {
        return false;
    }

    let mut start = conversion.segments.len() - 1;
    let mut tail_characters = 1;
    while start > 0 {
        let preceding_characters = conversion.segments[start - 1].reading.chars().count();
        if tail_characters + preceding_characters > MAXIMUM_TRAILING_FRAGMENT_READING_CHARACTERS {
            break;
        }
        start -= 1;
        tail_characters += preceding_characters;
    }
    if start == conversion.segments.len() - 1 {
        return false;
    }

    let mut tail_reading = String::new();
    let mut tail_surface = String::new();
    for segment in &conversion.segments[start..] {
        tail_reading.push_str(&segment.reading);
        tail_surface.push_str(&segment.surface);
    }
    let candidates = dictionary.convert_n_best(&tail_reading, EXPANDED_PATH_LIMIT);
    let Some(selected) = candidates
        .iter()
        .find(|candidate| candidate.surface == tail_surface)
    else {
        return false;
    };
    candidates.iter().any(|candidate| {
        candidate.surface != selected.surface
            && (i64::from(candidate.cost) - i64::from(selected.cost)).abs()
                < i64::from(MINIMUM_COST_MARGIN)
    })
}

fn implicit_numeric_phrase_has_close_local_challenger(
    dictionary: &Dictionary,
    conversion: &Conversion,
) -> bool {
    conversion
        .segments
        .iter()
        .enumerate()
        .any(|(index, numeric)| {
            if !contains_decimal_digit(&numeric.surface) || index == 0 {
                return false;
            }
            let preceding = &conversion.segments[index - 1];
            let Some(following) = conversion.segments.get(index + 1) else {
                return false;
            };
            if preceding.surface.ends_with('第') || !is_numeric_unit(&following.surface) {
                return false;
            }

            let local_segments = &conversion.segments[index - 1..=index + 1];
            let local_characters = local_segments
                .iter()
                .map(|segment| segment.reading.chars().count())
                .sum::<usize>();
            if local_characters > MAXIMUM_LOCAL_NUMERIC_READING_CHARACTERS {
                return false;
            }
            let mut local_reading = String::new();
            let mut local_surface = String::new();
            for segment in local_segments {
                local_reading.push_str(&segment.reading);
                local_surface.push_str(&segment.surface);
            }
            let candidates = dictionary.convert_n_best(&local_reading, EXPANDED_PATH_LIMIT);
            let Some(selected) = candidates
                .iter()
                .find(|candidate| candidate.surface == local_surface)
            else {
                return false;
            };
            candidates.iter().any(|candidate| {
                candidate.surface != selected.surface
                    && (i64::from(candidate.cost) - i64::from(selected.cost)).abs()
                        < i64::from(MINIMUM_COST_MARGIN)
            })
        })
}

fn implicit_numeric_surface_is_supported(
    conversion: &Conversion,
    alternatives: &[Conversion],
    reading: &str,
    marked_prefix_surface: Option<&str>,
) -> bool {
    if contains_decimal_digit(reading) || !contains_decimal_digit(&conversion.surface) {
        return true;
    }
    if !conversion
        .segments
        .iter()
        .enumerate()
        .all(|(index, _)| numeric_segment_is_supported(conversion, index, marked_prefix_surface))
    {
        return false;
    }

    let required_margin = implicit_numeric_cost_margin(conversion);
    if required_margin == MINIMUM_COST_MARGIN {
        return true;
    }
    alternatives
        .iter()
        .find(|alternative| !contains_decimal_digit(&alternative.surface))
        .is_none_or(|alternative| {
            alternative.cost.saturating_sub(conversion.cost) >= required_margin
        })
}

fn numeric_segment_is_supported(
    conversion: &Conversion,
    index: usize,
    marked_prefix_surface: Option<&str>,
) -> bool {
    let segment = &conversion.segments[index];
    if !contains_decimal_digit(&segment.surface) {
        return true;
    }
    if conversion.segments.len() == 1 && marked_prefix_surface.is_none() {
        return true;
    }
    let preceded_by_ordinal = index
        .checked_sub(1)
        .is_some_and(|previous| conversion.segments[previous].surface.ends_with('第'))
        || (index == 0 && marked_prefix_surface.is_some_and(|prefix| prefix.ends_with('第')));
    let following = conversion.segments.get(index + 1);
    if preceded_by_ordinal {
        // `第2` is a complete display, and `第2次` is supported by the
        // following unit. Do not let the ordinal exception bless an
        // unrelated incomplete-word parse such as `第2歩` for `大日本`.
        return following.is_some_and(|following| is_numeric_unit(&following.surface))
            || (following.is_none()
                && marked_prefix_surface.is_none()
                && conversion.segments.len() == 2
                && index == 1);
    }
    if following.is_some_and(|following| is_numeric_unit(&following.surface)) {
        return true;
    }
    // In a longer reading, a bare implicit number is usually an unfinished
    // homophone (`位置 -> 1`, `について -> 2つ`, `新日本 -> 新2歩`).
    // Explicit Space conversion is unaffected, and a following numeric unit
    // will make the candidate eligible as soon as it is actually typed.
    false
}

fn is_numeric_unit(surface: &str) -> bool {
    const UNITS: &[&str] = &[
        "年",
        "月",
        "日",
        "時",
        "分",
        "秒",
        "人",
        "名",
        "個",
        "本",
        "枚",
        "台",
        "基",
        "機",
        "回",
        "階",
        "歳",
        "才",
        "円",
        "万",
        "億",
        "兆",
        "位",
        "次",
        "番",
        "号",
        "件",
        "度",
        "点",
        "割",
        "章",
        "節",
        "話",
        "巻",
        "期",
        "世",
        "世紀",
        "キロ",
        "メートル",
        "センチ",
        "ミリ",
        "グラム",
        "トン",
        "リットル",
        "パーセント",
        "%",
        "％",
    ];
    UNITS.iter().any(|unit| surface.starts_with(unit))
}

fn numeric_segment_cost_margin(conversion: &Conversion, index: usize) -> i32 {
    let following = conversion.segments.get(index + 1);
    if following
        .is_some_and(|segment| matches!(segment.surface.as_str(), "個" | "度" | "時" | "期"))
    {
        MINIMUM_AMBIGUOUS_NUMERIC_UNIT_COST_MARGIN
    } else {
        MINIMUM_COST_MARGIN
    }
}

fn implicit_numeric_cost_margin(conversion: &Conversion) -> i32 {
    conversion
        .segments
        .iter()
        .enumerate()
        .filter(|(_, segment)| contains_decimal_digit(&segment.surface))
        .map(|(index, _)| numeric_segment_cost_margin(conversion, index))
        .max()
        .unwrap_or(MINIMUM_COST_MARGIN)
}

fn is_katakana_letter(character: char) -> bool {
    (matches!(character, 'ァ'..='ヺ') && !matches!(character, '・' | 'ー'))
        || (matches!(character, 'ｦ'..='ﾟ') && !matches!(character, '･' | 'ｰ' | 'ﾞ' | 'ﾟ'))
}

/// Keeps only a current, complete lattice path that preserves more of the
/// previous reliable display than reverting the whole reading to hiragana.
/// It is display-only: callers must not seal it as a stable bunsetsu.
fn stable_aligned_literal_suffix_fallback(
    dictionary: &Dictionary,
    reading: &str,
    stable_surface: Option<&str>,
    best: &Conversion,
    runner_up: &Conversion,
) -> Option<Conversion> {
    let stable_surface = stable_surface?;
    let mut prefix_reading_bytes = 0;
    let mut prefix_surface = String::new();
    for (left, right) in best.segments.iter().zip(&runner_up.segments) {
        if left.reading != right.reading || left.surface != right.surface {
            break;
        }
        prefix_reading_bytes += left.reading.len();
        prefix_surface.push_str(&left.surface);
    }
    if prefix_reading_bytes == 0
        || prefix_reading_bytes == reading.len()
        || prefix_surface.chars().count() < 2
        || prefix_surface == reading[..prefix_reading_bytes]
    {
        return None;
    }

    let mut expected = String::with_capacity(prefix_surface.len() + reading.len());
    expected.push_str(&prefix_surface);
    expected.push_str(&reading[prefix_reading_bytes..]);
    if common_prefix_characters(stable_surface, &expected)
        <= common_prefix_characters(stable_surface, reading)
        || (contains_decimal_digit(&expected) && !contains_decimal_digit(stable_surface))
    {
        return None;
    }
    lattice_fallback_path(dictionary, reading, &expected, best.cost)
}

fn common_prefix_characters(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn contains_decimal_digit(surface: &str) -> bool {
    surface
        .chars()
        .any(|character| character.is_ascii_digit() || matches!(character, '０'..='９'))
}

pub(crate) fn conversion_ends_bunsetsu(conversion: &Conversion) -> bool {
    let Some(last) = conversion.segments.last() else {
        return false;
    };
    last.reading == last.surface
        && matches!(
            last.reading.as_str(),
            "は" | "が"
                | "や"
                | "を"
                | "に"
                | "へ"
                | "と"
                | "で"
                | "も"
                | "の"
                | "から"
                | "まで"
                | "より"
                | "こそ"
                | "さえ"
                | "しか"
                | "だけ"
                | "ほど"
                | "って"
        )
}
