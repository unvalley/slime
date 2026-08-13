use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const USER_DICTIONARY_FILE: &str = "user_dictionary.tsv";
const HISTORY_FILE: &str = "history.tsv";
const HISTORY_PREFERENCES_FILE: &str = "history_preferences.tsv";
const CONTEXT_HISTORY_FILE: &str = "context_history.tsv";
const USER_DICTIONARY_HEADER: &str = "# slime-user-dictionary-v1";
const HISTORY_HEADER: &str = "# slime-history-v1";
const HISTORY_PREFERENCES_HEADER: &str = "# slime-history-preferences-v1";
const CONTEXT_HISTORY_HEADER: &str = "# slime-context-history-v1";
const MAX_HISTORY_ENTRIES: usize = 500;
const MAX_HISTORY_PREFERENCES: usize = MAX_HISTORY_ENTRIES;
const MAX_CONTEXT_HISTORY_ENTRIES: usize = 500;
const MIN_COMPLETION_REMAINING_CHARS: usize = 2;
const MIN_ESTABLISHED_HISTORY_COUNT: u32 = 5;
const MIN_ESTABLISHED_CONTEXT_COUNT: u32 = MIN_ESTABLISHED_HISTORY_COUNT;
const MIN_CONTEXT_USE_COUNT: u32 = 2;
const MIN_COMPLETION_USE_COUNT: u32 = MIN_ESTABLISHED_HISTORY_COUNT;
const MAX_HISTORY_READING_CHARS: usize = 64;
const MAX_HISTORY_SURFACE_CHARS: usize = 128;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserDictionaryEntry {
    pub reading: String,
    pub surface: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub reading: String,
    pub surface: String,
    pub count: u32,
    pub last_used: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HistoryPreferenceEntry {
    reading: String,
    surface: String,
    last_used: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingHistoryPreference {
    reading: String,
    surface: String,
    context: Option<HistoryPreferenceContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HistoryPreferenceContext {
    previous_reading: String,
    previous_surface: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ContextHistoryEntry {
    previous_reading: String,
    previous_surface: String,
    reading: String,
    surface: String,
    count: u32,
    last_used: u64,
}

#[derive(Clone, Debug, Default)]
pub struct UserData {
    directory: Option<PathBuf>,
    dictionary: Vec<UserDictionaryEntry>,
    history: Vec<HistoryEntry>,
    history_preferences: Vec<HistoryPreferenceEntry>,
    pending_history_preferences: Vec<PendingHistoryPreference>,
    context_history: Vec<ContextHistoryEntry>,
    history_is_writable: bool,
    history_preferences_are_writable: bool,
    context_history_is_writable: bool,
}

impl UserData {
    #[must_use]
    pub fn load(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        let dictionary = read_optional(&directory.join(USER_DICTIONARY_FILE))
            .ok()
            .flatten()
            .and_then(|bytes| parse_user_dictionary(&bytes).ok())
            .unwrap_or_default();
        let history_result = read_optional(&directory.join(HISTORY_FILE))
            .map_err(|_| ())
            .and_then(|bytes| bytes.map_or(Ok(Vec::new()), |bytes| parse_history(&bytes)));
        let (history, history_is_writable) = match history_result {
            Ok(history) => (history, true),
            Err(()) => (Vec::new(), false),
        };
        let history_preferences_result = read_optional(&directory.join(HISTORY_PREFERENCES_FILE))
            .map_err(|_| ())
            .and_then(|bytes| {
                bytes.map_or(Ok(Vec::new()), |bytes| parse_history_preferences(&bytes))
            });
        let (history_preferences, history_preferences_are_writable) =
            match history_preferences_result {
                Ok(preferences) => (preferences, true),
                Err(()) => (Vec::new(), false),
            };
        let context_history_result = read_optional(&directory.join(CONTEXT_HISTORY_FILE))
            .map_err(|_| ())
            .and_then(|bytes| bytes.map_or(Ok(Vec::new()), |bytes| parse_context_history(&bytes)));
        let (context_history, context_history_is_writable) = match context_history_result {
            Ok(history) => (history, true),
            Err(()) => (Vec::new(), false),
        };

        Self {
            directory: Some(directory),
            dictionary,
            history,
            history_preferences,
            pending_history_preferences: Vec::new(),
            context_history,
            history_is_writable,
            history_preferences_are_writable,
            context_history_is_writable,
        }
    }

    pub fn reload(&mut self) {
        let Some(directory) = self.directory.clone() else {
            return;
        };
        *self = Self::load(directory);
    }

    pub(crate) fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    pub fn exact_dictionary_surfaces(&self, reading: &str) -> impl Iterator<Item = &str> {
        self.dictionary
            .iter()
            .filter(move |entry| entry.reading == reading)
            .map(|entry| entry.surface.as_str())
    }

    #[must_use]
    pub(crate) fn contextual_history_surfaces(
        &self,
        previous_reading: &str,
        previous_surface: &str,
        reading: &str,
    ) -> Vec<&str> {
        let mut entries: Vec<_> = self
            .context_history
            .iter()
            .filter(|entry| {
                entry.previous_reading == previous_reading
                    && entry.previous_surface == previous_surface
                    && entry.reading == reading
                    && entry.count >= MIN_CONTEXT_USE_COUNT
                    && is_useful_history(&entry.reading, &entry.surface)
            })
            .collect();
        sort_context_history(&mut entries);
        entries
            .into_iter()
            .map(|entry| entry.surface.as_str())
            .collect()
    }

    /// Returns repeated contextual selections whose previous surface is a
    /// meaningful suffix of text supplied by the input client. The external
    /// text has no reading, so require at least a two-character surface anchor
    /// to avoid broad matches such as `人` matching `本人`.
    #[must_use]
    pub(crate) fn contextual_history_surfaces_for_external_surface(
        &self,
        external_surface: &str,
        reading: &str,
    ) -> Vec<&str> {
        let mut entries: Vec<_> = self
            .context_history
            .iter()
            .filter(|entry| {
                entry.previous_surface.chars().count() >= 2
                    && external_surface.ends_with(&entry.previous_surface)
                    && entry.reading == reading
                    && entry.count >= MIN_CONTEXT_USE_COUNT
                    && is_useful_context_anchor(&entry.previous_reading, &entry.previous_surface)
                    && is_useful_history(&entry.reading, &entry.surface)
            })
            .collect();
        sort_context_history(&mut entries);

        let mut surfaces = Vec::with_capacity(entries.len());
        for entry in entries {
            if !surfaces.contains(&entry.surface.as_str()) {
                surfaces.push(entry.surface.as_str());
            }
        }
        surfaces
    }

    #[must_use]
    pub(crate) fn contextual_completion_surfaces(
        &self,
        previous_reading: &str,
        previous_surface: &str,
        prefix: &str,
        limit: usize,
    ) -> Vec<&str> {
        if limit == 0 {
            return Vec::new();
        }
        let prefix_length = prefix.chars().count();
        let mut entries: Vec<_> = self
            .context_history
            .iter()
            .filter(|entry| {
                entry.previous_reading == previous_reading
                    && entry.previous_surface == previous_surface
                    && entry.count >= MIN_CONTEXT_USE_COUNT
                    && entry.reading.starts_with(prefix)
                    && entry.reading.chars().count().saturating_sub(prefix_length)
                        >= MIN_COMPLETION_REMAINING_CHARS
                    && is_useful_history(&entry.reading, &entry.surface)
            })
            .collect();
        sort_context_history(&mut entries);

        let mut surfaces = Vec::with_capacity(limit);
        for entry in entries {
            if !surfaces.contains(&entry.surface.as_str()) {
                surfaces.push(entry.surface.as_str());
            }
            if surfaces.len() == limit {
                break;
            }
        }
        surfaces
    }

    /// Returns repeated completions whose learned previous surface is a
    /// meaningful suffix of text supplied by the input client. External text
    /// has no reading, so keep the same two-character anchor requirement as
    /// exact contextual conversion history.
    #[must_use]
    pub(crate) fn contextual_completion_surfaces_for_external_surface(
        &self,
        external_surface: &str,
        prefix: &str,
        limit: usize,
    ) -> Vec<&str> {
        if limit == 0 {
            return Vec::new();
        }
        let prefix_length = prefix.chars().count();
        let mut entries: Vec<_> = self
            .context_history
            .iter()
            .filter(|entry| {
                entry.previous_surface.chars().count() >= 2
                    && external_surface.ends_with(&entry.previous_surface)
                    && entry.count >= MIN_CONTEXT_USE_COUNT
                    && entry.reading.starts_with(prefix)
                    && entry.reading.chars().count().saturating_sub(prefix_length)
                        >= MIN_COMPLETION_REMAINING_CHARS
                    && is_useful_context_anchor(&entry.previous_reading, &entry.previous_surface)
                    && is_useful_history(&entry.reading, &entry.surface)
            })
            .collect();
        sort_context_history(&mut entries);

        let mut surfaces = Vec::with_capacity(limit);
        for entry in entries {
            if !surfaces.contains(&entry.surface.as_str()) {
                surfaces.push(entry.surface.as_str());
            }
            if surfaces.len() == limit {
                break;
            }
        }
        surfaces
    }

    pub fn dictionary_entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.dictionary
            .iter()
            .map(|entry| (entry.reading.as_str(), entry.surface.as_str()))
    }

    #[must_use]
    pub fn exact_history_surfaces(&self, reading: &str) -> Vec<&str> {
        let (established, transient) = self.exact_history_surfaces_by_strength(reading);
        established.into_iter().chain(transient).collect()
    }

    #[must_use]
    pub(crate) fn exact_history_surfaces_by_strength(
        &self,
        reading: &str,
    ) -> (Vec<&str>, Vec<&str>) {
        let preferred_surface = self.preferred_history_surface(reading);
        let mut entries: Vec<_> = self
            .history
            .iter()
            .filter(|entry| {
                entry.reading == reading && is_useful_history(&entry.reading, &entry.surface)
            })
            .collect();
        sort_history(&mut entries, preferred_surface);
        let established_count = entries
            .iter()
            .take_while(|entry| {
                preferred_surface == Some(entry.surface.as_str()) || history_strength(entry)
            })
            .count();
        let (established, transient) = entries.split_at(established_count);
        (
            established
                .iter()
                .map(|entry| entry.surface.as_str())
                .collect(),
            transient
                .iter()
                .map(|entry| entry.surface.as_str())
                .collect(),
        )
    }

    fn preferred_history_surface(&self, reading: &str) -> Option<&str> {
        if let Some(preference) = self
            .history_preferences
            .iter()
            .find(|preference| preference.reading == reading)
            && self.history.iter().any(|entry| {
                entry.reading == reading
                    && entry.surface == preference.surface
                    && is_useful_history(&entry.reading, &entry.surface)
            })
        {
            return Some(preference.surface.as_str());
        }

        let mut established: Vec<_> = self
            .history
            .iter()
            .filter(|entry| {
                entry.reading == reading
                    && history_strength(entry)
                    && is_useful_history(&entry.reading, &entry.surface)
            })
            .collect();
        sort_history(&mut established, None);
        established.first().map(|entry| entry.surface.as_str())
    }

    /// Require the same alternative twice before changing an existing durable
    /// preference. The first conflicting selection freezes the current winner
    /// in a small sidecar; the second confirms and replaces it. Raw frequency
    /// remains in history.tsv, so two confirmations do not unlock prediction.
    fn confirm_history_preference(
        &mut self,
        reading: &str,
        surface: &str,
        context: Option<(&str, &str)>,
        last_used: u64,
    ) -> Option<String> {
        let Some(current) = self.preferred_history_surface(reading).map(str::to_owned) else {
            remove_pending_history_preference(&mut self.pending_history_preferences, reading);
            return None;
        };
        if current == surface {
            remove_pending_history_preference(&mut self.pending_history_preferences, reading);
            return None;
        }

        let mut preference_to_persist = None;
        let current_is_explicit = self
            .history_preferences
            .iter()
            .any(|preference| preference.reading == reading && preference.surface == current);
        if !current_is_explicit {
            update_history_preference(&mut self.history_preferences, reading, &current, last_used);
            trim_history_preferences(&mut self.history_preferences);
            preference_to_persist = Some(current);
        }

        if let Some(pending) = self
            .pending_history_preferences
            .iter()
            .find(|pending| pending.reading == reading)
            && pending.surface == surface
            && (context.is_none() || !pending_history_context_matches(pending, context))
        {
            update_history_preference(&mut self.history_preferences, reading, surface, last_used);
            trim_history_preferences(&mut self.history_preferences);
            remove_pending_history_preference(&mut self.pending_history_preferences, reading);
            return Some(surface.to_owned());
        }

        update_pending_history_preference(
            &mut self.pending_history_preferences,
            reading,
            surface,
            context,
        );
        preference_to_persist
    }

    #[must_use]
    pub fn completion_surfaces(&self, prefix: &str, limit: usize) -> Vec<String> {
        let prefix_length = prefix.chars().count();
        let mut entries: Vec<_> = self
            .history
            .iter()
            .filter(|entry| {
                is_useful_history(&entry.reading, &entry.surface)
                    && entry.count >= MIN_COMPLETION_USE_COUNT
                    && entry.reading.starts_with(prefix)
                    && entry.reading.chars().count().saturating_sub(prefix_length)
                        >= MIN_COMPLETION_REMAINING_CHARS
            })
            .collect();
        sort_completions(&mut entries);

        let mut surfaces = Vec::with_capacity(limit);
        for entry in entries {
            if !surfaces.contains(&entry.surface) {
                surfaces.push(entry.surface.clone());
            }
            if surfaces.len() == limit {
                break;
            }
        }
        surfaces
    }

    pub fn promote_completion(&mut self, prefix: &str, surface: &str) -> Option<String> {
        let mut entries: Vec<_> = self
            .history
            .iter()
            .filter(|entry| {
                is_useful_history(&entry.reading, &entry.surface)
                    && entry.count >= MIN_COMPLETION_USE_COUNT
                    && entry.reading.starts_with(prefix)
                    && entry.reading != prefix
                    && entry.surface == surface
            })
            .collect();
        sort_completions(&mut entries);
        let reading = entries.first().map(|entry| entry.reading.clone())?;

        self.record(&reading, surface);
        Some(reading)
    }

    pub fn record(&mut self, reading: &str, surface: &str) {
        self.record_with_preference_context(reading, surface, None);
    }

    pub(crate) fn record_with_preference_context(
        &mut self,
        reading: &str,
        surface: &str,
        context: Option<(&str, &str)>,
    ) {
        if !is_useful_history(reading, surface) {
            return;
        }

        let wall_clock = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let now = next_last_used(&self.history, wall_clock);
        let preference_to_persist = self.confirm_history_preference(reading, surface, context, now);
        update_history(&mut self.history, reading, surface, now);
        trim_history(&mut self.history);

        let Some(directory) = &self.directory else {
            return;
        };
        if !self.history_is_writable {
            return;
        }

        let path = directory.join(HISTORY_FILE);
        let history_saved = write_history_optimistically(&path, reading, surface, now).is_ok();
        if history_saved
            && let Ok(Some(bytes)) = read_optional(&path)
            && let Ok(history) = parse_history(&bytes)
        {
            self.history = history;
        }
        if history_saved
            && let Some(preferred_surface) = preference_to_persist
            && self.history_preferences_are_writable
        {
            let path = directory.join(HISTORY_PREFERENCES_FILE);
            if write_history_preference_optimistically(&path, reading, &preferred_surface, now)
                .is_ok()
                && let Ok(Some(bytes)) = read_optional(&path)
                && let Ok(preferences) = parse_history_preferences(&bytes)
            {
                self.history_preferences = preferences;
            }
        }
    }

    pub(crate) fn record_context(
        &mut self,
        previous_reading: &str,
        previous_surface: &str,
        reading: &str,
        surface: &str,
    ) {
        self.record_contexts([(previous_reading, previous_surface, reading, surface)]);
    }

    /// Records all useful edges from one confirmed conversion with one
    /// optimistic file update. A long conversion may contain several useful
    /// segments; persisting them separately would repeatedly parse and replace
    /// the same history file on the commit path.
    pub(crate) fn record_contexts<'a>(
        &mut self,
        contexts: impl IntoIterator<Item = (&'a str, &'a str, &'a str, &'a str)>,
    ) {
        let contexts = contexts
            .into_iter()
            .filter(|(previous_reading, previous_surface, reading, surface)| {
                is_useful_context_anchor(previous_reading, previous_surface)
                    && is_useful_history(reading, surface)
            })
            .map(|(previous_reading, previous_surface, reading, surface)| {
                (
                    previous_reading.to_owned(),
                    previous_surface.to_owned(),
                    reading.to_owned(),
                    surface.to_owned(),
                )
            })
            .collect::<Vec<_>>();
        let mut unique_contexts = Vec::with_capacity(contexts.len());
        for context in contexts {
            if !unique_contexts.contains(&context) {
                unique_contexts.push(context);
            }
        }
        let contexts = unique_contexts;
        if contexts.is_empty() {
            return;
        }

        let wall_clock = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        for (previous_reading, previous_surface, reading, surface) in &contexts {
            let now = next_context_last_used(&self.context_history, wall_clock);
            update_context_history(
                &mut self.context_history,
                previous_reading,
                previous_surface,
                reading,
                surface,
                now,
            );
        }
        trim_context_history(&mut self.context_history);

        let Some(directory) = &self.directory else {
            return;
        };
        if !self.context_history_is_writable {
            return;
        }

        let path = directory.join(CONTEXT_HISTORY_FILE);
        if write_context_history_optimistically(&path, &contexts, wall_clock).is_ok()
            && let Ok(Some(bytes)) = read_optional(&path)
            && let Ok(history) = parse_context_history(&bytes)
        {
            self.context_history = history;
        }
    }
}

fn sort_completions(entries: &mut Vec<&HistoryEntry>) {
    entries.sort_unstable_by(|left, right| {
        right
            .last_used
            .cmp(&left.last_used)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.surface.cmp(&right.surface))
    });
}

fn sort_history(entries: &mut Vec<&HistoryEntry>, preferred_surface: Option<&str>) {
    entries.sort_unstable_by(|left, right| {
        (preferred_surface == Some(right.surface.as_str()))
            .cmp(&(preferred_surface == Some(left.surface.as_str())))
            .then_with(|| history_strength(right).cmp(&history_strength(left)))
            .then_with(|| right.last_used.cmp(&left.last_used))
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.surface.cmp(&right.surface))
    });
}

