use std::{
    collections::BTreeSet,
    env,
    io::{self, Write},
    process,
    time::{Duration, Instant},
};

use rshell_core::{
    ResolvedTerminalProfile, TerminalOverrides, TerminalSettingsV1, TerminalSize, Viewport,
};
use rshell_session::{DefaultTerminalEngine, TerminalEngine};
use sha2::{Digest, Sha256};

const BACKEND_LINE: &str = "backend=alacritty-terminal@0.26.0";
const THROUGHPUT_BYTES: usize = 104857600;
const THROUGHPUT_SAMPLES: usize = 5;
const MINIMUM_MIB_PER_SECOND: f64 = 40.0;
const FRAME_COLS: u16 = 120;
const FRAME_ROWS: u16 = 40;
const FRAME_OBSERVATIONS: usize = 1000;
const MAXIMUM_FRAME_P95_MS: f64 = 16.0;
const SCROLLBACK_ROWS: usize = 1000;
const FIXTURE: &str = include_str!("../tests/fixtures/vt/canary.json");
const THROUGHPUT_RECORD: &[u8] =
    "rsHell throughput α界e\u{301} 0123456789 \x1b[31mRED\x1b[0m \x1b[1;34mBOLD-BLUE\x1b[0m\r\n"
        .as_bytes();
const SAMPLE_FIELDS: [&str; THROUGHPUT_SAMPLES] = [
    "throughput_sample_1_mib_s",
    "throughput_sample_2_mib_s",
    "throughput_sample_3_mib_s",
    "throughput_sample_4_mib_s",
    "throughput_sample_5_mib_s",
];
const CANDIDATE_DECISION: &str = "decision=CANDIDATE";
const NO_GO_DECISION: &str = "decision=NO-GO";
const GO_DECISION: &str = "decision=GO";

type GateResult<T> = Result<T, &'static str>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    RecordCandidate,
}

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        process::exit(1);
    }
}

fn run() -> GateResult<()> {
    let mode = mode()?;
    let expected_digest = fixture_digest()?;
    if mode == Mode::Normal && expected_digest.is_none() {
        return Err("terminal-engine fixture digest is unrecorded");
    }

    let workload = throughput_workload();
    let samples = measure_throughput(&workload)?;
    let mut sorted_samples = samples;
    sorted_samples.sort_by(f64::total_cmp);
    let median = sorted_samples[2];

    let frame_p95_ms = measure_frame_p95()?;
    let frame_p95_output = format!("{frame_p95_ms:.6}");
    let emitted_frame_p95_ms = frame_p95_output
        .parse::<f64>()
        .map_err(|_| "terminal-engine frame measurement is invalid")?;
    let digest = verify_scrollback_and_hash()?;
    if expected_digest
        .as_deref()
        .is_some_and(|expected| expected != digest)
    {
        return Err("terminal-engine fixture digest does not match");
    }

    let throughput_passed = median >= MINIMUM_MIB_PER_SECOND;
    let frame_passed =
        frame_p95_ms < MAXIMUM_FRAME_P95_MS && emitted_frame_p95_ms < MAXIMUM_FRAME_P95_MS;
    if !throughput_passed || !frame_passed {
        print_measurements(&samples, median, &frame_p95_output, &digest, NO_GO_DECISION);
        io::stdout()
            .flush()
            .map_err(|_| "terminal-engine diagnostic output failed")?;
        return Err(match (throughput_passed, frame_passed) {
            (false, false) => "terminal-engine throughput and frame thresholds were not met",
            (false, true) => "terminal-engine throughput threshold was not met",
            (true, false) => "terminal-engine frame threshold was not met",
            (true, true) => unreachable!(),
        });
    }

    let decision = if mode == Mode::RecordCandidate {
        CANDIDATE_DECISION
    } else {
        GO_DECISION
    };
    print_measurements(&samples, median, &frame_p95_output, &digest, decision);
    Ok(())
}

