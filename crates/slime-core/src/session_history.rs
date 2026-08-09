use crate::user_data::is_useful_context_anchor;

const MAX_EXTERNAL_CONTEXT_CHARACTERS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviousCommit {
    reading: String,
    surface: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionHistory {
    previous_commit: Option<PreviousCommit>,
    external_surface: Option<String>,
    external_right_surface: Option<String>,
}

impl SessionHistory {
    pub(crate) fn reset_context(&mut self) {
        self.previous_commit = None;
        self.external_surface = None;
        self.external_right_surface = None;
    }

    pub(crate) fn set_external_contexts(&mut self, left_surface: &str, right_surface: &str) {
        self.reset_context();
        if !left_surface.is_empty() {
            let start = left_surface
                .char_indices()
                .rev()
                .nth(MAX_EXTERNAL_CONTEXT_CHARACTERS - 1)
                .map_or(0, |(index, _)| index);
            self.external_surface = Some(left_surface[start..].to_owned());
        }
        if !right_surface.is_empty() {
            self.external_right_surface = Some(
                right_surface
                    .chars()
                    .take(MAX_EXTERNAL_CONTEXT_CHARACTERS)
                    .collect(),
            );
        }
    }

    pub(crate) fn record_commit(&mut self, reading: &str, surface: &str) {
        if !is_useful_context_anchor(reading, surface) {
            self.reset_context();
            return;
        }

        self.external_surface = None;
        self.previous_commit = Some(PreviousCommit {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
        });
    }

    pub(crate) fn previous_commit(&self) -> Option<(&str, &str)> {
        self.previous_commit
            .as_ref()
            .map(|previous| (previous.reading.as_str(), previous.surface.as_str()))
    }

    pub(crate) fn previous_surface(&self) -> Option<&str> {
        self.previous_commit()
            .map(|(_, surface)| surface)
            .or(self.external_surface.as_deref())
    }

    pub(crate) fn right_surface(&self) -> Option<&str> {
        self.external_right_surface.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_EXTERNAL_CONTEXT_CHARACTERS, SessionHistory};

    #[test]
    fn useful_commit_becomes_the_left_context() {
        let mut history = SessionHistory::default();
        history.record_commit("あした", "明日");

        assert_eq!(history.previous_commit(), Some(("あした", "明日")));
    }

    #[test]
    fn short_word_can_anchor_the_next_conversion() {
        let mut history = SessionHistory::default();
        history.record_commit("へや", "部屋");

        assert_eq!(history.previous_commit(), Some(("へや", "部屋")));

        history.record_commit("いえ", "家");
        assert_eq!(history.previous_commit(), Some(("いえ", "家")));
    }

    #[test]
    fn low_value_commit_breaks_context_without_being_retained() {
        let mut history = SessionHistory::default();
        history.record_commit("ぶんしょう", "文章");
        history.record_commit("かんじ", "漢字");
        history.record_commit("ぶんしょう", "文章");
        history.record_commit("。", "。");

        assert!(history.previous_commit.is_none());
    }

    #[test]
    fn external_context_is_bounded_and_never_impersonates_a_commit() {
        let mut history = SessionHistory::default();
        let long = "前".repeat(MAX_EXTERNAL_CONTEXT_CHARACTERS + 10);
        history.set_external_contexts(&long, "");

        assert!(history.previous_commit().is_none());
        assert_eq!(
            history.previous_surface().unwrap().chars().count(),
            MAX_EXTERNAL_CONTEXT_CHARACTERS
        );

        history.record_commit("ぶんしょう", "文章");
        assert_eq!(history.previous_commit(), Some(("ぶんしょう", "文章")));
        assert_eq!(history.previous_surface(), Some("文章"));
        assert!(history.right_surface().is_none());
    }

    #[test]
    fn external_right_context_is_bounded_from_the_caret() {
        let mut history = SessionHistory::default();
        let left = "前".repeat(MAX_EXTERNAL_CONTEXT_CHARACTERS + 10);
        let right = "後".repeat(MAX_EXTERNAL_CONTEXT_CHARACTERS + 10);
        history.set_external_contexts(&left, &right);

        assert_eq!(
            history.previous_surface().unwrap().chars().count(),
            MAX_EXTERNAL_CONTEXT_CHARACTERS
        );
        assert_eq!(
            history.right_surface().unwrap().chars().count(),
            MAX_EXTERNAL_CONTEXT_CHARACTERS
        );
    }
}
