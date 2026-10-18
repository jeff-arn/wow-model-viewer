# Naming

Good names are the cheapest documentation. These conventions are illustrated
with names already in this codebase — imitate them.

## General rules

- **Name the domain thing, not the type.** `internal` (an MPQ-internal path
  like `Creature\Dragon\DragonOnyxia.m2`) beats `path_str` or `s`. When two
  kinds of path exist, qualify both: `data_dir` (filesystem) vs `internal`
  (inside the archives).
- **Full words over abbreviations.** `vertices`, `warnings`, `textures`.
  Accepted short forms are the format's own vocabulary: `mpq`, `m2`, `blp`,
  `uv`, `rgba`, `gpu`, plus loop-local `i`, `x`, `y`.
- **Booleans read as questions:** `double_sided`, `unlit`, `stale`,
  `contains`. Avoid negative booleans (`not_opaque`); invert the sense
  (`transparent`) or match on an enum (`Blend::Opaque`).
- **Counts and sizes carry units or kind:** `n_verts`, `padded_row`,
  `triangle_count`, `depth_center`. A bare `n` or `size` must be obvious
  from the immediately surrounding lines.
- **Offsets say what they offset into:** `v_off`, `tex_off`, `lookup_off`
  in the parser; never `o2`, `off`, `tmp_offset`.
- **`_` prefix means deliberately ignored** (`_pad` in shader uniforms),
  never "I might use this later".

## Rust-specific conventions

- Types are `UpperCamelCase` and name concepts: `RenderModel`, `DataDir`,
  `DecodedTextures`, `LoadedModel`. Type aliases must document their payload
  (see `DecodedTextures`: "RGBA8 pixels, width, height").
- Functions are verbs: `parse_m2`, `decode_blp_rgba`, `load_model`,
  `find_skin_fallback`, `ordered_archives`. Predicates start with
  `is_`/`has_`/`contains`.
- Constructors that allocate GPU/device resources say so: `upload`,
  `upload_texture` (device-side) vs `decode_*` / `parse_*` (CPU-only).
  Keeping that verb split honest prevents accidental per-frame GPU work.
- Enum variants name the case, not the value: `Blend::AlphaKey`, not
  `Blend::Mode1`. Raw format values stay at the parse boundary
  (`Blend::Other(u16)` keeps the unknown value for diagnostics).
- Modules are short lower-case domain words: `mpq`, `model`, `render`,
  `app`. One concept per module; grow a submodule, not a `utils.rs`.

## Examples: rename on sight

| Instead of | Write | Why |
|---|---|---|
| `data2`, `info`, `obj` | `decoded`, `loaded_model` | says what it holds |
| `flag` in a batch loop | `blend_raw`, `render_flags` | which flag, whose |
| `path` for an MPQ entry | `internal` | not a filesystem path |
| `tex` (a decoded image) | `decoded`, `rgba` | `tex` is the path/name |
| `handle_err` | return `Result` with context | naming can't fix control flow |

## File and test names

- Integration tests name the behavior under test:
  `patch5_hd_texture_overrides_base`, `corrupt_inputs_error_without_panic`.
  Read as a sentence: *subject, condition, expectation*.
- Screenshots and fixtures name the model and purpose
  (`onyxia-final.png`), never `test1.png` or `out.png` in committed code.
