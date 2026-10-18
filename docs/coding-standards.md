# Coding Standards

Rust standards for this repository. The pre-commit hook enforces the
mechanical parts (`rustfmt`, Clippy with `-D warnings`, tests, gitleaks);
this document covers the judgment parts.

## Formatting and lints

- `cargo fmt` is the only formatting authority. Do not hand-align code,
  reorder imports manually, or fight the formatter.
- Clippy runs as `cargo clippy --release --all-targets -- -D warnings`.
  A warning is a failed check, not a suggestion.
- Suppress a lint only at the smallest scope that needs it, with a comment
  stating why, e.g.:
  `#[allow(clippy::too_many_arguments)] // GPU handles are passed explicitly`.
  Blanket `#![allow(...)]` at crate level needs a stated reason in review.

## Errors

- Library code (`src/mpq.rs`, `src/model.rs`, `src/app.rs`, `src/render.rs`)
  returns `anyhow::Result` with context: `with_context(|| format!("reading {internal}"))`.
- No `unwrap()` / `expect()` / `panic!` on data-derived values (file bytes,
  archive contents, GPU readbacks that can fail). `expect` is acceptable only
  for programmer invariants with a message explaining why it cannot fail.
- The binary (`src/main.rs`) converts errors to a message on stderr and a
  non-zero exit code: `2` for usage errors, `1` for runtime failures.

## Parsing untrusted data (MPQ / M2 / BLP bytes)

Game files are untrusted input. This is the highest-risk code in the project.

- Every offset/count read goes through a bounds check and returns an error
  on overflow — follow the `Rd` helper pattern in `src/model.rs`. Never index
  a byte slice with an unchecked file-derived offset.
- Reject unsupported versions explicitly (`version != 256` errors) instead of
  parsing speculatively. Unknown flags/chunks: warn and continue, never crash.
- Counts from files are not allocation sizes until validated against the
  remaining input length. Do not `Vec::with_capacity(file_controlled_count)`
  without that check.
- Fuzz-adjacent inputs (truncated, zeroed, wrong magic) belong in
  `tests/mpq_override.rs::corrupt_inputs_error_without_panic` style tests.

## GPU / rendering code

- One `wgpu` device path serves both the GUI and headless screenshots; do
  not fork a second renderer for a new feature.
- Cache GPU resources across frames (buffers, textures, bind groups,
  registered `egui` textures). Per-frame allocation of render targets caused
  a real out-of-memory crash; treat new per-frame `create_texture` /
  `register_native_texture` calls as review red flags.
- Coordinate convention: WoW is Z-up; the renderer converts to Y-up in
  `Renderer::upload` (`[x, z, -y]`). Keep conversions in exactly one place.

## Dependencies

- Prefer the standard library and existing dependencies. A new crate needs a
  clear payoff and must build offline from `Cargo.lock` afterwards.
- Pin nothing by hand beyond what `cargo add` writes; commit `Cargo.lock`
  (this is a binary project).
- No dependency may require `sudo`, system packages, or network at runtime.

## Comments

- Comment *why*, not *what*: format quirks, deliberate deviations, and
  invariants (see the patch-order and skin-fallback comments in the code).
- No commented-out code, no TODO without an owner or issue reference.
