use slime_core::{InputEvent, SlimeAction, SlimeEngine};

fn convert(input: &str) -> String {
    let mut engine = SlimeEngine::bundled();
    for character in input.chars() {
        engine.handle(InputEvent::Character(character));
    }
    engine.handle(InputEvent::Space);
    engine
        .handle(InputEvent::Enter)
        .into_iter()
        .find_map(|action| match action {
            SlimeAction::Commit(text) => Some(text),
            _ => None,
        })
        .expect("conversion must commit text")
}

#[test]
fn core_conversion_golden_cases() {
    let cases = [
        ("nihon", "日本"),
        ("kyou", "今日"),
        ("watashi", "私"),
        ("watashihanihon", "私は日本"),
        ("neko", "猫"),
        ("henkan", "変換"),
        ("nyuuryoku", "入力"),
        ("dousa", "動作"),
        ("komaru", "困る"),
        ("jishowokakujuusasemashou", "辞書を拡充させましょう"),
        ("iikanji", "いい感じ"),
    ];

    for (input, expected) in cases {
        assert_eq!(convert(input), expected, "input: {input}");
    }
}

/// Conversions the current cost model cannot get right from the reading
/// alone: each needs word context (漢字/感じ, 精度/制度, 箸/橋 share a noun
/// class, so connection costs cannot separate them). These stay red until a
/// context model lands — do not make them pass with cost overrides or by
/// adding the test sentences to the dictionary.
#[test]
#[ignore = "requires a context model"]
fn context_dependent_golden_cases() {
    let cases = [
        ("kanjihenohenkan", "漢字への変換"),
        (
            "seidowotakamerukufuuwoshiteikimashou",
            "精度を高める工夫をしていきましょう",
        ),
        ("hashidetaberu", "箸で食べる"),
    ];

    for (input, expected) in cases {
        assert_eq!(convert(input), expected, "input: {input}");
    }
}
