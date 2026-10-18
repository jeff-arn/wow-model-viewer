# WoW 1.12.1 Model Viewer (Linux, Rust)

Native Linux viewer for World of Warcraft 1.12.1 `M2` models with `BLP` textures.
Reads a real 1.12.1 `Data/` directory directly, including `patch-*.MPQ` overrides
in numeric order, so HD texture packs (e.g. 4x upscales in `patch-5.MPQ`) preview
exactly as the client would load them.

## Build

```sh
cargo build --release
```

No `sudo`, no system dev packages: all dependencies come from crates.io
(`eframe`/`egui` + `wgpu` for UI/rendering, `wow-mpq`/`wow-blp` for formats,
plus a small built-in MD20 v256 parser in `src/model.rs` — the `wow-m2` crate
fails on some 1.12 files such as Ragnaros, so the rendering subset is parsed
locally and bounds-checked).

## Usage

GUI browser:

```sh
./target/release/modelview --data '/path/to/World Of Warcraft Classic/Data'
```

Left-drag orbits, mouse wheel zooms, right/middle-drag pans, `R` resets the view.
The left panel lists every `.m2` in the patch chain with live search.

Headless screenshot (for texture-pack review loops):

```sh
./target/release/modelview --data <Data-dir> \
  --model 'Creature\Dragon\DragonOnyxia.m2' \
  --screenshot out.png [--width 1024 --height 768 --angle 0.6,0.35]
```

Exits 0 on success (PNG written), non-zero with a message on failure.

## Design choices (v1)

- Skin profile 0 only; bone/animation data is not used (Phase 2).
- BLP base mip level only; no GPU mipmap generation.
- Render flags map to opaque / alpha-test (discard) / transparent (blended,
  back-to-front) passes; non-opaque batches are never backface-culled.
- Creature skins with variant filenames (`RagnarosSkin.blp`, `...LightSkin.blp`)
  are resolved by a same-directory fallback when the derived `Name1.blp` is absent.
- Models with no geometry (camera-only `.m2` files) and non-256 versions fail
  with an error message, never a crash. Parse sweep over the full local install:
  9,674 of 10,193 `.m2` files render; the rest are camera/empty or later-version files.

## Tests

```sh
cargo test --release   # MPQ patch-override (256x256 HD icon), browser listing, corrupt-input handling
cargo run --release --example sweep -- <Data-dir> 50   # full parse sweep + consecutive loads
```

Tests use `WOW_DATA_DIR` if set, otherwise the default local install path, and
skip gracefully when no game data is present.