fn print_measurements(
    samples: &[f64; THROUGHPUT_SAMPLES],
    median: f64,
    frame_p95_output: &str,
    digest: &str,
    decision: &str,
) {
    println!("RSHELL_TERMINAL_ENGINE_GATE version=1");
    println!("{BACKEND_LINE}");
    println!("throughput_bytes={THROUGHPUT_BYTES}");
    for (field, sample) in SAMPLE_FIELDS.iter().zip(samples.iter()) {
        println!("{field}={sample:.6}");
    }
    println!("throughput_median_mib_s={median:.6}");
    println!("frame_120x40_observations={FRAME_OBSERVATIONS}");
    println!("frame_120x40_p95_ms={frame_p95_output}");
    println!("scrollback_rows={SCROLLBACK_ROWS}");
    println!("scrollback_sha256={digest}");
    println!("{decision}");
}

fn mode() -> GateResult<Mode> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => Ok(Mode::Normal),
        [argument] if argument == "--bench" => Ok(Mode::Normal),
        [candidate, bench] if candidate == "--record-candidate" && bench == "--bench" => {
            Ok(Mode::RecordCandidate)
        }
        _ => Err("terminal-engine gate arguments are invalid"),
    }
}

fn fixture_digest() -> GateResult<Option<String>> {
    let entries = FIXTURE
        .lines()
        .filter_map(|line| line.trim().strip_prefix("\"sha256\":"))
        .collect::<Vec<_>>();
    if entries.len() != 1 {
        return Err("terminal-engine fixture digest field is malformed");
    }
    let value = entries[0].trim().trim_end_matches(',').trim();
    if value == "null" {
        return Ok(None);
    }
    let digest = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .filter(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or("terminal-engine fixture digest field is malformed")?;
    Ok(Some(digest.to_owned()))
}

fn profile() -> ResolvedTerminalProfile {
    TerminalSettingsV1 {
        scrollback_lines: 2_000,
        ..TerminalSettingsV1::default()
    }
    .resolve(&TerminalOverrides::default())
}

fn size() -> TerminalSize {
    TerminalSize {
        cols: FRAME_COLS,
        rows: FRAME_ROWS,
        pixel_width: u32::from(FRAME_COLS) * 8,
        pixel_height: u32::from(FRAME_ROWS) * 16,
        dpi: 96,
    }
}

fn engine() -> GateResult<DefaultTerminalEngine> {
    DefaultTerminalEngine::new(&profile(), size())
        .map_err(|_| "terminal-engine initialization failed")
}

fn throughput_workload() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(THROUGHPUT_BYTES);
    while bytes.len() < THROUGHPUT_BYTES {
        bytes.extend_from_slice(THROUGHPUT_RECORD);
    }
    bytes.truncate(THROUGHPUT_BYTES);
    bytes
}

fn measure_throughput(workload: &[u8]) -> GateResult<[f64; THROUGHPUT_SAMPLES]> {
    if workload.len() != THROUGHPUT_BYTES {
        return Err("terminal-engine throughput byte count is invalid");
    }
    let mut samples = [0.0; THROUGHPUT_SAMPLES];
    for sample in &mut samples {
        let mut terminal = engine()?;
        let started = Instant::now();
        terminal
            .input(workload)
            .map_err(|_| "terminal-engine throughput input failed")?;
        let elapsed = started.elapsed().as_secs_f64();
        *sample = THROUGHPUT_BYTES as f64 / (1024.0 * 1024.0) / elapsed;
        if !sample.is_finite() || sample.is_sign_negative() {
            return Err("terminal-engine throughput measurement is invalid");
        }
    }
    Ok(samples)
}

fn frame_workload(fill: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(usize::from(FRAME_ROWS) * 128);
    bytes.extend_from_slice(b"\x1b[H");
    for row in 0..FRAME_ROWS {
        bytes.extend_from_slice(b"\x1b[2K");
        bytes.extend(std::iter::repeat_n(fill, usize::from(FRAME_COLS)));
        if row + 1 < FRAME_ROWS {
            bytes.extend_from_slice(b"\r\n");
        }
    }
    bytes
}

