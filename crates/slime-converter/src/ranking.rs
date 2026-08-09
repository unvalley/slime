use crate::Conversion;

const DOCUMENT_REPEAT_MIN_SURFACE_CHARACTERS: usize = 2;
const DOCUMENT_REPEAT_PROMOTION: i32 = 750;
const DOCUMENT_SEGMENT_REPEAT_PROMOTION: i32 = 2_000;

/// Assigns a final ordering cost to a complete conversion candidate.
///
/// The dictionary and connection matrix generate plausible paths first. A
/// statistical language model can implement this trait later without changing
/// the lattice search or the platform-facing candidate API. Lower costs rank
/// first.
pub trait CandidateRanker {
    fn ranking_cost(&self, reading: &str, conversion: &Conversion) -> i32;

    fn ranking_cost_with_context(
        &self,
        reading: &str,
        _left_context: &str,
        conversion: &Conversion,
    ) -> i32 {
        self.ranking_cost(reading, conversion)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CostOnlyRanker;

impl CandidateRanker for CostOnlyRanker {
    fn ranking_cost(&self, _reading: &str, conversion: &Conversion) -> i32 {
        conversion.cost
    }
}

/// Reuses exact surfaces already visible in the local document context.
///
/// This is intentionally narrower than a corpus language model: a candidate
/// must already exist in the dictionary N-best. A complete surface may match
/// anywhere in the supplied context; a segment inside a longer conversion must
/// be a multi-character kanji term at the immediate context tail. That tighter
/// boundary avoids promoting arbitrary substrings from older compounds.
#[derive(Clone, Copy, Debug, Default)]
pub struct DocumentContextRanker;

impl CandidateRanker for DocumentContextRanker {
    fn ranking_cost(&self, _reading: &str, conversion: &Conversion) -> i32 {
        conversion.cost
    }

    fn ranking_cost_with_context(
        &self,
        _reading: &str,
        left_context: &str,
        conversion: &Conversion,
    ) -> i32 {
        let surface_characters = conversion.surface.chars().count();
        if surface_characters >= DOCUMENT_REPEAT_MIN_SURFACE_CHARACTERS
            && left_context.contains(&conversion.surface)
        {
            conversion.cost.saturating_sub(DOCUMENT_REPEAT_PROMOTION)
        } else if conversion.segments.iter().any(|segment| {
            segment.surface.chars().count() >= DOCUMENT_REPEAT_MIN_SURFACE_CHARACTERS
                && segment.surface != segment.reading
                && segment.surface.chars().any(is_kanji)
                && context_tail_matches_surface(left_context, &segment.surface)
        }) {
            conversion
                .cost
                .saturating_sub(DOCUMENT_SEGMENT_REPEAT_PROMOTION)
        } else {
            conversion.cost
        }
    }
}

fn context_tail_matches_surface(left_context: &str, surface: &str) -> bool {
    left_context
        .trim_end_matches(|character: char| !character.is_alphanumeric())
        .ends_with(surface)
}

fn is_kanji(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
    )
}

#[cfg(test)]
mod tests {
    use super::{CandidateRanker, DocumentContextRanker};
    use crate::{Conversion, Segment};

    fn segment(reading: &str, surface: &str) -> Segment {
        Segment {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            cost: 0,
        }
    }

    #[test]
    fn repeated_kanji_segment_promotes_a_longer_conversion() {
        let conversion = Conversion {
            surface: "そして書紀が引用する".to_owned(),
            segments: vec![
                segment("そして", "そして"),
                segment("しょき", "書紀"),
                segment("が", "が"),
                segment("いんよう", "引用"),
                segment("する", "する"),
            ],
            cost: 5_000,
        };

        assert_eq!(
            DocumentContextRanker.ranking_cost_with_context(
                "そしてしょきがいんようする",
                "前文は『日本書紀』",
                &conversion,
            ),
            3_000
        );
    }

    #[test]
    fn kana_and_one_character_segments_are_not_reused() {
        let conversion = Conversion {
            surface: "これを書いた".to_owned(),
            segments: vec![
                segment("これ", "これ"),
                segment("を", "を"),
                segment("か", "書"),
                segment("いた", "いた"),
            ],
            cost: 5_000,
        };

        assert_eq!(
            DocumentContextRanker.ranking_cost_with_context(
                "これをかいた",
                "これを書籍に記録した",
                &conversion,
            ),
            5_000
        );
    }

    #[test]
    fn repeated_segment_inside_older_context_does_not_promote() {
        let conversion = Conversion {
            surface: "閉じた冷却型回路".to_owned(),
            segments: vec![
                segment("とじた", "閉じた"),
                segment("れいきゃく", "冷却"),
                segment("がた", "型"),
                segment("かいろ", "回路"),
            ],
            cost: 5_000,
        };

        assert_eq!(
            DocumentContextRanker.ranking_cost_with_context(
                "とじたれいきゃくがたかいろ",
                "内燃機関の冷却水は船舶に用いられ",
                &conversion,
            ),
            5_000
        );
    }
}
