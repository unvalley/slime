//! Debug helper: dump candidates and n-best paths for a reading.
//! Usage: `cargo run -p slime-converter --example debug_reading -- いいかんじ`

fn main() {
    let reading = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "いいかんじ".to_owned());
    let limit = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);
    let dictionary = slime_converter::Dictionary::bundled();

    println!("== candidates ==");
    for candidate in dictionary.candidates(&reading) {
        println!("{:>8}  {}", candidate.cost, candidate.surface);
    }

    println!("== n-best paths ==");
    for conversion in dictionary.convert_n_best(&reading, limit) {
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

    println!("== recombined top-10 segments ==");
    for candidate in dictionary.recombined_n_best_variants(&reading, 10, 32) {
        println!("{:>8}  {}", candidate.cost, candidate.surface);
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
