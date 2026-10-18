use modelview::mpq::DataDir;
use std::path::PathBuf;

fn data_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("WOW_DATA_DIR") {
        return Some(PathBuf::from(d));
    }
    let default = PathBuf::from(
        "/home/jeffa/Games/wow-1.12/drive_c/Program Files (x86)/World Of Warcraft Classic/Data",
    );
    if default.is_dir() {
        Some(default)
    } else {
        None
    }
}

#[test]
fn patch5_hd_texture_overrides_base() {
    let Some(dir) = data_dir() else {
        eprintln!("skipping: no WoW Data dir (set WOW_DATA_DIR)");
        return;
    };
    let mut data = DataDir::open(&dir).expect("open Data dir");
    // This icon exists in base archives at 64x64 and in patch-5.MPQ upscaled to 256x256.
    let raw = data
        .read_file("Interface\\Icons\\INV_Misc_Gear_08.blp")
        .expect("read icon through patch chain");
    assert_eq!(&raw[0..4], b"BLP2");
    let w = u32::from_le_bytes(raw[12..16].try_into().unwrap());
    let h = u32::from_le_bytes(raw[16..20].try_into().unwrap());
    assert_eq!(
        (w, h),
        (256, 256),
        "expected HD override from patch-5.MPQ, got {w}x{h}"
    );
}

#[test]
fn model_browser_lists_m2_files() {
    let Some(dir) = data_dir() else {
        return;
    };
    let mut data = DataDir::open(&dir).expect("open Data dir");
    let models = data.list_models().expect("list models");
    assert!(
        models.len() > 1000,
        "expected thousands of models, got {}",
        models.len()
    );
    assert!(
        models
            .iter()
            .any(|m| m.eq_ignore_ascii_case("Creature\\Dragon\\DragonOnyxia.m2"))
    );
    assert!(
        models
            .iter()
            .any(|m| m.eq_ignore_ascii_case("Creature\\Ragnaros\\Ragnaros.m2"))
    );
}

#[test]
fn corrupt_inputs_error_without_panic() {
    assert!(modelview::model::parse_m2(b"NOPE", "x.m2").is_err());
    assert!(modelview::model::parse_m2(&[0u8; 400], "x.m2").is_err());
    let mut bad = vec![0u8; 400];
    bad[0..4].copy_from_slice(b"MD20");
    bad[4..8].copy_from_slice(&256u32.to_le_bytes());
    assert!(modelview::model::parse_m2(&bad, "x.m2").is_err());
    assert!(modelview::app::decode_blp_rgba(b"not a blp").is_err());
}
