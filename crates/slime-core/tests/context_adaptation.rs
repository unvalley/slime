use slime_core::{ALL_DATE_FORMATS, EnginePreferences, InputEvent, SlimeEngine, UserData};

const LEARNING: EnginePreferences = EnginePreferences {
    live_conversion: false,
    history_completion: true,
    history_learning: true,
    dictionary_packs: 0,
    private_mode: false,
    date_format_mask: ALL_DATE_FORMATS,
};

#[derive(Debug)]
struct Case<'a> {
    id: &'a str,
    expectation: &'a str,
    previous_reading: &'a str,
    previous_surface: &'a str,
    target_reading: &'a str,
    expected_surface: &'a str,
    competing_previous_reading: &'a str,
    competing_previous_surface: &'a str,
    competing_surface: &'a str,
}

#[test]
fn context_adaptation_improves_without_breaking_existing_first_choices() {
    let cases = cases();
    let mut improved = 0;
    let mut preserved = 0;
    let mut regressed = 0;

    for case in &cases {
        let baseline = first_candidate_after(
            &mut new_engine(),
            case.previous_reading,
            case.previous_surface,
            case.target_reading,
        );
        match case.expectation {
            "improve" => assert_ne!(
                baseline, case.expected_surface,
                "{} must begin as a measurable error",
                case.id
            ),
            "preserve" => assert_eq!(
                baseline, case.expected_surface,
                "{} must begin as an already-correct first choice",
                case.id
            ),
            expectation => panic!("unknown expectation {expectation:?} in {}", case.id),
        }

        let mut adapted = new_engine();
        for _ in 0..2 {
            commit(&mut adapted, case.previous_reading, case.previous_surface);
            commit(&mut adapted, case.target_reading, case.expected_surface);
            commit(
                &mut adapted,
                case.competing_previous_reading,
                case.competing_previous_surface,
            );
            commit(&mut adapted, case.target_reading, case.competing_surface);
        }
        let after = first_candidate_after(
            &mut adapted,
            case.previous_reading,
            case.previous_surface,
            case.target_reading,
        );

        if baseline != case.expected_surface && after == case.expected_surface {
            improved += 1;
        } else if baseline == case.expected_surface && after == case.expected_surface {
            preserved += 1;
        } else if baseline == case.expected_surface && after != case.expected_surface {
            regressed += 1;
        }
        assert_eq!(
            after, case.expected_surface,
            "{} adapted to the wrong surface; baseline={baseline:?}",
            case.id
        );
    }

    let expected_improvements = cases
        .iter()
        .filter(|case| case.expectation == "improve")
        .count();
    let expected_preservations = cases.len() - expected_improvements;
    println!(
        "context adaptation: total={} improved={improved} preserved={preserved} regressed={regressed}",
        cases.len()
    );
    assert_eq!(improved, expected_improvements);
    assert_eq!(preserved, expected_preservations);
    assert_eq!(
        regressed, 0,
        "context adaptation must not break a baseline first choice"
    );
}

fn new_engine() -> SlimeEngine {
    let mut engine = SlimeEngine::bundled_with_user_data(UserData::default());
    engine.set_preferences(LEARNING);
    engine
}

fn first_candidate_after(
    engine: &mut SlimeEngine,
    previous_reading: &str,
    previous_surface: &str,
    target_reading: &str,
) -> String {
    commit(engine, previous_reading, previous_surface);
    type_text(engine, target_reading);
    engine.handle(InputEvent::Space);
    engine.snapshot().preedit
}

fn commit(engine: &mut SlimeEngine, reading: &str, surface: &str) {
    type_text(engine, reading);
    engine.handle(InputEvent::Space);
    let snapshot = engine.snapshot();
    let index = snapshot
        .candidates
        .iter()
        .position(|candidate| candidate == surface)
        .unwrap_or_else(|| {
            panic!(
                "surface {surface:?} is unavailable for {reading:?}: {:?}",
                snapshot.candidates
            )
        });
    engine.handle(InputEvent::SelectCandidate(
        u32::try_from(index).expect("candidate index fits u32"),
    ));
    engine.handle(InputEvent::Enter);
}

fn type_text(engine: &mut SlimeEngine, input: &str) {
    for character in input.chars() {
        engine.handle(InputEvent::Character(character));
    }
}

fn cases() -> Vec<Case<'static>> {
    include_str!("../testdata/context_adaptation.tsv")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut columns = line.split('\t');
            let case = Case {
                id: columns.next().expect("case id"),
                expectation: columns.next().expect("expectation"),
                previous_reading: columns.next().expect("previous reading"),
                previous_surface: columns.next().expect("previous surface"),
                target_reading: columns.next().expect("target reading"),
                expected_surface: columns.next().expect("expected surface"),
                competing_previous_reading: columns.next().expect("competing previous reading"),
                competing_previous_surface: columns.next().expect("competing previous surface"),
                competing_surface: columns.next().expect("competing surface"),
            };
            assert!(
                columns.next().is_none(),
                "unexpected fixture column: {line}"
            );
            case
        })
        .collect()
}
