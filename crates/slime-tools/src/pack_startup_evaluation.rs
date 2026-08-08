//! Measures dictionary-pack process startup without exposing pack vocabulary.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use slime_core::{SlimeEngine, UserData};

mod dictionary_pack_policy;
use dictionary_pack_policy::load_signed_pack_trust;

const MIN_ITERATIONS: usize = 3;
const MAX_ITERATIONS: usize = 21;

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let first = arguments.next();
    if first.as_deref() == Some("--probe") {
        return run_probe(arguments);
    }
    let options = Options::parse(first.into_iter().chain(arguments))?;
    let report = evaluate(&options)?;
    enforce_thresholds(&options, &report)?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|_| "cannot serialize startup report".to_owned())?
        );
    } else {
        println!(
            "iterations={}\tpacks={}\tentries={}\tcontext-rules={}\tbytes={}\tmedian-ms={:.3}\tp95-ms={:.3}\tmax-rss-bytes={}\tmedian-delta-ms={:.3}\tp95-delta-ms={:.3}\trss-delta-bytes={}",
            report.iterations,
            report.pack_count,
            report.entry_count,
            report.context_rule_count,
            report.pack_bytes,
            report.startup_ms.median,
            report.startup_ms.p95,
            report.max_rss_bytes,
            report.median_delta_ms.unwrap_or_default(),
            report.p95_delta_ms.unwrap_or_default(),
            report.rss_delta_bytes.unwrap_or_default()
        );
    }
    Ok(())
}

const fn usage() -> &'static str {
    "usage: slime-pack-startup-evaluate --data-dir PATH [--baseline-data-dir PATH] \
     [--iterations N] [--max-median-ms N] [--max-p95-ms N] [--max-rss-bytes N] \
     [--max-median-delta-ms N] [--max-p95-delta-ms N] [--max-rss-delta-bytes N] \
     [--verification-keys PATH --version-floors PATH --expected-packs N \
      --baseline-expected-packs N] [--json]"
}

#[derive(Debug)]
struct Options {
    data_directory: PathBuf,
    baseline_data_directory: Option<PathBuf>,
    iterations: usize,
    max_median_ms: Option<f64>,
    max_p95_ms: Option<f64>,
    max_rss_bytes: Option<u64>,
    max_median_delta_ms: Option<f64>,
    max_p95_delta_ms: Option<f64>,
    max_rss_delta_bytes: Option<u64>,
    verification_keys: Option<PathBuf>,
    version_floors: Option<PathBuf>,
    expected_packs: Option<usize>,
    baseline_expected_packs: Option<usize>,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut builder = OptionsBuilder::default();
        while let Some(argument) = arguments.next() {
            builder.parse_argument(&argument, &mut arguments)?;
        }
        builder.finish()
    }
}

#[derive(Default)]
struct OptionsBuilder {
    data_directory: Option<PathBuf>,
    baseline_data_directory: Option<PathBuf>,
    iterations: Option<usize>,
    max_median_ms: Option<f64>,
    max_p95_ms: Option<f64>,
    max_rss_bytes: Option<u64>,
    max_median_delta_ms: Option<f64>,
    max_p95_delta_ms: Option<f64>,
    max_rss_delta_bytes: Option<u64>,
    verification_keys: Option<PathBuf>,
    version_floors: Option<PathBuf>,
    expected_packs: Option<usize>,
    baseline_expected_packs: Option<usize>,
    json: bool,
}

