# Gate targets. `make check` mirrors the pre-commit hook (minus gitleaks,
# which needs staged changes; the hook runs it as `gitleaks protect --staged`).
.PHONY: check fmt clippy test build

build:
	cargo build --release

fmt:
	cargo fmt --check

clippy:
	cargo clippy --release --all-targets -- -D warnings

test:
	cargo test --release

check: fmt clippy test
	@echo "check: all gates passed"
