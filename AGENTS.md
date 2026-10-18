# AGENTS.md — WoW 1.12.1 Model Viewer

Guidance for AI agents and human contributors working in this repository.
Read this file first; follow the linked standards for anything you write.

## What this project is

A native Linux (x86_64) viewer for World of Warcraft **1.12.1** `M2` models
with `BLP` textures, written in Rust. It reads a real 1.12.1 `Data/` directory
directly — including `patch-*.MPQ` overrides in numeric order — so HD texture
packs preview exactly as the client loads them. It is a texture-pack review
tool, not a game client.

- GUI: searchable model browser + orbit/zoom/pan viewport (`egui` via `eframe`).
- CLI: headless screenshot mode for automated review loops.
- Out of scope (future phases): skeletal animation, WMO/ADT rendering,
  character/gear compositing. Do not add these without an explicit request.

## Repository layout

| Path | Contents |
|---|---|
| `src/mpq.rs` | `DataDir`: MPQ patch-chain opening, file reads, model listing |
| `src/model.rs` | Bounds-checked MD20 v256 parser producing `RenderModel` |
| `src/render.rs` | `wgpu` renderer: three alpha passes, orbit `Camera` |
| `src/app.rs` | BLP decode, `load_model`, headless `Headless::screenshot` |
| `src/main.rs` | CLI parsing, `eframe` GUI (`ViewerApp`) |
| `tests/mpq_override.rs` | Durable integration tests against real game data |
| `examples/sweep.rs` | Full-install parse sweep + consecutive-load check |
| `docs/` | Coding standards, naming, and testing conventions |

## Commands

```sh
cargo build --release              # build (no sudo, no system packages)
cargo test --release               # durable tests (skip gracefully without game data)
cargo clippy --release --all-targets -- -D warnings
cargo fmt --check                  # formatting gate (run `cargo fmt` to fix)
make check                         # all gates above, in order
scripts/setup.sh                   # one-time: git hooks + tool checks
```

Run the viewer:

```sh
./target/release/modelview --data '<Data-dir>' [--model '<internal M2 path>']
./target/release/modelview --data '<Data-dir>' --model '<path>' --screenshot out.png
```

## Environment facts (verified on the dev machine)

- Game data (never commit it): `/home/jeffa/Games/wow-1.12/drive_c/Program Files (x86)/World Of Warcraft Classic/Data`
- Tests find it via `WOW_DATA_DIR`, falling back to the path above.
- Reference material: `~/mpq-tools/upscale/blp2png.py` (BLP oracle),
  `~/mpq-tools/StormLib` (MPQ reference), https://wowdev.wiki/M2 (format spec).
- Toolchain: Rust stable via rustup (`~/.cargo/bin`). `gitleaks` lives in
  `~/.local/bin`. Nothing in this project requires `sudo` or `dnf install`.

## Non-obvious decisions — do not "fix" these

- **`wow-m2` is deliberately unused.** It fails on real 1.12 files (e.g.
  Ragnaros: "failed to fill whole buffer"), so `src/model.rs` parses the
  v256 rendering subset directly. `wow-mpq` and `wow-blp` are used and trusted.
- **Patch order is numeric, later wins.** `patch.MPQ` < `patch-2.MPQ` < …;
  base archives rank below all patches. `patch-5.MPQ` holding 256×256 HD
  textures over 64×64 base icons is the canonical proof (see tests).
- **Creature skins have variant filenames** (`RagnarosSkin.blp`,
  `...LightSkin.blp`). `load_model` falls back to a same-directory skin when
  the derived `Name1.blp` is absent; keep that fallback working.
- **Non-opaque batches are never backface-culled** and the GUI render target
  is cached across frames. Both were real bugs found by GUI verification
  (missing foliage; GPU out-of-memory). See git history / README.
- Models with no geometry (camera-only `.m2`) and non-256 versions must
  return errors, never panic. Baseline: 9,674 of 10,193 local `.m2` files parse.

## Standards

- [Coding standards](docs/coding-standards.md) — errors, parsing safety, GPU code.
- [Naming](docs/naming.md) — how to name things; names from this codebase as examples.
- [Testing](docs/testing.md) — coverage conventions and what "verified" means here.

## Before you finish any change

1. `make check` passes (fmt, clippy `-D warnings`, tests).
2. Behavior changes carry a durable test per `docs/testing.md`.
3. Rendering changes are screenshot-verified per `docs/testing.md` —
   the four acceptance models must still render correctly.
4. Commits pass the pre-commit hook (never bypass with `--no-verify`
   without saying so and explaining why).