fn measure_frame_p95() -> GateResult<f64> {
    let payloads = [frame_workload(b'A'), frame_workload(b'B')];
    let mut terminal = engine()?;
    terminal
        .input(&payloads[0])
        .map_err(|_| "terminal-engine frame warmup failed")?;
    let _ = terminal.snapshot(
        Viewport {
            top_stable_row: 0,
            rows: FRAME_ROWS,
        },
        None,
    );

    let mut observations = Vec::with_capacity(FRAME_OBSERVATIONS);
    for observation in 0..FRAME_OBSERVATIONS {
        let fill = if observation % 2 == 0 { b'B' } else { b'A' };
        let started = Instant::now();
        terminal
            .input(&payloads[(observation + 1) % 2])
            .map_err(|_| "terminal-engine frame input failed")?;
        let frame = terminal.snapshot(
            Viewport {
                top_stable_row: 0,
                rows: FRAME_ROWS,
            },
            None,
        );
        observations.push(started.elapsed());
        let complete = frame.rows.len() == usize::from(FRAME_ROWS)
            && frame.rows.iter().all(|row| {
                row.cells.len() == usize::from(FRAME_COLS)
                    && row
                        .cells
                        .iter()
                        .all(|cell| cell.width == 1 && cell.text.as_bytes() == [fill])
            });
        if !complete {
            return Err("terminal-engine frame was not a complete 120x40 render");
        }
    }
    observations.sort_unstable();
    let p95_index = (95 * FRAME_OBSERVATIONS).div_ceil(100) - 1;
    let p95_ms = duration_ms(observations[p95_index]);
    if !p95_ms.is_finite() || p95_ms.is_sign_negative() {
        return Err("terminal-engine frame measurement is invalid");
    }
    Ok(p95_ms)
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn verify_scrollback_and_hash() -> GateResult<String> {
    let expected = (0..SCROLLBACK_ROWS)
        .map(|index| format!("scrollback-{index:04}"))
        .collect::<Vec<_>>();
    let mut input = Vec::new();
    for label in &expected {
        input.extend_from_slice(label.as_bytes());
        input.extend_from_slice(b"\r\n");
    }

    let mut terminal = engine()?;
    terminal
        .input(&input)
        .map_err(|_| "terminal-engine scrollback input failed")?;
    let bounds = terminal.viewport_bounds();
    let mut top = bounds.first_stable_row;
    let mut seen = BTreeSet::new();
    let mut rendered = Vec::with_capacity(SCROLLBACK_ROWS + 1);
    let cursor_row = loop {
        let frame = terminal.snapshot(
            Viewport {
                top_stable_row: top,
                rows: FRAME_ROWS,
            },
            None,
        );
        if frame.rows.len() != usize::from(FRAME_ROWS) {
            return Err("terminal-engine scrollback window was incomplete");
        }
        for row in frame.rows.iter() {
            if seen.insert(row.stable_row) {
                let raw = row
                    .cells
                    .iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>();
                let text = if raw.bytes().all(|byte| byte == b' ') {
                    String::new()
                } else {
                    raw.trim_end_matches(' ').to_owned()
                };
                rendered.push((row.stable_row, text));
            }
        }
        if top == bounds.bottom_top_stable_row {
            break frame
                .cursor
                .ok_or("terminal-engine scrollback cursor was missing")?
                .position
                .stable_row;
        }
        top = top
            .saturating_add(i64::from(FRAME_ROWS))
            .min(bounds.bottom_top_stable_row);
    };

    if rendered.len() != SCROLLBACK_ROWS + 1 {
        return Err("terminal-engine scrollback row count did not match");
    }
    let trailing = rendered
        .pop()
        .ok_or("terminal-engine trailing cursor row was missing")?;
    if trailing.0 != cursor_row || !trailing.1.is_empty() {
        return Err("terminal-engine trailing cursor row was not blank");
    }
    let actual = rendered
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>();
    if actual != expected {
        return Err("terminal-engine rendered rows did not match expected labels");
    }

    let canonical = actual.join("\n");
    Ok(format!("{:x}", Sha256::digest(canonical.as_bytes())))
}
