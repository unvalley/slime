use slime_converter::Dictionary;

use crate::UserData;

/// Live conversion leaves readings shorter than this untouched.
pub(crate) const MINIMUM_READING_CHARACTERS: usize = 2;

/// The best lattice path must clearly beat the runner-up before live
/// conversion changes the text under the user's cursor. Mozc-style costs are
/// approximately negative log probabilities scaled by 500, so this requires
/// roughly a 2.7:1 advantage. Explicit Space conversion remains unrestricted.
const MINIMUM_COST_MARGIN: i32 = 500;

/// Live confidence compares visible surfaces, not internal lattice paths.
/// Expand only when two top paths render the same text, keeping the common
/// live path on the cheaper two-best search.
const INITIAL_PATH_LIMIT: usize = 2;
const EXPANDED_PATH_LIMIT: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    Confident(String),
    Ambiguous(String),
    Literal,
}

pub(crate) fn decide(dictionary: &Dictionary, user_data: &UserData, reading: &str) -> Decision {
    if let Some(surface) = user_data.exact_dictionary_surfaces(reading).next() {
        return if surface == reading {
            Decision::Literal
        } else {
            Decision::Confident(surface.to_owned())
        };
    }

    let mut conversions = dictionary
        .convert_n_best(reading, INITIAL_PATH_LIMIT)
        .into_iter();
    let Some(best) = conversions.next() else {
        return Decision::Literal;
    };
    if best.surface == reading {
        return Decision::Literal;
    }

    let runner_up = conversions
        .find(|conversion| conversion.surface != best.surface)
        .or_else(|| {
            dictionary
                .convert_n_best(reading, EXPANDED_PATH_LIMIT)
                .into_iter()
                .find(|conversion| conversion.surface != best.surface)
        });
    if let Some(runner_up) = runner_up
        && runner_up.cost.saturating_sub(best.cost) < MINIMUM_COST_MARGIN
    {
        return Decision::Ambiguous(best.surface);
    }
    Decision::Confident(best.surface)
}