fn sort_context_history(entries: &mut Vec<&ContextHistoryEntry>) {
    entries.sort_unstable_by(|left, right| {
        context_history_strength(right)
            .cmp(&context_history_strength(left))
            .then_with(|| right.last_used.cmp(&left.last_used))
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.surface.cmp(&right.surface))
    });
}

/// A single exceptional selection should not replace an established spelling.
/// Without a confirmed preference, recency decides between established rows;
/// the sidecar confirmation layer above freezes deliberate preference changes.
fn history_strength(entry: &HistoryEntry) -> bool {
    entry.count >= MIN_ESTABLISHED_HISTORY_COUNT
}

fn update_history_preference(
    preferences: &mut Vec<HistoryPreferenceEntry>,
    reading: &str,
    surface: &str,
    last_used: u64,
) {
    if let Some(preference) = preferences
        .iter_mut()
        .find(|preference| preference.reading == reading)
    {
        surface.clone_into(&mut preference.surface);
        preference.last_used = last_used;
    } else {
        preferences.push(HistoryPreferenceEntry {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            last_used,
        });
    }
}

fn trim_history_preferences(preferences: &mut Vec<HistoryPreferenceEntry>) {
    preferences.sort_unstable_by(|left, right| {
        right
            .last_used
            .cmp(&left.last_used)
            .then_with(|| left.reading.cmp(&right.reading))
    });
    preferences.truncate(MAX_HISTORY_PREFERENCES);
}

