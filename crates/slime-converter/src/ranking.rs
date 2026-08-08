use crate::Conversion;

const DOCUMENT_REPEAT_MIN_SURFACE_CHARACTERS: usize = 2;
const DOCUMENT_REPEAT_PROMOTION: i32 = 750;

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

/// Reuses an exact surface already visible in the local document context.
///
/// This is intentionally narrower than a corpus language model: a candidate
/// must already exist in the dictionary N-best and must have appeared verbatim
/// in context supplied by the input client. One-character surfaces are too
/// broad to be useful anchors.
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
        } else {
            conversion.cost
        }
    }
}
