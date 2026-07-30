use slime_core::{InputEvent, SlimeEngine};

#[test]
fn period_key_exposes_period_and_leader_alternatives() {
    let mut engine = SlimeEngine::bundled();
    engine.handle(InputEvent::Character('.'));
    engine.handle(InputEvent::Space);

    let candidates = engine.snapshot().candidates;
    assert_eq!(candidates[0], "。");
    for expected in ["．", ".", "｡", "…", "‥", "⋯"] {
        assert!(
            candidates.iter().any(|candidate| candidate == expected),
            "missing {expected:?}: {candidates:?}"
        );
    }
}

#[test]
fn slash_key_exposes_middle_dot_and_slash_alternatives() {
    let mut engine = SlimeEngine::bundled();
    engine.handle(InputEvent::Character('/'));
    engine.handle(InputEvent::Space);

    let candidates = engine.snapshot().candidates;
    assert_eq!(candidates[0], "・");
    for expected in ["／", "/", "･", "＼", "\\", "÷", "…", "‥"] {
        assert!(
            candidates.iter().any(|candidate| candidate == expected),
            "missing {expected:?}: {candidates:?}"
        );
    }
}