fn update_pending_history_preference(
    preferences: &mut Vec<PendingHistoryPreference>,
    reading: &str,
    surface: &str,
    context: Option<(&str, &str)>,
) {
    let context = context.map(
        |(previous_reading, previous_surface)| HistoryPreferenceContext {
            previous_reading: previous_reading.to_owned(),
            previous_surface: previous_surface.to_owned(),
        },
    );
    if let Some(preference) = preferences
        .iter_mut()
        .find(|preference| preference.reading == reading)
    {
        surface.clone_into(&mut preference.surface);
        preference.context = context;
    } else {
        preferences.push(PendingHistoryPreference {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            context,
        });
        if preferences.len() > MAX_HISTORY_PREFERENCES {
            preferences.remove(0);
        }
    }
}

fn pending_history_context_matches(
    pending: &PendingHistoryPreference,
    context: Option<(&str, &str)>,
) -> bool {
    match (&pending.context, context) {
        (None, None) => true,
        (Some(pending), Some((previous_reading, previous_surface))) => {
            pending.previous_reading == previous_reading
                && pending.previous_surface == previous_surface
        }
        _ => false,
    }
}

fn remove_pending_history_preference(
    preferences: &mut Vec<PendingHistoryPreference>,
    reading: &str,
) {
    if let Some(index) = preferences
        .iter()
        .position(|preference| preference.reading == reading)
    {
        preferences.swap_remove(index);
    }
}

