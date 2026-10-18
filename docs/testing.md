# Testing and Coverage Conventions

## Philosophy

Tests here prove behavior against **real game data**, not against mocks of
it. The formats (MPQ patch order, BLP variants, MD20 v256 layout) are the
risk; only the actual 1.12.1 archives exercise them honestly. A smaller set
of data-true tests beats broad synthetic coverage.

There is no line-coverage percentage target. The coverage requirement is
*behavioral*: every behavior listed below has a named check, and a change
that alters behavior updates its check in the same commit.

## Required coverage

| Behavior | Check | Where |
|---|---|---|
| Patch override order (later archive wins) | HD icon resolves to 256×256 from `patch-5.MPQ` | `tests/mpq_override.rs` |
| Model listing | >1,000 `.m2` entries; Onyxia + Ragnaros present | `tests/mpq_override.rs` |
| Corrupt/truncated input | Errors, never panics | `tests/mpq_override.rs` |
| Full-install parse health | Parse sweep over every `.m2`; failures categorized | `examples/sweep.rs` |
| Consecutive loads | 50 loads without crash | `examples/sweep.rs <Data> 50` |
| CLI contract | Four acceptance models exit 0 with correct PNGs; bogus path exits non-zero | Manual/scripted, see below |
| GUI controls | Orbit, zoom, pan (both buttons), `R` reset, search, selection | Screenshot-verified, see below |

## Conventions

- **Durable tests live in `tests/`** and use the project's normal harness
  (`cargo test`). One-off probes go in `examples/` or `/tmp`, never in `tests/`.
- **Game-data tests skip gracefully**: resolve the data dir from
  `WOW_DATA_DIR`, else the default local install; if neither exists, print
  why and pass. A skip is not a failure, but it must be *visible* — never
  silently return without a message.
- **Assert the requirement, not the implementation.** The override test
  asserts the decoded texture is 256×256; it does not assert which archive
  index supplied it.
- **New behavior = new check in the same commit.** Bug fixes add the
  regression test that would have caught the bug (the pan-target and
  render-target-leak fixes are the model examples: their checks are the
  GUI verification steps below).
- Deterministic inputs only. Tests must not depend on network, display,
  GPU model, or wall-clock timing.

## Rendering verification (required for renderer/GUI changes)

Headless checks cannot see pixels lie, so rendering changes require
screenshot evidence:

```sh
DATA='/home/jeffa/Games/wow-1.12/drive_c/Program Files (x86)/World Of Warcraft Classic/Data'
for m in 'Creature\Dragon\DragonOnyxia.m2' 'Creature\Ragnaros\Ragnaros.m2' \
         'Creature\HumanMalePeasant\HumanMalePeasant.m2' \
         'World\Azeroth\Westfall\PassiveDoodads\Trees\WestFallTree01.m2'; do
  ./target/release/modelview --data "$DATA" --model "$m" --screenshot "/tmp/shots/$m.png"
done
```

Each PNG must be opened and inspected: correctly textured, upright, framed,
foliage cutout clean, no missing-texture magenta. For GUI changes, launch
the app (a nested `Xephyr :99` display plus XTest input injection is the
proven recipe on this machine) and screenshot after each control:
orbit drag, wheel zoom, right-drag pan, middle-drag pan, `R` reset
(must restore the baseline frame), search filter, and list selection.

## Running everything

```sh
make check   # cargo fmt --check, clippy -D warnings, cargo test
cargo run --release --example sweep -- "$DATA" 50
```

The pre-commit hook runs `make check` plus a gitleaks scan of staged
changes. If game data is absent, say so in the commit description rather
than claiming the data-backed tests passed.
