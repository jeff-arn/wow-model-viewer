#!/bin/sh
# One-time developer setup: git hooks + required tools. No sudo needed.
set -eu

cd "$(dirname "$0")/.."

# Versioned hooks in .githooks/ (shared with every clone).
git config core.hooksPath .githooks
echo "setup: core.hooksPath -> .githooks"

# Rust components the gates require.
if command -v rustup >/dev/null 2>&1; then
    rustup component add clippy rustfmt >/dev/null 2>&1 || true
fi
command -v cargo >/dev/null 2>&1 || { echo "setup: cargo not found; install Rust via rustup first" >&2; exit 1; }
cargo clippy --version >/dev/null 2>&1 || { echo "setup: clippy unavailable; run: rustup component add clippy" >&2; exit 1; }
cargo fmt --version >/dev/null 2>&1 || { echo "setup: rustfmt unavailable; run: rustup component add rustfmt" >&2; exit 1; }
echo "setup: clippy + rustfmt present"

# gitleaks (secret scanning) into ~/.local/bin if missing.
if ! command -v gitleaks >/dev/null 2>&1; then
    version=8.30.1
    tmp=$(mktemp -d)
    echo "setup: installing gitleaks $version to ~/.local/bin"
    curl -sL "https://github.com/gitleaks/gitleaks/releases/download/v${version}/gitleaks_${version}_linux_x64.tar.gz" -o "$tmp/gl.tar.gz"
    tar -xzf "$tmp/gl.tar.gz" -C "$tmp" gitleaks
    mkdir -p "$HOME/.local/bin"
    install -m 755 "$tmp/gitleaks" "$HOME/.local/bin/gitleaks"
    rm -rf "$tmp"
fi
gitleaks version >/dev/null 2>&1 || { echo "setup: gitleaks still unavailable; add ~/.local/bin to PATH" >&2; exit 1; }
echo "setup: gitleaks $(gitleaks version)"
echo "setup: done. 'make check' runs the same gates as the pre-commit hook (minus gitleaks)."
