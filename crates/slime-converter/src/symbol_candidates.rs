use super::{Candidate, LITERAL_CANDIDATE_COST};

/// Candidate alternatives normally supplied by Mozc's symbol rewriter rather
/// than its system dictionary. Keep these out of the lattice: they are useful
/// for an explicitly converted punctuation mark, but not as extra branches in
/// ordinary sentence conversion.
pub(super) fn append_for_reading(reading: &str, candidates: &mut Vec<Candidate>) {
    let surfaces: &[&str] = match reading {
        // Mozc symbol readings for `。`, plus its half-width transliteration.
        "。" => &["．", ".", "｡", "…", "‥", "⋮", "⋯", "⋰", "⋱"],
        // Mozc symbol readings for `・`, plus its half-width transliteration.
        "・" => &[
            "／", "/", "･", "＼", "\\", "÷", "…", "‥", "︙", "︰", "⋮", "⋯", "⋰", "⋱",
        ],
        _ => return,
    };

    for &surface in surfaces {
        if !candidates
            .iter()
            .any(|candidate| candidate.surface == surface)
        {
            candidates.push(Candidate {
                surface: surface.to_owned(),
                cost: LITERAL_CANDIDATE_COST,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::append_for_reading;

    #[test]
    fn unrelated_readings_do_not_gain_symbol_candidates() {
        let mut candidates = Vec::new();
        append_for_reading("にほん", &mut candidates);
        assert!(candidates.is_empty());
    }
}
