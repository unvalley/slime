use slime_converter::{DictionaryEntry, DictionaryLayer};

pub const TECHNOLOGY_DICTIONARY: u32 = 1 << 0;
pub const BUSINESS_DICTIONARY: u32 = 1 << 1;
pub const CREATIVE_DICTIONARY: u32 = 1 << 2;
pub const ALL_DOMAIN_DICTIONARIES: u32 =
    TECHNOLOGY_DICTIONARY | BUSINESS_DICTIONARY | CREATIVE_DICTIONARY;

const SUPPLEMENTAL_POS_ID: u16 = 1851;
const DOMAIN_WORD_COST: i32 = 500;
const MIN_DOMAIN_WORD_COST: i32 = 100;
const MAX_DOMAIN_WORD_COST: i32 = 12_000;
const USER_WORD_COST: i32 = 100;

struct DomainSource {
    mask: u32,
    id: &'static str,
    name: &'static str,
    source: &'static str,
}

const SOURCES: [DomainSource; 3] = [
    DomainSource {
        mask: TECHNOLOGY_DICTIONARY,
        id: "technology",
        name: "テクノロジー",
        source: include_str!("../data/technology.tsv"),
    },
    DomainSource {
        mask: BUSINESS_DICTIONARY,
        id: "business",
        name: "ビジネス",
        source: include_str!("../data/business.tsv"),
    },
    DomainSource {
        mask: CREATIVE_DICTIONARY,
        id: "creative",
        name: "クリエイティブ",
        source: include_str!("../data/creative.tsv"),
    },
];

pub fn layers(mask: u32) -> Vec<DictionaryLayer> {
    SOURCES
        .iter()
        .filter(|source| mask & source.mask != 0)
        .map(|source| parse_layer(source.id, source.name, source.source, DOMAIN_WORD_COST))
        .collect()
}

/// Returns the (reading, surface) pairs of the packs selected by `mask`.
///
/// # Panics
///
/// Panics if a bundled dictionary line is malformed, which the crate tests
/// rule out for shipped data.
#[must_use]
pub fn words(mask: u32) -> Vec<(&'static str, &'static str)> {
    let mut words = Vec::new();
    for source in SOURCES.iter().filter(|source| mask & source.mask != 0) {
        for line in source
            .source
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let mut columns = line.split('\t');
            let reading = columns.next().expect("domain dictionary reading");
            let surface = columns.next().expect("domain dictionary surface");
            words.push((reading, surface));
        }
    }
    words
}

pub fn user_layer<'a>(
    entries: impl Iterator<Item = (&'a str, &'a str)>,
) -> Option<DictionaryLayer> {
    let entries: Vec<_> = entries
        .map(|(reading, surface)| entry(reading, surface, USER_WORD_COST))
        .collect();
    (!entries.is_empty()).then(|| DictionaryLayer::new("user", "ユーザー辞書", entries))
}

fn parse_layer(id: &str, name: &str, source: &str, cost: i32) -> DictionaryLayer {
    let entries = source
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut columns = line.split('\t');
            let reading = columns.next().expect("domain dictionary reading");
            let surface = columns.next().expect("domain dictionary surface");
            let word_cost = columns.next().map_or(cost, |value| {
                value.parse().expect("domain dictionary numeric cost")
            });
            assert!(columns.next().is_none(), "domain dictionary column count");
            assert!(!reading.is_empty(), "domain dictionary non-empty reading");
            assert!(!surface.is_empty(), "domain dictionary non-empty surface");
            assert!(
                (MIN_DOMAIN_WORD_COST..=MAX_DOMAIN_WORD_COST).contains(&word_cost),
                "domain dictionary cost is within the reviewed range"
            );
            entry(reading, surface, word_cost)
        })
        .collect();
    DictionaryLayer::new(id, name, entries)
}

fn entry(reading: &str, surface: &str, cost: i32) -> DictionaryEntry {
    DictionaryEntry::with_pos(
        reading,
        surface,
        SUPPLEMENTAL_POS_ID,
        SUPPLEMENTAL_POS_ID,
        cost,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ALL_DOMAIN_DICTIONARIES, BUSINESS_DICTIONARY, CREATIVE_DICTIONARY, TECHNOLOGY_DICTIONARY,
        layers, parse_layer, words,
    };
    use std::collections::HashSet;

    #[test]
    fn each_domain_dictionary_is_an_independent_layer() {
        let all = layers(ALL_DOMAIN_DICTIONARIES);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id(), "technology");
        assert_eq!(all[1].id(), "business");
        assert_eq!(all[2].id(), "creative");
        assert!(all.iter().all(|layer| layer.entry_count() >= 75));

        assert_eq!(layers(TECHNOLOGY_DICTIONARY).len(), 1);
        assert_eq!(layers(BUSINESS_DICTIONARY).len(), 1);
        assert_eq!(layers(CREATIVE_DICTIONARY).len(), 1);
    }

    #[test]
    fn words_expose_every_entry_of_the_selected_packs() {
        let technology = words(TECHNOLOGY_DICTIONARY);
        assert_eq!(
            technology.len(),
            layers(TECHNOLOGY_DICTIONARY)[0].entry_count()
        );
        assert!(
            technology
                .iter()
                .all(|(reading, surface)| !reading.is_empty() && !surface.is_empty())
        );

        let total: usize = layers(ALL_DOMAIN_DICTIONARIES)
            .iter()
            .map(slime_converter::DictionaryLayer::entry_count)
            .sum();
        assert_eq!(words(ALL_DOMAIN_DICTIONARIES).len(), total);
        assert!(words(0).is_empty());
    }

    #[test]
    fn domain_sources_are_well_formed_and_have_unique_pairs() {
        for (id, source) in [
            ("technology", include_str!("../data/technology.tsv")),
            ("business", include_str!("../data/business.tsv")),
            ("creative", include_str!("../data/creative.tsv")),
        ] {
            let layer = parse_layer(id, id, source, 500);
            assert!(layer.entry_count() >= 75, "{id}");

            let mut pairs = HashSet::new();
            for line in source.lines().filter(|line| !line.is_empty()) {
                let mut columns = line.split('\t');
                let reading = columns.next().unwrap();
                let surface = columns.next().unwrap();
                assert!(
                    reading
                        .chars()
                        .all(|character| { matches!(character, '\u{3041}'..='\u{3096}' | 'ー') }),
                    "{id}: reading must be hiragana: {reading}"
                );
                assert!(pairs.insert((reading, surface)), "{id}: duplicate {line}");
            }
        }
    }
}