impl OptionsBuilder {
    fn parse_argument(
        &mut self,
        argument: &str,
        arguments: &mut impl Iterator<Item = String>,
    ) -> Result<(), String> {
        match argument {
            "--data-dir" => {
                self.data_directory = Some(PathBuf::from(next_value(argument, arguments)?));
            }
            "--baseline-data-dir" => {
                self.baseline_data_directory =
                    Some(PathBuf::from(next_value(argument, arguments)?));
            }
            "--iterations" => self.iterations = Some(parse_usize(argument, arguments)?),
            "--max-median-ms" => {
                self.max_median_ms = Some(parse_non_negative(argument, arguments)?);
            }
            "--max-p95-ms" => {
                self.max_p95_ms = Some(parse_non_negative(argument, arguments)?);
            }
            "--max-rss-bytes" => self.max_rss_bytes = Some(parse_u64(argument, arguments)?),
            "--max-median-delta-ms" => {
                self.max_median_delta_ms = Some(parse_non_negative(argument, arguments)?);
            }
            "--max-p95-delta-ms" => {
                self.max_p95_delta_ms = Some(parse_non_negative(argument, arguments)?);
            }
            "--max-rss-delta-bytes" => {
                self.max_rss_delta_bytes = Some(parse_u64(argument, arguments)?);
            }
            "--verification-keys" => {
                self.verification_keys = Some(PathBuf::from(next_value(argument, arguments)?));
            }
            "--version-floors" => {
                self.version_floors = Some(PathBuf::from(next_value(argument, arguments)?));
            }
            "--expected-packs" => self.expected_packs = Some(parse_usize(argument, arguments)?),
            "--baseline-expected-packs" => {
                self.baseline_expected_packs = Some(parse_usize(argument, arguments)?);
            }
            "--json" => self.json = true,
            "--help" | "-h" => return Err(usage().to_owned()),
            _ => return Err("unknown startup evaluation option".to_owned()),
        }
        Ok(())
    }

    fn finish(self) -> Result<Options, String> {
        let iterations = self.iterations.unwrap_or(5);
        if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
            return Err(format!(
                "--iterations must be between {MIN_ITERATIONS} and {MAX_ITERATIONS}"
            ));
        }
        if self.baseline_data_directory.is_none()
            && (self.max_median_delta_ms.is_some()
                || self.max_p95_delta_ms.is_some()
                || self.max_rss_delta_bytes.is_some())
        {
            return Err("startup delta limits require --baseline-data-dir".to_owned());
        }
        let signed_option_count = usize::from(self.verification_keys.is_some())
            + usize::from(self.version_floors.is_some())
            + usize::from(self.expected_packs.is_some());
        if signed_option_count != 0 && signed_option_count != 3 {
            return Err(
                "signed startup evaluation requires keys, floors, and expected pack count"
                    .to_owned(),
            );
        }
        if self
            .expected_packs
            .is_some_and(|count| !(1..=64).contains(&count))
        {
            return Err("--expected-packs must be between 1 and 64".to_owned());
        }
        if self
            .baseline_expected_packs
            .is_some_and(|count| !(1..=64).contains(&count))
        {
            return Err("--baseline-expected-packs must be between 1 and 64".to_owned());
        }
        if self.verification_keys.is_some()
            && self.baseline_data_directory.is_some()
            && self.baseline_expected_packs.is_none()
        {
            return Err(
                "signed baseline startup evaluation requires --baseline-expected-packs".to_owned(),
            );
        }
        if self.baseline_expected_packs.is_some()
            && (self.verification_keys.is_none() || self.baseline_data_directory.is_none())
        {
            return Err("--baseline-expected-packs requires signed baseline evaluation".to_owned());
        }
        Ok(Options {
            data_directory: self.data_directory.ok_or_else(|| usage().to_owned())?,
            baseline_data_directory: self.baseline_data_directory,
            iterations,
            max_median_ms: self.max_median_ms,
            max_p95_ms: self.max_p95_ms,
            max_rss_bytes: self.max_rss_bytes,
            max_median_delta_ms: self.max_median_delta_ms,
            max_p95_delta_ms: self.max_p95_delta_ms,
            max_rss_delta_bytes: self.max_rss_delta_bytes,
            verification_keys: self.verification_keys,
            version_floors: self.version_floors,
            expected_packs: self.expected_packs,
            baseline_expected_packs: self.baseline_expected_packs,
            json: self.json,
        })
    }
}

