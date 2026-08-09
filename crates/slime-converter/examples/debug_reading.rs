//! Debug helper: dump candidates and n-best paths for a reading.
//! Usage: `cargo run -p slime-converter --example debug_reading -- いいかんじ [left_context] [right_context]`

fn main() {
    let reading = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "いいかんじ".to_owned());
    let left_context = std::env::args().nth(2).unwrap_or_default();
    let right_context = std::env::args().nth(3).unwrap_or_default();
    let dictionary = slime_converter::Dictionary::bundled();

    println!("== candidates with context ==");
    for candidate in
        dictionary.candidates_with_surrounding_context(&reading, &left_context, &right_context)
    {
        println!("{:>8}  {}", candidate.cost, candidate.surface);
    }

    println!("== n-best paths ==");
    for conversion in dictionary.convert_n_best(&reading, 20) {
        let segments: Vec<String> = conversion
            .segments
            .iter()
            .map(|s| format!("{}/{}({})", s.reading, s.surface, s.cost))
            .collect();
        println!(
            "{:>8}  {}  [{}]",
            conversion.cost,
            conversion.surface,
            segments.join(" + ")
        );
    }

    println!("== convert_best ==");
    if let Some(best) = dictionary.convert_best(&reading) {
        let segments: Vec<String> = best
            .segments
            .iter()
            .map(|s| format!("{}/{}({})", s.reading, s.surface, s.cost))
            .collect();
        println!(
            "{:>8}  {}  [{}]",
            best.cost,
            best.surface,
            segments.join(" + ")
        );
    }
}