fn context_history_strength(entry: &ContextHistoryEntry) -> bool {
    entry.count >= MIN_ESTABLISHED_CONTEXT_COUNT
}

fn update_history(history: &mut Vec<HistoryEntry>, reading: &str, surface: &str, last_used: u64) {
    if let Some(entry) = history
        .iter_mut()
        .find(|entry| entry.reading == reading && entry.surface == surface)
    {
        entry.count = entry.count.saturating_add(1);
        entry.last_used = last_used;
    } else {
        history.push(HistoryEntry {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            count: 1,
            last_used,
        });
    }
}

fn update_context_history(
    history: &mut Vec<ContextHistoryEntry>,
    previous_reading: &str,
    previous_surface: &str,
    reading: &str,
    surface: &str,
    last_used: u64,
) {
    if let Some(entry) = history.iter_mut().find(|entry| {
        entry.previous_reading == previous_reading
            && entry.previous_surface == previous_surface
            && entry.reading == reading
            && entry.surface == surface
    }) {
        entry.count = entry.count.saturating_add(1);
        entry.last_used = last_used;
    } else {
        history.push(ContextHistoryEntry {
            previous_reading: previous_reading.to_owned(),
            previous_surface: previous_surface.to_owned(),
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            count: 1,
            last_used,
        });
    }
}

fn next_last_used(history: &[HistoryEntry], wall_clock: u64) -> u64 {
    history
        .iter()
        .map(|entry| entry.last_used)
        .max()
        .map_or(wall_clock, |latest| {
            wall_clock.max(latest.saturating_add(1))
        })
}

fn next_context_last_used(history: &[ContextHistoryEntry], wall_clock: u64) -> u64 {
    history
        .iter()
        .map(|entry| entry.last_used)
        .max()
        .map_or(wall_clock, |latest| {
            wall_clock.max(latest.saturating_add(1))
        })
}

fn trim_history(history: &mut Vec<HistoryEntry>) {
    history.sort_unstable_by(|left, right| {
        is_useful_history(&right.reading, &right.surface)
            .cmp(&is_useful_history(&left.reading, &left.surface))
            .then_with(|| {
                right
                    .last_used
                    .cmp(&left.last_used)
                    .then_with(|| right.count.cmp(&left.count))
            })
    });
    history.truncate(MAX_HISTORY_ENTRIES);
}

fn trim_context_history(history: &mut Vec<ContextHistoryEntry>) {
    history.sort_unstable_by(|left, right| {
        is_useful_context_anchor(&right.previous_reading, &right.previous_surface)
            .cmp(&is_useful_context_anchor(
                &left.previous_reading,
                &left.previous_surface,
            ))
            .then_with(|| {
                is_useful_history(&right.reading, &right.surface)
                    .cmp(&is_useful_history(&left.reading, &left.surface))
            })
            .then_with(|| {
                right
                    .last_used
                    .cmp(&left.last_used)
                    .then_with(|| right.count.cmp(&left.count))
            })
    });
    history.truncate(MAX_CONTEXT_HISTORY_ENTRIES);
}

