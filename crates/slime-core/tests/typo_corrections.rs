use slime_core::{InputEvent, SlimeAction, SlimeEngine};
use std::collections::BTreeMap;

#[test]
fn conservative_typo_fixture_keeps_input_and_labels_every_correction() {
    let mut total = 0;
    let mut recalled = 0;
    let mut by_edit = BTreeMap::new();
    for line in include_str!("../testdata/typo_corrections.tsv").lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let raw = columns.next().expect("raw input");
        let corrected_reading = columns.next().expect("corrected reading");
        let expected_surface = columns.next().expect("expected surface");
        let edit = columns.next().expect("edit kind");
        assert!(
            columns.next().is_none(),
            "unexpected fixture column: {line}"
        );

        let mut engine = SlimeEngine::bundled();
        engine.set_typo_correction_enabled(true);
        for character in raw.chars() {
            engine.handle(InputEvent::Character(character));
        }
        let actions = engine.handle(InputEvent::Space);
        let snapshot = engine.snapshot();

        assert_ne!(
            snapshot.preedit, expected_surface,
            "correction must not be selected automatically: {line}"
        );
        assert_eq!(
            snapshot.candidates.first(),
            Some(&snapshot.preedit),
            "original input must remain first: {line}"
        );
        total += 1;
        *by_edit.entry(edit).or_insert(0usize) += 1;
        let found = snapshot
            .candidates
            .iter()
            .any(|candidate| candidate == expected_surface);
        recalled += usize::from(found);
        assert!(
            found,
            "missing corrected surface: {line}; candidates={:?}",
            snapshot.candidates
        );
        let expected_label = format!("{expected_surface}　（{corrected_reading}に訂正）");
        assert!(
            actions.iter().any(|action| {
                matches!(
                    action,
                    SlimeAction::ShowCandidates { candidates, .. }
                        if candidates.contains(&expected_label)
                )
            }),
            "missing correction label: {line}; actions={actions:?}"
        );
    }
    println!("typo correction positives: total={total} recalled={recalled} by_edit={by_edit:?}");
    assert_eq!(recalled, total, "every fixed positive must be recalled");
    for required_edit in [
        "deletion",
        "duplicate",
        "missing_consonant",
        "missing_geminate",
        "missing_syllabic_n",
        "missing_vowel",
        "neighbor",
        "transposition",
    ] {
        assert!(
            by_edit.get(required_edit).is_some_and(|count| *count >= 2),
            "edit kind needs at least two independent fixtures: {required_edit}"
        );
    }
}

#[test]
fn conservative_typo_fixture_does_not_annotate_known_or_unsupported_input() {
    let mut total = 0;
    let mut unnecessary = 0;
    for line in include_str!("../testdata/typo_non_corrections.tsv").lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let raw = columns.next().expect("raw input");
        let _reason = columns.next().expect("reason");
        assert!(
            columns.next().is_none(),
            "unexpected fixture column: {line}"
        );

        let mut engine = SlimeEngine::bundled();
        engine.set_typo_correction_enabled(true);
        for character in raw.chars() {
            engine.handle(InputEvent::Character(character));
        }
        let actions = engine.handle(InputEvent::Space);

        total += 1;
        let annotated = actions.iter().any(|action| {
            matches!(
                action,
                SlimeAction::ShowCandidates { candidates, .. }
                    if candidates.iter().any(|candidate| candidate.contains("に訂正）"))
            )
        });
        unnecessary += usize::from(annotated);
        assert!(!annotated, "unexpected correction for {line}: {actions:?}");
    }
    println!("typo correction negatives: total={total} unnecessary={unnecessary}");
    assert_eq!(unnecessary, 0, "fixed negatives must not show corrections");
}