fn parse_usize(
    option: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<usize, String> {
    next_value(option, arguments)?
        .parse()
        .map_err(|_| format!("{option} requires an integer"))
}

fn parse_u64(option: &str, arguments: &mut impl Iterator<Item = String>) -> Result<u64, String> {
    next_value(option, arguments)?
        .parse()
        .map_err(|_| format!("{option} requires an integer"))
}

fn next_value(
    option: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_non_negative(
    option: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<f64, String> {
    let value = next_value(option, arguments)?
        .parse::<f64>()
        .map_err(|_| format!("{option} requires a number"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{option} requires a finite non-negative number"));
    }
    Ok(value)
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct ProbeReport {
    signed_policy: bool,
    pack_count: usize,
    entry_count: usize,
    context_rule_count: usize,
    pack_bytes: u64,
}

#[derive(Serialize)]
struct StartupReport {
    iterations: usize,
    signed_policy: bool,
    baseline_pack_count: Option<usize>,
    baseline_entry_count: Option<usize>,
    baseline_context_rule_count: Option<usize>,
    baseline_pack_bytes: Option<u64>,
    pack_count: usize,
    entry_count: usize,
    context_rule_count: usize,
    pack_bytes: u64,
    startup_ms: LatencyReport,
    max_rss_bytes: u64,
    baseline_startup_ms: Option<LatencyReport>,
    baseline_max_rss_bytes: Option<u64>,
    median_delta_ms: Option<f64>,
    p95_delta_ms: Option<f64>,
    rss_delta_bytes: Option<i64>,
}

#[derive(Clone, Serialize)]
struct LatencyReport {
    median: f64,
    p95: f64,
    max: f64,
}

struct ProcessMeasurement {
    probe: ProbeReport,
    latency: LatencyReport,
    max_rss_bytes: u64,
}

fn run_probe(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut data_directory = None;
    let mut verification_keys = None;
    let mut version_floors = None;
    let mut expected_packs = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--data-dir" => {
                data_directory = Some(PathBuf::from(next_value(&argument, &mut arguments)?));
            }
            "--verification-keys" => {
                verification_keys = Some(PathBuf::from(next_value(&argument, &mut arguments)?));
            }
            "--version-floors" => {
                version_floors = Some(PathBuf::from(next_value(&argument, &mut arguments)?));
            }
            "--expected-packs" => {
                expected_packs = Some(
                    next_value(&argument, &mut arguments)?
                        .parse::<usize>()
                        .map_err(|_| "startup probe expected pack count is invalid".to_owned())?,
                );
            }
            _ => return Err("startup probe received an unknown option".to_owned()),
        }
    }
    let data_directory =
        data_directory.ok_or_else(|| "startup probe requires a data directory".to_owned())?;
    let signed_policy = verification_keys.is_some();
    let engine = match (verification_keys, version_floors, expected_packs) {
        (Some(keys), Some(floors), Some(_)) => {
            let trust = load_signed_pack_trust(&keys, &floors)?;
            SlimeEngine::bundled_with_user_data_and_pack_trust(
                UserData::load(&data_directory),
                trust,
            )
        }
        (None, None, None) => SlimeEngine::bundled_with_user_data(UserData::load(&data_directory)),
        _ => return Err("startup probe signed policy is incomplete".to_owned()),
    };
    if !engine.dictionary_pack_load_errors().is_empty() {
        return Err(format!(
            "startup probe rejected {} dictionary pack file(s)",
            engine.dictionary_pack_load_errors().len()
        ));
    }
    let (pack_count, entry_count, context_rule_count) =
        engine
            .installed_dictionary_packs()
            .fold((0_usize, 0_usize, 0_usize), |counts, info| {
                (
                    counts.0 + 1,
                    counts.1 + info.entry_count,
                    counts.2 + info.context_rule_count,
                )
            });
    if pack_count == 0 {
        return Err("startup probe loaded no dictionary packs".to_owned());
    }
    if expected_packs.is_some_and(|expected| pack_count != expected) {
        return Err(format!(
            "startup probe expected {} pack(s), accepted {pack_count}",
            expected_packs.expect("checked above")
        ));
    }
    let report = ProbeReport {
        signed_policy,
        pack_count,
        entry_count,
        context_rule_count,
        pack_bytes: dictionary_pack_bytes(&data_directory)?,
    };
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|_| "cannot serialize probe report".to_owned())?
    );
    Ok(())
}

fn dictionary_pack_bytes(data_directory: &Path) -> Result<u64, String> {
    let entries = fs::read_dir(data_directory.join("dictionary-packs"))
        .map_err(|_| "cannot inspect startup dictionary pack directory".to_owned())?;
    let mut bytes = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|_| "cannot inspect a startup dictionary pack".to_owned())?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("slime-dict") {
            continue;
        }
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| "cannot inspect a startup dictionary pack".to_owned())?;
        if metadata.file_type().is_file() {
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or_else(|| "startup dictionary pack bytes overflowed".to_owned())?;
        }
    }
    Ok(bytes)
}