pub(crate) fn is_useful_history(reading: &str, surface: &str) -> bool {
    let reading_length = reading.chars().count();
    let surface_length = surface.chars().count();
    (3..=MAX_HISTORY_READING_CHARS).contains(&reading_length)
        && (2..=MAX_HISTORY_SURFACE_CHARS).contains(&surface_length)
        && reading != surface
        && reading
            .chars()
            .any(|character| matches!(character, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}'))
}

/// A committed word may be valuable as the left side of a context edge even
/// when it is too short to retain as a global conversion preference. The
/// selected surface disambiguates the anchor, while equality and script checks
/// continue to reject literal kana, punctuation, and raw ASCII input.
pub(crate) fn is_useful_context_anchor(reading: &str, surface: &str) -> bool {
    let reading_length = reading.chars().count();
    let surface_length = surface.chars().count();
    (1..=MAX_HISTORY_READING_CHARS).contains(&reading_length)
        && (1..=MAX_HISTORY_SURFACE_CHARS).contains(&surface_length)
        && reading != surface
        && reading
            .chars()
            .any(|character| matches!(character, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}'))
}

fn parse_user_dictionary(bytes: &[u8]) -> Result<Vec<UserDictionaryEntry>, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line == USER_DICTIONARY_HEADER {
            continue;
        }
        let (reading, surface) = line.split_once('\t').ok_or(())?;
        if reading.is_empty() || surface.is_empty() || surface.contains('\t') {
            return Err(());
        }
        entries.push(UserDictionaryEntry {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
        });
    }
    Ok(entries)
}

fn parse_history(bytes: &[u8]) -> Result<Vec<HistoryEntry>, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line == HISTORY_HEADER {
            continue;
        }
        let mut columns = line.split('\t');
        let reading = columns.next().ok_or(())?;
        let surface = columns.next().ok_or(())?;
        let count = columns.next().ok_or(())?.parse().map_err(|_| ())?;
        let last_used = columns.next().ok_or(())?.parse().map_err(|_| ())?;
        if reading.is_empty() || surface.is_empty() || columns.next().is_some() {
            return Err(());
        }
        entries.push(HistoryEntry {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            count,
            last_used,
        });
    }
    Ok(entries)
}

fn parse_history_preferences(bytes: &[u8]) -> Result<Vec<HistoryPreferenceEntry>, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line == HISTORY_PREFERENCES_HEADER {
            continue;
        }
        let mut columns = line.split('\t');
        let reading = columns.next().ok_or(())?;
        let surface = columns.next().ok_or(())?;
        let last_used = columns.next().ok_or(())?.parse().map_err(|_| ())?;
        if reading.is_empty()
            || surface.is_empty()
            || columns.next().is_some()
            || entries.len() == MAX_HISTORY_PREFERENCES
            || entries
                .iter()
                .any(|entry: &HistoryPreferenceEntry| entry.reading == reading)
        {
            return Err(());
        }
        entries.push(HistoryPreferenceEntry {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            last_used,
        });
    }
    Ok(entries)
}

fn parse_context_history(bytes: &[u8]) -> Result<Vec<ContextHistoryEntry>, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line == CONTEXT_HISTORY_HEADER {
            continue;
        }
        let mut columns = line.split('\t');
        let previous_reading = columns.next().ok_or(())?;
        let previous_surface = columns.next().ok_or(())?;
        let reading = columns.next().ok_or(())?;
        let surface = columns.next().ok_or(())?;
        let count = columns.next().ok_or(())?.parse().map_err(|_| ())?;
        let last_used = columns.next().ok_or(())?.parse().map_err(|_| ())?;
        if previous_reading.is_empty()
            || previous_surface.is_empty()
            || reading.is_empty()
            || surface.is_empty()
            || columns.next().is_some()
        {
            return Err(());
        }
        entries.push(ContextHistoryEntry {
            previous_reading: previous_reading.to_owned(),
            previous_surface: previous_surface.to_owned(),
            reading: reading.to_owned(),
            surface: surface.to_owned(),
            count,
            last_used,
        });
    }
    Ok(entries)
}

fn serialize_history(history: &[HistoryEntry]) -> Vec<u8> {
    let mut output = String::from(HISTORY_HEADER);
    output.push('\n');
    for entry in history {
        output.push_str(&entry.reading);
        output.push('\t');
        output.push_str(&entry.surface);
        output.push('\t');
        output.push_str(&entry.count.to_string());
        output.push('\t');
        output.push_str(&entry.last_used.to_string());
        output.push('\n');
    }
    output.into_bytes()
}

fn serialize_history_preferences(preferences: &[HistoryPreferenceEntry]) -> Vec<u8> {
    let mut output = String::from(HISTORY_PREFERENCES_HEADER);
    output.push('\n');
    for preference in preferences {
        output.push_str(&preference.reading);
        output.push('\t');
        output.push_str(&preference.surface);
        output.push('\t');
        output.push_str(&preference.last_used.to_string());
        output.push('\n');
    }
    output.into_bytes()
}

fn serialize_context_history(history: &[ContextHistoryEntry]) -> Vec<u8> {
    let mut output = String::from(CONTEXT_HISTORY_HEADER);
    output.push('\n');
    for entry in history {
        output.push_str(&entry.previous_reading);
        output.push('\t');
        output.push_str(&entry.previous_surface);
        output.push('\t');
        output.push_str(&entry.reading);
        output.push('\t');
        output.push_str(&entry.surface);
        output.push('\t');
        output.push_str(&entry.count.to_string());
        output.push('\t');
        output.push_str(&entry.last_used.to_string());
        output.push('\n');
    }
    output.into_bytes()
}

fn write_history_optimistically(
    path: &Path,
    reading: &str,
    surface: &str,
    last_used: u64,
) -> io::Result<()> {
    for _ in 0..3 {
        let base = read_optional(path)?;
        let mut history = match base.as_deref() {
            Some(bytes) => parse_history(bytes)
                .map_err(|()| io::Error::new(io::ErrorKind::InvalidData, "malformed history"))?,
            None => Vec::new(),
        };
        let last_used = next_last_used(&history, last_used);
        update_history(&mut history, reading, surface, last_used);
        trim_history(&mut history);
        let proposed = serialize_history(&history);
        if atomic_replace_if_unchanged(path, base.as_deref(), &proposed)? {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "history changed while saving",
    ))
}

