//! `validate-replay`: assert properties of a replay JSONL log.
//!
//! Reads the JSONL file produced by a match (live or from `--replay`), parses it via
//! the existing `replay::read_records` so the on-disk format is the single source of
//! truth, and applies a set of assertions defined in a TOML file. Used by the
//! integration test harness: each scenario ships a `docker-compose.yml` + `expect.toml`,
//! the harness runs the compose stack, then invokes this binary on the resulting log.
//!
//! Exit codes: 0 on success, 1 on a failed assertion, 2 on an IO or parse error. Any
//! failure prints a one-line diagnostic to stderr suitable for surfacing in CI.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use naval_server::replay::{self, ReplayRecord};
use serde::Deserialize;

#[derive(Parser, Debug)]
#[command(
    name = "validate-replay",
    about = "Assert properties of a naval-server replay JSONL log"
)]
struct Args {
    /// Path to the replay JSONL file to validate.
    replay: PathBuf,

    /// Path to a TOML file describing expected outcomes. See `Expect` for the
    /// supported fields; every field is optional.
    #[arg(long)]
    expect: PathBuf,
}

/// Assertions consumed from `expect.toml`. Each field is optional — only the assertions
/// the scenario cares about need to be present.
#[derive(Deserialize, Debug, Default)]
struct Expect {
    /// Expected seed in the header.
    seed: Option<u64>,
    /// Expected tick rate in the header.
    tick_hz: Option<u32>,
    /// Bot names (in `bot_id` order) the header must contain.
    bots: Option<Vec<String>>,

    /// Required: the log must contain an `end` record (i.e. the match concluded
    /// cleanly). Defaults to true.
    require_end_record: Option<bool>,
    /// Expected winning `bot_id`. Use the string "draw" to assert no winner.
    /// Omit to skip the assertion.
    winner: Option<String>,
    /// Inclusive lower bound on the final tick number.
    min_final_tick: Option<u64>,
    /// Inclusive upper bound on the final tick number.
    max_final_tick: Option<u64>,

    /// Minimum number of `tick` records (i.e. ticks where at least one bot issued a
    /// command). A bot that never moved would produce zero — this catches "bot
    /// silently crashed at startup" regressions.
    min_tick_records: Option<u64>,
    /// Minimum number of `fire` commands across all bots and ticks. Useful for
    /// scenarios where combat is expected.
    min_fire_commands: Option<u64>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => {
            println!("validate-replay: OK ({})", args.replay.display());
            ExitCode::SUCCESS
        }
        Err(ValidateError::Assertion(msg)) => {
            eprintln!("validate-replay: FAIL ({}): {msg}", args.replay.display());
            ExitCode::from(1)
        }
        Err(ValidateError::Other(msg)) => {
            eprintln!("validate-replay: ERROR ({}): {msg}", args.replay.display());
            ExitCode::from(2)
        }
    }
}

enum ValidateError {
    /// An assertion was evaluated and did not hold. Distinguished from `Other` so the
    /// harness can tell a real test failure apart from a setup error.
    Assertion(String),
    /// Anything else — file not found, malformed JSONL, malformed expect.toml.
    Other(String),
}

fn run(args: &Args) -> Result<(), ValidateError> {
    let expect_text = std::fs::read_to_string(&args.expect)
        .map_err(|e| ValidateError::Other(format!("read {}: {e}", args.expect.display())))?;
    let expect: Expect = toml::from_str(&expect_text)
        .map_err(|e| ValidateError::Other(format!("parse {}: {e}", args.expect.display())))?;

    let records = replay::read_records(&args.replay)
        .map_err(|e| ValidateError::Other(format!("read records: {e}")))?;

    if records.is_empty() {
        return Err(ValidateError::Assertion("replay is empty".into()));
    }

    let header = match &records[0] {
        ReplayRecord::Header(h) => h,
        _ => {
            return Err(ValidateError::Assertion(
                "first record is not a header".into(),
            ))
        }
    };

    if let Some(expected) = expect.seed {
        if header.seed != expected {
            return Err(ValidateError::Assertion(format!(
                "seed mismatch: expected {expected}, got {}",
                header.seed
            )));
        }
    }
    if let Some(expected) = expect.tick_hz {
        if header.tick_hz != expected {
            return Err(ValidateError::Assertion(format!(
                "tick_hz mismatch: expected {expected}, got {}",
                header.tick_hz
            )));
        }
    }
    if let Some(expected) = &expect.bots {
        let actual: Vec<&str> = header.bots.iter().map(|b| b.name.as_str()).collect();
        if actual != expected.iter().map(String::as_str).collect::<Vec<_>>() {
            return Err(ValidateError::Assertion(format!(
                "bot roster mismatch: expected {expected:?}, got {actual:?}"
            )));
        }
    }

    let mut tick_records: u64 = 0;
    let mut fire_commands: u64 = 0;
    let mut end_record: Option<&replay::ReplayEnd> = None;
    for rec in &records[1..] {
        match rec {
            ReplayRecord::Tick(t) => {
                tick_records += 1;
                for c in &t.commands {
                    if c.fire.is_some() {
                        fire_commands += 1;
                    }
                }
            }
            ReplayRecord::End(e) => {
                end_record = Some(e);
            }
            ReplayRecord::Header(_) => {
                return Err(ValidateError::Assertion(
                    "stray header record mid-stream".into(),
                ))
            }
        }
    }

    if expect.require_end_record.unwrap_or(true) && end_record.is_none() {
        return Err(ValidateError::Assertion(
            "missing terminal `end` record".into(),
        ));
    }

    if let Some(end) = end_record {
        if let Some(expected) = &expect.winner {
            let actual = end.winner.as_deref().unwrap_or("draw");
            let normalized_expected = if expected.is_empty() {
                "draw"
            } else {
                expected
            };
            if actual != normalized_expected {
                return Err(ValidateError::Assertion(format!(
                    "winner mismatch: expected `{normalized_expected}`, got `{actual}`"
                )));
            }
        }
        if let Some(min) = expect.min_final_tick {
            if end.tick < min {
                return Err(ValidateError::Assertion(format!(
                    "final tick {} below min {min}",
                    end.tick
                )));
            }
        }
        if let Some(max) = expect.max_final_tick {
            if end.tick > max {
                return Err(ValidateError::Assertion(format!(
                    "final tick {} above max {max}",
                    end.tick
                )));
            }
        }
    }

    if let Some(min) = expect.min_tick_records {
        if tick_records < min {
            return Err(ValidateError::Assertion(format!(
                "tick records {tick_records} below min {min}"
            )));
        }
    }
    if let Some(min) = expect.min_fire_commands {
        if fire_commands < min {
            return Err(ValidateError::Assertion(format!(
                "fire commands {fire_commands} below min {min}"
            )));
        }
    }

    Ok(())
}
