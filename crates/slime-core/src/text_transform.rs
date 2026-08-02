use slime_romaji::kana_spellings;

pub(crate) fn hiragana(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\u{30a1}'..='\u{30f6}' | '\u{30fd}'..='\u{30fe}' => {
                char::from_u32(u32::from(character) - 0x60).expect("Katakana has Hiragana pair")
            }
            _ => character,
        })
        .collect()
}

pub(crate) fn full_katakana(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\u{3041}'..='\u{3096}' | '\u{309d}'..='\u{309e}' => {
                char::from_u32(u32::from(character) + 0x60).expect("Hiragana has Katakana pair")
            }
            _ => character,
        })
        .collect()
}

pub(crate) fn half_katakana(text: &str) -> String {
    let full = full_katakana(text);
    let mut output = String::with_capacity(full.len());
    for character in full.chars() {
        if let Some(replacement) = half_katakana_character(character) {
            output.push_str(replacement);
        } else {
            output.push(character);
        }
    }
    output
}

pub(crate) fn full_alphanumeric(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            ' ' => '\u{3000}',
            '!'..='~' => char::from_u32(u32::from(character) + 0xfee0)
                .expect("ASCII graphic has full-width pair"),
            _ => character,
        })
        .collect()
}

pub(crate) fn half_alphanumeric(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\u{3000}' => ' ',
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(u32::from(character) - 0xfee0)
                .expect("full-width graphic has ASCII pair"),
            _ => character,
        })
        .collect()
}

pub(crate) fn romanize(text: &str) -> String {
    let text = hiragana(text);
    let spellings: Vec<_> = kana_spellings().collect();
    let mut output = String::with_capacity(text.len());
    let mut offset = 0;
    while offset < text.len() {
        let suffix = &text[offset..];
        if let Some(rest) = suffix.strip_prefix('っ') {
            if let Some((_, spelling)) = preferred_spelling(rest, &spellings)
                && let Some(first) = spelling.chars().next()
                && is_ascii_consonant(first)
            {
                output.push(first);
            } else {
                output.push_str("xtu");
            }
            offset += 'っ'.len_utf8();
            continue;
        }
        if let Some((kana, spelling)) = preferred_spelling(suffix, &spellings) {
            output.push_str(spelling);
            offset += kana.len();
            continue;
        }
        let character = suffix.chars().next().expect("non-empty suffix");
        output.push(character);
        offset += character.len_utf8();
    }
    output
}

fn preferred_spelling<'a>(
    suffix: &str,
    spellings: &'a [(&'static str, &'static str)],
) -> Option<(&'a str, &'a str)> {
    spellings
        .iter()
        .filter(|(kana, _)| suffix.starts_with(kana))
        .max_by(|left, right| {
            left.0
                .len()
                .cmp(&right.0.len())
                .then_with(|| right.1.len().cmp(&left.1.len()))
        })
        .copied()
}

fn is_ascii_consonant(character: char) -> bool {
    character.is_ascii_alphabetic()
        && !matches!(character.to_ascii_lowercase(), 'a' | 'i' | 'u' | 'e' | 'o')
}

fn half_katakana_character(character: char) -> Option<&'static str> {
    Some(match character {
        '。' => "｡",
        '「' => "｢",
        '」' => "｣",
        '、' => "､",
        '・' => "･",
        'ヲ' => "ｦ",
        'ァ' => "ｧ",
        'ィ' => "ｨ",
        'ゥ' => "ｩ",
        'ェ' => "ｪ",
        'ォ' => "ｫ",
        'ャ' => "ｬ",
        'ュ' => "ｭ",
        'ョ' => "ｮ",
        'ッ' => "ｯ",
        'ー' => "ｰ",
        'ア' => "ｱ",
        'イ' => "ｲ",
        'ウ' => "ｳ",
        'エ' => "ｴ",
        'オ' => "ｵ",
        'カ' => "ｶ",
        'キ' => "ｷ",
        'ク' => "ｸ",
        'ケ' => "ｹ",
        'コ' => "ｺ",
        'サ' => "ｻ",
        'シ' => "ｼ",
        'ス' => "ｽ",
        'セ' => "ｾ",
        'ソ' => "ｿ",
        'タ' => "ﾀ",
        'チ' => "ﾁ",
        'ツ' => "ﾂ",
        'テ' => "ﾃ",
        'ト' => "ﾄ",
        'ナ' => "ﾅ",
        'ニ' => "ﾆ",
        'ヌ' => "ﾇ",
        'ネ' => "ﾈ",
        'ノ' => "ﾉ",
        'ハ' => "ﾊ",
        'ヒ' => "ﾋ",
        'フ' => "ﾌ",
        'ヘ' => "ﾍ",
        'ホ' => "ﾎ",
        'マ' => "ﾏ",
        'ミ' => "ﾐ",
        'ム' => "ﾑ",
        'メ' => "ﾒ",
        'モ' => "ﾓ",
        'ヤ' => "ﾔ",
        'ユ' => "ﾕ",
        'ヨ' => "ﾖ",
        'ラ' => "ﾗ",
        'リ' => "ﾘ",
        'ル' => "ﾙ",
        'レ' => "ﾚ",
        'ロ' => "ﾛ",
        'ワ' => "ﾜ",
        'ン' => "ﾝ",
        'ガ' => "ｶﾞ",
        'ギ' => "ｷﾞ",
        'グ' => "ｸﾞ",
        'ゲ' => "ｹﾞ",
        'ゴ' => "ｺﾞ",
        'ザ' => "ｻﾞ",
        'ジ' => "ｼﾞ",
        'ズ' => "ｽﾞ",
        'ゼ' => "ｾﾞ",
        'ゾ' => "ｿﾞ",
        'ダ' => "ﾀﾞ",
        'ヂ' => "ﾁﾞ",
        'ヅ' => "ﾂﾞ",
        'デ' => "ﾃﾞ",
        'ド' => "ﾄﾞ",
        'バ' => "ﾊﾞ",
        'ビ' => "ﾋﾞ",
        'ブ' => "ﾌﾞ",
        'ベ' => "ﾍﾞ",
        'ボ' => "ﾎﾞ",
        'パ' => "ﾊﾟ",
        'ピ' => "ﾋﾟ",
        'プ' => "ﾌﾟ",
        'ペ' => "ﾍﾟ",
        'ポ' => "ﾎﾟ",
        'ヴ' => "ｳﾞ",
        'ヷ' => "ﾜﾞ",
        'ヺ' => "ｦﾞ",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        full_alphanumeric, full_katakana, half_alphanumeric, half_katakana, hiragana, romanize,
    };

    #[test]
    fn transforms_the_five_standard_input_styles() {
        assert_eq!(hiragana("ニホン"), "にほん");
        assert_eq!(full_katakana("にほん"), "ニホン");
        assert_eq!(half_katakana("がっこう。"), "ｶﾞｯｺｳ｡");
        assert_eq!(full_alphanumeric("Slime 42"), "Ｓｌｉｍｅ　４２");
        assert_eq!(half_alphanumeric("Ｓｌｉｍｅ　４２"), "Slime 42");
        assert_eq!(romanize("にほんご"), "nihongo");
        assert_eq!(romanize("きょう"), "kyou");
    }
}