fn evaluate(options: &Options) -> Result<StartupReport, String> {
    let executable = env::current_exe()
        .map_err(|_| "cannot locate the startup evaluation executable".to_owned())?;
    let baseline = options
        .baseline_data_directory
        .as_ref()
        .map(|directory| {
            evaluate_processes(
                &executable,
                directory,
                options,
                options.baseline_expected_packs,
            )
        })
        .transpose()?;
    let candidate = evaluate_processes(
        &executable,
        &options.data_directory,
        options,
        options.expected_packs,
    )?;
    let median_delta_ms = baseline
        .as_ref()
        .map(|baseline| candidate.latency.median - baseline.latency.median);
    let p95_delta_ms = baseline
        .as_ref()
        .map(|baseline| candidate.latency.p95 - baseline.latency.p95);
    let rss_delta_bytes = baseline
        .as_ref()
        .map(|baseline| signed_difference(candidate.max_rss_bytes, baseline.max_rss_bytes))
        .transpose()?;
    Ok(StartupReport {
        iterations: options.iterations,
        signed_policy: candidate.probe.signed_policy,
        baseline_pack_count: baseline.as_ref().map(|value| value.probe.pack_count),
        baseline_entry_count: baseline.as_ref().map(|value| value.probe.entry_count),
        baseline_context_rule_count: baseline
            .as_ref()
            .map(|value| value.probe.context_rule_count),
        baseline_pack_bytes: baseline.as_ref().map(|value| value.probe.pack_bytes),
        pack_count: candidate.probe.pack_count,
        entry_count: candidate.probe.entry_count,
        context_rule_count: candidate.probe.context_rule_count,
        pack_bytes: candidate.probe.pack_bytes,
        startup_ms: candidate.latency,
        max_rss_bytes: candidate.max_rss_bytes,
        baseline_startup_ms: baseline.as_ref().map(|value| value.latency.clone()),
        baseline_max_rss_bytes: baseline.as_ref().map(|value| value.max_rss_bytes),
        median_delta_ms,
        p95_delta_ms,
        rss_delta_bytes,
    })
}

fn evaluate_processes(
    executable: &Path,
    data_directory: &Path,
    options: &Options,
    expected_packs: Option<usize>,
) -> Result<ProcessMeasurement, String> {
    let mut latencies = Vec::with_capacity(options.iterations);
    let mut maximum_rss = 0_u64;
    let mut expected_probe = None;
    for _ in 0..options.iterations {
        let start = Instant::now();
        let output = timed_probe_command(executable, data_directory, options, expected_packs)?
            .output()
            .map_err(|_| "cannot run the startup probe".to_owned())?;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        if !output.status.success() {
            return Err("startup probe failed".to_owned());
        }
        let probe: ProbeReport = serde_json::from_slice(&output.stdout)
            .map_err(|_| "startup probe returned an invalid aggregate report".to_owned())?;
        if expected_probe
            .as_ref()
            .is_some_and(|expected| expected != &probe)
        {
            return Err("startup probe aggregate changed between iterations".to_owned());
        }
        expected_probe = Some(probe);
        maximum_rss = maximum_rss.max(parse_maximum_rss(&output.stderr)?);
        latencies.push(elapsed_ms);
    }
    latencies.sort_by(f64::total_cmp);
    let probe = expected_probe.expect("iteration range is non-empty");
    Ok(ProcessMeasurement {
        probe,
        latency: LatencyReport {
            median: percentile(&latencies, 50),
            p95: percentile(&latencies, 95),
            max: latencies.last().copied().unwrap_or_default(),
        },
        max_rss_bytes: maximum_rss,
    })
}

fn signed_difference(left: u64, right: u64) -> Result<i64, String> {
    let difference = i128::from(left) - i128::from(right);
    i64::try_from(difference).map_err(|_| "startup RSS delta overflowed".to_owned())
}

fn timed_probe_command(
    executable: &Path,
    data_directory: &Path,
    options: &Options,
    expected_packs: Option<usize>,
) -> Result<Command, String> {
    let mut command = Command::new("/usr/bin/time");
    match env::consts::OS {
        "macos" => {
            command.arg("-l");
        }
        "linux" => {
            command.arg("-v");
        }
        _ => return Err("startup RSS evaluation supports macOS and Linux".to_owned()),
    }
    command
        .arg(executable)
        .arg("--probe")
        .arg("--data-dir")
        .arg(data_directory);
    if let (Some(keys), Some(floors), Some(expected)) = (
        &options.verification_keys,
        &options.version_floors,
        expected_packs,
    ) {
        command
            .arg("--verification-keys")
            .arg(keys)
            .arg("--version-floors")
            .arg(floors)
            .arg("--expected-packs")
            .arg(expected.to_string());
    }
    Ok(command)
}

