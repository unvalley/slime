use crate::Conversion;

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
