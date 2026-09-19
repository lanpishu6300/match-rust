#!/usr/bin/env bash
# Fail unless llvm-cov TOTAL branch cover is 100.00% for gated crates.
set -euo pipefail
export PATH="${HOME}/.cargo/bin:${PATH}"

run_and_check() {
  local label="$1"
  local ignore_regex="$2"
  shift 2
  local out
  out="$(cargo +nightly llvm-cov "$@" --branch --ignore-filename-regex "$ignore_regex" --summary-only 2>&1)" || {
    echo "$out"
    echo "llvm-cov failed: $label" >&2
    exit 1
  }
  echo "$out" | tail -20
  local total
  total="$(echo "$out" | grep '^TOTAL' | tail -1 || true)"
  # TOTAL line ends with branch cover percentage as last field
  if ! echo "$total" | grep -Eq '100\.00%[[:space:]]*$'; then
    echo "ERROR: $label branch cover is not 100%: $total" >&2
    exit 1
  fi
  echo "OK: $label → 100% branches"
}

run_and_check "protocol+core+hp" '(tests/|/bin/)' -p match-protocol -p match-core -p match-core-hp
run_and_check "match-spot" '(tests/|main\.rs|/bin/)' -p match-spot
run_and_check "core-hp+art" '(tests/|/bin/)' -p match-core-hp --features art
