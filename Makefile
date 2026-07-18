# match-rust — common maintainer targets (inspired by perpetual_exchange layout)

.PHONY: help test test-art clippy fmt fair bench wal-bench run-local ci clean

help:
	@echo "Targets:"
	@echo "  test        cargo test --workspace"
	@echo "  test-art    match-core-hp with --features art"
	@echo "  clippy      cargo clippy --workspace --all-targets"
	@echo "  fmt         cargo fmt --all -- --check"
	@echo "  fair        fair_compare (fill_rate > 0)"
	@echo "  bench       criterion engine_cmp (sample-size 20)"
	@echo "  wal-bench   match-wal async throughput"
	@echo "  run-local   match-contract memory transport"
	@echo "  ci          fmt + clippy + test + test-art + fair"
	@echo "  clean       cargo clean"

test:
	cargo test --workspace

test-art:
	cargo test -p match-core-hp --features art

clippy:
	cargo clippy --workspace --all-targets

fmt:
	cargo fmt --all -- --check

fair:
	cargo run -p match-bench --release --bin fair_compare -- --n 50000

bench:
	cargo bench -p match-bench --bench engine_cmp -- --sample-size 20

wal-bench:
	cargo run -p match-wal --release --bin wal_bench -- 100000

run-local:
	MATCH_CONTRACT_CONFIG=crates/match-contract/config.example.yaml \
	MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt \
	cargo run -p match-contract

ci: fmt clippy test test-art fair

clean:
	cargo clean