fn parse_maximum_rss(output: &[u8]) -> Result<u64, String> {
    let output = std::str::from_utf8(output)
        .map_err(|_| "startup resource report is not UTF-8".to_owned())?;
    if env::consts::OS == "macos" {
        return output
            .lines()
            .find(|line| line.contains("maximum resident set size"))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| "startup resource report omitted maximum RSS".to_owned());
    }
    if env::consts::OS == "linux" {
        let kilobytes = output
            .lines()
            .find(|line| line.contains("Maximum resident set size (kbytes):"))
            .and_then(|line| line.rsplit_once(':'))
            .map(|(_, value)| value.trim())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| "startup resource report omitted maximum RSS".to_owned())?;
        return kilobytes
            .checked_mul(1_024)
            .ok_or_else(|| "startup maximum RSS overflowed".to_owned());
    }
    Err("startup RSS evaluation supports macOS and Linux".to_owned())
}

fn percentile(values: &[f64], percentile: usize) -> f64 {
    let index = (values.len() * percentile).div_ceil(100).saturating_sub(1);
    values.get(index).copied().unwrap_or_default()
}

fn enforce_thresholds(options: &Options, report: &StartupReport) -> Result<(), String> {
    if options
        .max_median_ms
        .is_some_and(|maximum| report.startup_ms.median > maximum)
    {
        return Err(format!(
            "process startup median {:.3} ms exceeds the allowed maximum",
            report.startup_ms.median
        ));
    }
    if options
        .max_median_delta_ms
        .is_some_and(|maximum| report.median_delta_ms.is_some_and(|delta| delta > maximum))
    {
        return Err(format!(
            "process startup median delta {:.3} ms exceeds the allowed maximum",
            report.median_delta_ms.expect("checked above")
        ));
    }
    if options
        .max_p95_delta_ms
        .is_some_and(|maximum| report.p95_delta_ms.is_some_and(|delta| delta > maximum))
    {
        return Err(format!(
            "process startup p95 delta {:.3} ms exceeds the allowed maximum",
            report.p95_delta_ms.expect("checked above")
        ));
    }
    if options.max_rss_delta_bytes.is_some_and(|maximum| {
        report
            .rss_delta_bytes
            .is_some_and(|delta| delta > i64::try_from(maximum).unwrap_or(i64::MAX))
    }) {
        return Err(format!(
            "process maximum RSS delta {} bytes exceeds the allowed maximum",
            report.rss_delta_bytes.expect("checked above")
        ));
    }
    if options
        .max_p95_ms
        .is_some_and(|maximum| report.startup_ms.p95 > maximum)
    {
        return Err(format!(
            "process startup p95 {:.3} ms exceeds the allowed maximum",
            report.startup_ms.p95
        ));
    }
    if options
        .max_rss_bytes
        .is_some_and(|maximum| report.max_rss_bytes > maximum)
    {
        return Err(format!(
            "process maximum RSS {} bytes exceeds the allowed maximum",
            report.max_rss_bytes
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Options, parse_maximum_rss, percentile};

    #[test]
    fn options_require_bounded_iterations() {
        let error = Options::parse(
            ["--data-dir", "private", "--iterations", "2"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap_err();
        assert!(error.contains("between 3 and 21"));
        assert!(!error.contains("private"));
    }

    #[test]
    fn signed_options_are_an_all_or_nothing_policy() {
        let private_path = "private-verification-keys.tsv";
        let error = Options::parse(
            ["--data-dir", "data", "--verification-keys", private_path]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap_err();
        assert!(error.contains("requires keys, floors, and expected pack count"));
        assert!(!error.contains(private_path));
    }

    #[test]
    fn nearest_rank_percentiles_are_deterministic() {
        assert!((percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 50) - 3.0).abs() < f64::EPSILON);
        assert!((percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 95) - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_platform_maximum_rss() {
        let fixture = if std::env::consts::OS == "macos" {
            b" 123456 maximum resident set size\n".as_slice()
        } else if std::env::consts::OS == "linux" {
            b" Maximum resident set size (kbytes): 123\n".as_slice()
        } else {
            return;
        };
        let expected = if std::env::consts::OS == "macos" {
            123_456
        } else {
            123 * 1_024
        };
        assert_eq!(parse_maximum_rss(fixture).unwrap(), expected);
    }
}
