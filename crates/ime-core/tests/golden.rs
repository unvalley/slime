use ime_core::{ImeAction, ImeEngine, InputEvent};

fn convert(input: &str) -> String {
    let mut engine = ImeEngine::bundled();
    for character in input.chars() {
        engine.handle(InputEvent::Character(character));
    }
    engine.handle(InputEvent::Space);
    engine
        .handle(InputEvent::Enter)
        .into_iter()
        .find_map(|action| match action {
            ImeAction::Commit(text) => Some(text),
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
        ("kanjihenohenkan", "漢字への変換"),
        (
            "seidowotakamerukufuuwoshiteikimashou",
            "精度を高める工夫をしていきましょう",
        ),
        ("jishowokakujuusasemashou", "辞書を拡充させましょう"),
    ];

    for (input, expected) in cases {
        assert_eq!(convert(input), expected, "input: {input}");
    }
}
