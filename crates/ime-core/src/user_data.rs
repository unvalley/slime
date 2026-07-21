use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const USER_DICTIONARY_FILE: &str = "user_dictionary.tsv";
const HISTORY_FILE: &str = "history.tsv";
const USER_DICTIONARY_HEADER: &str = "# unvalley-ime-user-dictionary-v1";
const HISTORY_HEADER: &str = "# unvalley-ime-history-v1";
const MAX_HISTORY_ENTRIES: usize = 500;
const MIN_COMPLETION_REMAINING_CHARS: usize = 2;
const MIN_COMPLETION_USE_COUNT: u32 = 5;
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

#[derive(Clone, Debug, Default)]
pub struct UserData {
    directory: Option<PathBuf>,
    dictionary: Vec<UserDictionaryEntry>,
    history: Vec<HistoryEntry>,
    history_is_writable: bool,
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

        Self {
            directory: Some(directory),
            dictionary,
            history,
            history_is_writable,
        }
    }

    pub fn reload(&mut self) {
        let Some(directory) = self.directory.clone() else {
            return;
        };
        *self = Self::load(directory);
    }

    pub fn exact_dictionary_surfaces(&self, reading: &str) -> impl Iterator<Item = &str> {
        self.dictionary
            .iter()
            .filter(move |entry| entry.reading == reading)
            .map(|entry| entry.surface.as_str())
    }

    pub fn dictionary_entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.dictionary
            .iter()
            .map(|entry| (entry.reading.as_str(), entry.surface.as_str()))
    }

    #[must_use]
    pub fn exact_history_surfaces(&self, reading: &str) -> Vec<&str> {
        let mut entries: Vec<_> = self
            .history
            .iter()
            .filter(|entry| {
                entry.reading == reading && is_useful_history(&entry.reading, &entry.surface)
            })
            .collect();
        sort_history(&mut entries);
        entries
            .into_iter()
            .map(|entry| entry.surface.as_str())
            .collect()
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
        if !is_useful_history(reading, surface) {
            return;
        }

        let wall_clock = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let now = next_last_used(&self.history, wall_clock);
        update_history(&mut self.history, reading, surface, now);
        trim_history(&mut self.history);

        let Some(directory) = &self.directory else {
            return;
        };
        if !self.history_is_writable {
            return;
        }

        let path = directory.join(HISTORY_FILE);
        if write_history_optimistically(&path, reading, surface, now).is_ok()
            && let Ok(Some(bytes)) = read_optional(&path)
            && let Ok(history) = parse_history(&bytes)
        {
            self.history = history;
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

fn sort_history(entries: &mut Vec<&HistoryEntry>) {
    entries.sort_unstable_by(|left, right| {
        right
            .last_used
            .cmp(&left.last_used)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.surface.cmp(&right.surface))
    });
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

fn next_last_used(history: &[HistoryEntry], wall_clock: u64) -> u64 {
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
    use super::{HISTORY_HEADER, USER_DICTIONARY_HEADER, UserData, atomic_replace_if_unchanged};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_directory(name: &str) -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "unvalley-ime-{name}-{}-{counter}",
            std::process::id()
        ));
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
    fn a_recent_selection_outranks_an_old_frequent_selection() {
        let directory = test_directory("recent-selection");
        fs::write(
            directory.join("history.tsv"),
            format!("{HISTORY_HEADER}\nかんじ\t漢字\t100\t10\nかんじ\t感じ\t1\t20\n"),
        )
        .unwrap();

        let data = UserData::load(&directory);
        assert_eq!(data.exact_history_surfaces("かんじ"), ["感じ", "漢字"]);

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
