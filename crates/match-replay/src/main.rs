//! CLI: `match-replay --input path.ndjson` — replay inputs, diff fill/depth/revoke.
//!
//! Exit 0 on match, 1 on mismatch / error.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use match_replay::{assert_golden_matches, diff_replay, replay_paths};

#[derive(Parser, Debug)]
#[command(
    name = "match-replay",
    about = "Replay GoldenTrace NDJSON against match-core"
)]
struct Args {
    /// NDJSON golden file with `input` ops (and optionally expected fill/depth/revoke).
    #[arg(long)]
    input: PathBuf,

    /// Optional separate expected NDJSON (defaults to `--input`).
    #[arg(long)]
    expected: Option<PathBuf>,

    /// Engine backend (only `rust` is implemented).
    #[arg(long, default_value = "rust")]
    engine: String,
}

fn main() -> ExitCode {
    let args = Args::parse();

    if args.engine != "rust" {
        eprintln!("unsupported --engine {} (only rust)", args.engine);
        return ExitCode::from(1);
    }

    let expected = args.expected.as_ref().unwrap_or(&args.input);

    let result = if expected == &args.input {
        assert_golden_matches(&args.input)
    } else {
        match replay_paths(&args.input, expected) {
            Ok(collected) => {
                let diffs = diff_replay(&collected);
                if diffs.is_empty() {
                    Ok(())
                } else {
                    Err(diffs)
                }
            }
            Err(e) => Err(vec![e]),
        }
    };

    match result {
        Ok(()) => {
            println!("OK {}", args.input.display());
            ExitCode::SUCCESS
        }
        Err(diffs) => {
            eprintln!("MISMATCH {}", args.input.display());
            for d in diffs {
                eprintln!("  {d}");
            }
            ExitCode::from(1)
        }
    }
}
