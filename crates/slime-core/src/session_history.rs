use crate::user_data::is_useful_history;

const MAX_SESSION_TRANSITIONS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionTransition {
    previous_reading: String,
    previous_surface: String,
    reading: String,
    surface: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviousCommit {
    reading: String,
    surface: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionHistory {
    previous_commit: Option<PreviousCommit>,
    // Most recently selected transition first. A small vector keeps lookup and
    // LRU maintenance simple while bounding both work and retained text.
    transitions: Vec<SessionTransition>,
}

impl SessionHistory {
    pub(crate) fn reset_context(&mut self) {
        self.previous_commit = None;
    }

    pub(crate) fn record_commit(&mut self, reading: &str, surface: &str) {
        if !is_useful_history(reading, surface) {
            self.reset_context();
            return;
        }

        if let Some(previous) = self.previous_commit.as_ref() {
            if let Some(index) = self.transitions.iter().position(|transition| {
                transition.previous_reading == previous.reading
                    && transition.previous_surface == previous.surface
                    && transition.reading == reading
                    && transition.surface == surface
            }) {
                self.transitions.remove(index);
            }
            self.transitions.insert(
                0,
                SessionTransition {
                    previous_reading: previous.reading.clone(),
                    previous_surface: previous.surface.clone(),
                    reading: reading.to_owned(),
                    surface: surface.to_owned(),
                },
            );
            self.transitions.truncate(MAX_SESSION_TRANSITIONS);
        }

        self.previous_commit = Some(PreviousCommit {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
        });
    }

    pub(crate) fn exact_surfaces(&self, reading: &str, limit: usize) -> Vec<&str> {
        self.matching_surfaces(limit, |candidate_reading| candidate_reading == reading)
    }

    pub(crate) fn completion_surfaces(&self, prefix: &str, limit: usize) -> Vec<&str> {
        let prefix_length = prefix.chars().count();
        self.matching_surfaces(limit, |candidate_reading| {
            candidate_reading.starts_with(prefix)
                && candidate_reading
                    .chars()
                    .count()
                    .saturating_sub(prefix_length)
                    >= 2
        })
    }

    fn matching_surfaces(&self, limit: usize, matches_reading: impl Fn(&str) -> bool) -> Vec<&str> {
        if limit == 0 {
            return Vec::new();
        }
        let Some(previous) = self.previous_commit.as_ref() else {
            return Vec::new();
        };

        let mut surfaces = Vec::with_capacity(limit);
        for transition in &self.transitions {
            if transition.previous_reading == previous.reading
                && transition.previous_surface == previous.surface
                && matches_reading(&transition.reading)
                && !surfaces.contains(&transition.surface.as_str())
            {
                surfaces.push(transition.surface.as_str());
                if surfaces.len() == limit {
                    break;
                }
            }
        }
        surfaces
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SESSION_TRANSITIONS, SessionHistory};

    #[test]
    fn previous_surface_reranks_only_the_matching_context() {
        let mut history = SessionHistory::default();
        history.record_commit("ぶんしょう", "文章");
        history.record_commit("かんじ", "漢字");
        history.record_commit("きもち", "気持ち");
        history.record_commit("かんじ", "感じ");

        history.record_commit("ぶんしょう", "文章");
        assert_eq!(history.exact_surfaces("かんじ", 9), ["漢字"]);

        history.record_commit("きもち", "気持ち");
        assert_eq!(history.exact_surfaces("かんじ", 9), ["感じ"]);
    }

    #[test]
    fn same_surface_with_a_different_reading_is_a_different_context() {
        let mut history = SessionHistory::default();
        history.record_commit("あした", "明日");
        history.record_commit("かんじ", "漢字");
        history.record_commit("みょうにち", "明日");

        assert!(history.exact_surfaces("かんじ", 9).is_empty());
    }

    #[test]
    fn completion_requires_at_least_two_remaining_characters() {
        let mut history = SessionHistory::default();
        history.record_commit("ぶんしょう", "文章");
        history.record_commit("かんじへんかん", "漢字変換");
        history.record_commit("ぶんしょう", "文章");

        assert_eq!(history.completion_surfaces("かんじ", 9), ["漢字変換"]);
        assert!(history.completion_surfaces("かんじへんか", 9).is_empty());
    }

    #[test]
    fn transitions_are_lru_bounded() {
        let mut history = SessionHistory::default();
        history.record_commit("ぶんしょう", "文章");
        for index in 0..MAX_SESSION_TRANSITIONS + 10 {
            history.record_commit(&format!("こうほその{index}"), &format!("候補その{index}"));
        }

        assert_eq!(history.transitions.len(), MAX_SESSION_TRANSITIONS);
        assert!(
            history
                .transitions
                .iter()
                .all(|transition| { transition.reading != "こうほその0" })
        );
    }

    #[test]
    fn low_value_commit_breaks_context_without_being_retained() {
        let mut history = SessionHistory::default();
        history.record_commit("ぶんしょう", "文章");
        history.record_commit("かんじ", "漢字");
        history.record_commit("ぶんしょう", "文章");
        history.record_commit("。", "。");

        assert!(history.exact_surfaces("かんじ", 9).is_empty());
        assert!(history.previous_commit.is_none());
    }
}