fn write_history_preference_optimistically(
    path: &Path,
    reading: &str,
    surface: &str,
    last_used: u64,
) -> io::Result<()> {
    for _ in 0..3 {
        let base = read_optional(path)?;
        let mut preferences = match base.as_deref() {
            Some(bytes) => parse_history_preferences(bytes).map_err(|()| {
                io::Error::new(io::ErrorKind::InvalidData, "malformed history preferences")
            })?,
            None => Vec::new(),
        };
        let last_used = preferences
            .iter()
            .map(|preference| preference.last_used)
            .max()
            .map_or(last_used, |latest| last_used.max(latest.saturating_add(1)));
        update_history_preference(&mut preferences, reading, surface, last_used);
        trim_history_preferences(&mut preferences);
        let proposed = serialize_history_preferences(&preferences);
        if atomic_replace_if_unchanged(path, base.as_deref(), &proposed)? {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "history preferences changed while saving",
    ))
}

fn write_context_history_optimistically(
    path: &Path,
    contexts: &[(String, String, String, String)],
    last_used: u64,
) -> io::Result<()> {
    for _ in 0..3 {
        let base = read_optional(path)?;
        let mut history = match base.as_deref() {
            Some(bytes) => parse_context_history(bytes).map_err(|()| {
                io::Error::new(io::ErrorKind::InvalidData, "malformed context history")
            })?,
            None => Vec::new(),
        };
        for (previous_reading, previous_surface, reading, surface) in contexts {
            let last_used = next_context_last_used(&history, last_used);
            update_context_history(
                &mut history,
                previous_reading,
                previous_surface,
                reading,
                surface,
                last_used,
            );
        }
        trim_context_history(&mut history);
        let proposed = serialize_context_history(&history);
        if atomic_replace_if_unchanged(path, base.as_deref(), &proposed)? {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "context history changed while saving",
    ))
}

fn atomic_replace_if_unchanged(
    path: &Path,
    base: Option<&[u8]>,
    proposed: &[u8],
) -> io::Result<bool> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent directory"))?;
    fs::create_dir_all(parent)?;

    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("user-data");
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    if let Err(error) = file.write_all(proposed).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    drop(file);

    if read_optional(path)?.as_deref() != base {
        fs::remove_file(&temporary)?;
        return Ok(false);
    }

    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(true)
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONTEXT_HISTORY_HEADER, HISTORY_FILE, HISTORY_HEADER, HISTORY_PREFERENCES_FILE,
        HISTORY_PREFERENCES_HEADER, USER_DICTIONARY_HEADER, UserData, atomic_replace_if_unchanged,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_directory(name: &str) -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("slime-{name}-{}-{counter}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn loads_dictionary_and_ranks_history_completions() {
        let directory = test_directory("load");
        fs::write(
            directory.join("user_dictionary.tsv"),
            format!("{USER_DICTIONARY_HEADER}\nほげ\tHOGE\n"),
        )
        .unwrap();
        fs::write(
            directory.join("history.tsv"),
            format!(
                "{HISTORY_HEADER}\nぱふぉーまんす\tパフォーマンス\t8\t10\nぱそこん\tパソコン\t5\t20\n"
            ),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(
            data.exact_dictionary_surfaces("ほげ").collect::<Vec<_>>(),
            ["HOGE"]
        );
        assert_eq!(
            data.completion_surfaces("ぱ", 5),
            ["パソコン", "パフォーマンス"]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recording_history_persists_and_reloads() {
        let directory = test_directory("record");
        let mut data = UserData::load(&directory);
        data.record("にほん", "日本");
        data.record("にほん", "日本");

        let reloaded = UserData::load(&directory);
        assert_eq!(reloaded.exact_history_surfaces("にほん"), ["日本"]);
        let bytes = fs::read(directory.join("history.tsv")).unwrap();
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("にほん\t日本\t2\t")
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn contextual_history_requires_repetition_and_persists() {
        let directory = test_directory("context-history");
        let mut data = UserData::load(&directory);

        data.record_context("ぶんしょう", "文章", "かんじ", "漢字");
        assert!(
            data.contextual_history_surfaces("ぶんしょう", "文章", "かんじ")
                .is_empty()
        );

        data.record_context("ぶんしょう", "文章", "かんじ", "漢字");
        assert_eq!(
            data.contextual_history_surfaces("ぶんしょう", "文章", "かんじ"),
            ["漢字"]
        );

        let reloaded = UserData::load(&directory);
        assert_eq!(
            reloaded.contextual_history_surfaces("ぶんしょう", "文章", "かんじ"),
            ["漢字"]
        );
        let context = fs::read_to_string(directory.join("context_history.tsv")).unwrap();
        assert!(context.starts_with(CONTEXT_HISTORY_HEADER));
        assert!(context.contains("ぶんしょう\t文章\tかんじ\t漢字\t2\t"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn one_confirmed_conversion_counts_a_repeated_edge_once() {
        let directory = test_directory("context-history-batch-deduplication");
        let mut data = UserData::load(&directory);

        data.record_contexts([
            ("へや", "部屋", "しょうめい", "照明"),
            ("へや", "部屋", "しょうめい", "照明"),
        ]);

        assert!(
            data.contextual_history_surfaces("へや", "部屋", "しょうめい")
                .is_empty(),
            "one conversion must not satisfy the repetition gate"
        );
        let context = fs::read_to_string(directory.join("context_history.tsv")).unwrap();
        assert!(context.contains("へや\t部屋\tしょうめい\t照明\t1\t"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn external_surface_context_requires_a_specific_repeated_suffix() {
        let directory = test_directory("external-surface-context");
        fs::write(
            directory.join("context_history.tsv"),
            format!(
                "{CONTEXT_HISTORY_HEADER}\n\
                 ひと\t人\tしょうめい\t証明\t20\t30\n\
                 へや\t部屋\tしょうめい\t照明\t2\t20\n\
                 しつ\t部屋\tしょうめい\t照明\t5\t10\n\
                 ほんにん\t本人\tしょうめい\t証明\t1\t40\n"
            ),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(
            data.contextual_history_surfaces_for_external_surface("既存文書の部屋", "しょうめい"),
            ["照明"]
        );
        assert!(
            data.contextual_history_surfaces_for_external_surface("本人", "しょうめい")
                .is_empty(),
            "a one-character anchor or one-off context must not match"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn short_previous_word_can_anchor_context_without_becoming_plain_history() {
        let directory = test_directory("short-context-anchor");
        let mut data = UserData::load(&directory);

        data.record("へや", "部屋");
        assert!(data.exact_history_surfaces("へや").is_empty());

        for _ in 0..2 {
            data.record_context("へや", "部屋", "しょうめい", "照明");
        }
        assert_eq!(
            data.contextual_history_surfaces("へや", "部屋", "しょうめい"),
            ["照明"]
        );

        let reloaded = UserData::load(&directory);
        assert_eq!(
            reloaded.contextual_history_surfaces("へや", "部屋", "しょうめい"),
            ["照明"]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn established_context_outranks_a_recent_transient_context() {
        let directory = test_directory("context-strength");
        fs::write(
            directory.join("context_history.tsv"),
            format!(
                "{CONTEXT_HISTORY_HEADER}\nぶんしょう\t文章\tかんじ\t漢字\t100\t10\nぶんしょう\t文章\tかんじ\t感じ\t2\t20\n"
            ),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(
            data.contextual_history_surfaces("ぶんしょう", "文章", "かんじ"),
            ["漢字", "感じ"]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recency_decides_between_established_contexts() {
        let directory = test_directory("established-context-recency");
        fs::write(
            directory.join("context_history.tsv"),
            format!(
                "{CONTEXT_HISTORY_HEADER}\nぶんしょう\t文章\tかんじ\t漢字\t100\t10\nぶんしょう\t文章\tかんじ\t感じ\t5\t20\n"
            ),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(
            data.contextual_history_surfaces("ぶんしょう", "文章", "かんじ"),
            ["感じ", "漢字"]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn contextual_completion_requires_repetition_and_survives_reload() {
        let directory = test_directory("context-completion");
        let mut data = UserData::load(&directory);
        for _ in 0..2 {
            data.record_context("ぶんしょう", "文章", "かんじへんかん", "漢字変換");
        }

        let reloaded = UserData::load(&directory);
        assert_eq!(
            reloaded.contextual_completion_surfaces("ぶんしょう", "文章", "かんじ", 9),
            ["漢字変換"]
        );
        assert!(
            reloaded
                .contextual_completion_surfaces("ぶんしょう", "文章", "かんじへんか", 9)
                .is_empty()
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn external_surface_context_can_anchor_a_repeated_completion() {
        let directory = test_directory("external-context-completion");
        fs::write(
            directory.join("context_history.tsv"),
            format!(
                "{CONTEXT_HISTORY_HEADER}\n\
                 ひと\t人\tしょうめいけいかく\t証明計画\t20\t30\n\
                 へや\t部屋\tしょうめいけいかく\t照明計画\t2\t20\n\
                 ほんにん\t本人\tしょうめいけいかく\t証明計画\t1\t40\n"
            ),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(
            data.contextual_completion_surfaces_for_external_surface(
                "既存文書の部屋",
                "しょうめい",
                9,
            ),
            ["照明計画"]
        );
        assert!(
            data.contextual_completion_surfaces_for_external_surface("本人", "しょうめい", 9)
                .is_empty(),
            "a one-character anchor or one-off context must not predict"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_established_selection_outranks_a_recent_one_off_selection() {
        let directory = test_directory("learning-strength");
        fs::write(
            directory.join("history.tsv"),
            format!("{HISTORY_HEADER}\nかんじ\t漢字\t100\t10\nかんじ\t感じ\t1\t20\n"),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(data.exact_history_surfaces("かんじ"), ["漢字", "感じ"]);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn history_surfaces_are_partitioned_without_changing_their_rank() {
        let directory = test_directory("partitioned-learning-strength");
        fs::write(
            directory.join("history.tsv"),
            format!(
                "{HISTORY_HEADER}\nかんじ\t漢字\t100\t10\nかんじ\t感じ\t1\t30\nかんじ\t幹事\t1\t20\n"
            ),
        )
        .unwrap();

        let data = UserData::load(&directory);
        let (established, transient) = data.exact_history_surfaces_by_strength("かんじ");
        assert_eq!(established, ["漢字"]);
        assert_eq!(transient, ["感じ", "幹事"]);
        assert_eq!(
            data.exact_history_surfaces("かんじ"),
            ["漢字", "感じ", "幹事"]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recency_decides_between_established_selections() {
        let directory = test_directory("established-recency");
        fs::write(
            directory.join("history.tsv"),
            format!("{HISTORY_HEADER}\nかんじ\t漢字\t100\t10\nかんじ\t感じ\t5\t20\n"),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(data.exact_history_surfaces("かんじ"), ["感じ", "漢字"]);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_alternative_switches_preference_without_unlocking_completion() {
        let directory = test_directory("confirmed-preference");
        fs::write(
            directory.join(HISTORY_FILE),
            format!("{HISTORY_HEADER}\nかんじ\t漢字\t100\t10\n"),
        )
        .unwrap();
        let mut data = UserData::load(&directory);

        data.record("かんじ", "感じ");
        assert_eq!(data.exact_history_surfaces("かんじ"), ["漢字", "感じ"]);
        assert_eq!(data.completion_surfaces("か", 5), ["漢字"]);
        let frozen = fs::read_to_string(directory.join(HISTORY_PREFERENCES_FILE)).unwrap();
        assert!(frozen.contains("かんじ\t漢字\t"));

        data.record("かんじ", "感じ");
        assert_eq!(data.exact_history_surfaces("かんじ"), ["感じ", "漢字"]);
        assert_eq!(
            data.completion_surfaces("か", 5),
            ["漢字"],
            "two confirmations must not bypass the five-use completion gate"
        );

        let reloaded = UserData::load(&directory);
        assert_eq!(reloaded.exact_history_surfaces("かんじ"), ["感じ", "漢字"]);
        let confirmed = fs::read_to_string(directory.join(HISTORY_PREFERENCES_FILE)).unwrap();
        assert!(confirmed.contains("かんじ\t感じ\t"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn one_off_selection_does_not_revert_a_confirmed_preference() {
        let directory = test_directory("confirmed-preference-revert");
        fs::write(
            directory.join(HISTORY_FILE),
            format!("{HISTORY_HEADER}\nかんじ\t漢字\t100\t10\nかんじ\t感じ\t2\t20\n"),
        )
        .unwrap();
        fs::write(
            directory.join(HISTORY_PREFERENCES_FILE),
            format!("{HISTORY_PREFERENCES_HEADER}\nかんじ\t感じ\t20\n"),
        )
        .unwrap();
        let mut data = UserData::load(&directory);

        data.record("かんじ", "漢字");
        assert_eq!(data.exact_history_surfaces("かんじ"), ["感じ", "漢字"]);
        data.record("かんじ", "漢字");
        assert_eq!(data.exact_history_surfaces("かんじ"), ["漢字", "感じ"]);

        let reloaded = UserData::load(&directory);
        assert_eq!(reloaded.exact_history_surfaces("かんじ"), ["漢字", "感じ"]);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stale_history_preference_is_ignored() {
        let directory = test_directory("stale-preference");
        fs::write(
            directory.join(HISTORY_FILE),
            format!("{HISTORY_HEADER}\nかんじ\t漢字\t5\t10\n"),
        )
        .unwrap();
        fs::write(
            directory.join(HISTORY_PREFERENCES_FILE),
            format!("{HISTORY_PREFERENCES_HEADER}\nかんじ\t感じ\t20\n"),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(data.exact_history_surfaces("かんじ"), ["漢字"]);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn promoting_completion_updates_full_reading_and_persists_ranking() {
        let directory = test_directory("promote-completion");
        fs::write(
            directory.join("history.tsv"),
            format!(
                "{HISTORY_HEADER}\nぱふぉーまんす\tパフォーマンス\t5\t20\nぱそこん\tパソコン\t5\t10\n"
            ),
        )
        .unwrap();
        let mut data = UserData::load(&directory);

        assert_eq!(
            data.promote_completion("ぱ", "パソコン"),
            Some("ぱそこん".to_owned())
        );

        let reloaded = UserData::load(&directory);
        assert_eq!(
            reloaded.completion_surfaces("ぱ", 5),
            ["パソコン", "パフォーマンス"]
        );
        assert!(reloaded.exact_history_surfaces("ぱ").is_empty());
        let history = fs::read_to_string(directory.join("history.tsv")).unwrap();
        assert!(history.contains("ぱそこん\tパソコン\t6\t"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completion_requires_at_least_five_uses() {
        let directory = test_directory("completion-minimum-count");
        fs::write(
            directory.join("history.tsv"),
            format!(
                "{HISTORY_HEADER}\nぱふぉーまんす\tパフォーマンス\t4\t20\nぱそこん\tパソコン\t5\t10\n"
            ),
        )
        .unwrap();
        let mut data = UserData::load(&directory);

        assert_eq!(data.completion_surfaces("ぱ", 5), ["パソコン"]);
        assert_eq!(data.promote_completion("ぱふ", "パフォーマンス"), None);
        assert_eq!(
            data.exact_history_surfaces("ぱふぉーまんす"),
            ["パフォーマンス"]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completion_omits_entries_that_save_only_one_character() {
        let directory = test_directory("completion-minimum-saving");
        fs::write(
            directory.join("history.tsv"),
            format!(
                "{HISTORY_HEADER}\nぱふぇ\tパフェ\t6\t20\nぱふぉーまんす\tパフォーマンス\t5\t10\n"
            ),
        )
        .unwrap();
        let data = UserData::load(&directory);

        assert_eq!(data.completion_surfaces("ぱふ", 5), ["パフォーマンス"]);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_low_value_entries_never_affect_candidates() {
        let directory = test_directory("legacy-low-value");
        fs::write(
            directory.join("history.tsv"),
            format!(
                "{HISTORY_HEADER}\nに\t二\t100\t30\nかな\tかな\t100\t20\nnihon\t日本\t100\t10\nにほん\t日本\t5\t1\n"
            ),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert!(data.exact_history_surfaces("に").is_empty());
        assert!(data.exact_history_surfaces("かな").is_empty());
        assert!(data.exact_history_surfaces("nihon").is_empty());
        assert_eq!(data.exact_history_surfaces("にほん"), ["日本"]);
        assert_eq!(data.completion_surfaces("に", 5), ["日本"]);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sentence_sized_entries_are_not_saved_as_completion_history() {
        let directory = test_directory("oversized-history");
        let mut data = UserData::load(&directory);
        let long_reading = "あ".repeat(65);
        let long_surface = "亜".repeat(129);

        data.record(&long_reading, "長すぎる読み");
        data.record("ながすぎるひょうき", &long_surface);

        assert!(!directory.join("history.tsv").exists());
        assert!(data.completion_surfaces("ああ", 5).is_empty());
        assert!(data.exact_history_surfaces("ながすぎるひょうき").is_empty());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_history_is_preserved_instead_of_overwritten() {
        let directory = test_directory("malformed");
        let path = directory.join("history.tsv");
        let malformed = b"not valid history\n";
        fs::write(&path, malformed).unwrap();

        let mut data = UserData::load(&directory);
        data.record("にほん", "日本");

        assert_eq!(fs::read(path).unwrap(), malformed);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_context_history_is_preserved_without_disabling_plain_history() {
        let directory = test_directory("malformed-context");
        let path = directory.join("context_history.tsv");
        let malformed = b"not valid context history\n";
        fs::write(&path, malformed).unwrap();

        let mut data = UserData::load(&directory);
        data.record("にほん", "日本");
        data.record_context("ぶんしょう", "文章", "かんじ", "漢字");

        assert_eq!(fs::read(path).unwrap(), malformed);
        assert!(
            fs::read_to_string(directory.join("history.tsv"))
                .unwrap()
                .contains("にほん\t日本\t1\t")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_history_preferences_are_preserved() {
        let directory = test_directory("malformed-preferences");
        let path = directory.join(HISTORY_PREFERENCES_FILE);
        let malformed = b"not valid history preferences\n";
        fs::write(&path, malformed).unwrap();
        let mut data = UserData::load(&directory);

        for _ in 0..2 {
            data.record("かんじ", "漢字");
        }

        assert_eq!(fs::read(path).unwrap(), malformed);
        assert!(
            fs::read_to_string(directory.join(HISTORY_FILE))
                .unwrap()
                .contains("かんじ\t漢字\t2\t")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_change_prevents_atomic_replacement() {
        let directory = test_directory("conflict");
        let path = directory.join("history.tsv");
        fs::write(&path, b"external").unwrap();

        let replaced = atomic_replace_if_unchanged(&path, Some(b"stale"), b"proposed").unwrap();

        assert!(!replaced);
        assert_eq!(fs::read(path).unwrap(), b"external");
        fs::remove_dir_all(directory).unwrap();
    }
}
