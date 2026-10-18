use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wow_mpq::PatchChain;

/// A 1.12.1 `Data/` directory opened as a patch chain of MPQ archives.
/// Later (higher-priority) archives override earlier ones per file,
/// matching Blizzard patch semantics.
pub struct DataDir {
    pub root: PathBuf,
    patch_chain: PatchChain,
    pub archive_paths: Vec<PathBuf>,
}

/// Numeric suffix of a patch archive: `patch.MPQ` -> 0, `patch-5.MPQ` -> 5.
/// Returns `None` for base (non-patch) archives.
pub fn patch_archive_number(archive_name: &str) -> Option<i32> {
    let lowercased_name = archive_name.to_ascii_lowercase();
    let stem = lowercased_name.strip_suffix(".mpq")?;
    if stem == "patch" {
        return Some(0);
    }
    stem.strip_prefix("patch-")?.parse().ok()
}

fn is_mpq_archive_name(file_name: &str) -> bool {
    file_name.to_ascii_lowercase().ends_with(".mpq")
}

/// List a Data directory's archives in load order with chain priorities:
/// base archives first (alphabetical, low priorities), then `patch.MPQ`,
/// `patch-2.MPQ`, ... in numeric order at higher priorities so they win.
pub fn ordered_archives(data_directory: &Path) -> Result<Vec<(PathBuf, i32)>> {
    let mut base_archives: Vec<PathBuf> = Vec::new();
    let mut patch_archives: BTreeMap<i32, PathBuf> = BTreeMap::new();
    let entries = std::fs::read_dir(data_directory)
        .with_context(|| format!("reading {}", data_directory.display()))?;
    for entry in entries {
        let candidate_path = entry?.path();
        let Some(file_name) = candidate_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_mpq_archive_name(file_name) {
            continue;
        }
        match patch_archive_number(file_name) {
            Some(patch_number) => {
                patch_archives.insert(patch_number, candidate_path);
            }
            None => base_archives.push(candidate_path),
        }
    }
    base_archives.sort();
    if base_archives.is_empty() && patch_archives.is_empty() {
        bail!("no .MPQ archives found in {}", data_directory.display());
    }
    let mut ordered: Vec<(PathBuf, i32)> = base_archives
        .into_iter()
        .enumerate()
        .map(|(index, path)| (path, index as i32))
        .collect();
    for (patch_number, path) in patch_archives {
        ordered.push((path, 1000 + patch_number));
    }
    Ok(ordered)
}

/// Sort internal file names and remove case-insensitive duplicates.
fn sorted_unique_names(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

impl DataDir {
    pub fn open(data_directory: &Path) -> Result<Self> {
        let ordered = ordered_archives(data_directory)?;
        let mut patch_chain = PatchChain::new();
        let mut archive_paths = Vec::new();
        for (archive_path, priority) in &ordered {
            patch_chain
                .add_archive(archive_path, *priority)
                .with_context(|| format!("opening {}", archive_path.display()))?;
            archive_paths.push(archive_path.clone());
        }
        Ok(Self {
            root: data_directory.to_path_buf(),
            patch_chain,
            archive_paths,
        })
    }

    /// Read one file by MPQ-internal path (e.g. `Creature\Dragon\DragonOnyxia.m2`),
    /// resolved through the patch chain so overrides win.
    pub fn read_file(&mut self, internal_path: &str) -> Result<Vec<u8>> {
        self.patch_chain
            .read_file(internal_path)
            .with_context(|| format!("reading {internal_path}"))
    }

    pub fn contains_file(&self, internal_path: &str) -> bool {
        self.patch_chain.contains_file(internal_path)
    }

    /// All `.m2` model paths visible through the patch chain, sorted.
    pub fn list_models(&mut self) -> Result<Vec<String>> {
        let all_names = self.list_all_files()?;
        Ok(all_names
            .into_iter()
            .filter(|name| name.to_ascii_lowercase().ends_with(".m2"))
            .collect())
    }

    /// All file paths visible through the patch chain, sorted and deduplicated.
    pub fn list_all_files(&mut self) -> Result<Vec<String>> {
        let entries = self.patch_chain.list().context("listing archives")?;
        Ok(sorted_unique_names(
            entries.into_iter().map(|entry| entry.name).collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn patch_archive_number_parses_patch_names() {
        assert_eq!(patch_archive_number("patch.MPQ"), Some(0));
        assert_eq!(patch_archive_number("patch-2.MPQ"), Some(2));
        assert_eq!(patch_archive_number("Patch-12.mpq"), Some(12));
    }

    #[test]
    fn patch_archive_number_rejects_base_archives() {
        assert_eq!(patch_archive_number("model.MPQ"), None);
        assert_eq!(patch_archive_number("texture.mpq"), None);
        assert_eq!(patch_archive_number("patch-notes.txt"), None);
        assert_eq!(patch_archive_number("patch-x.MPQ"), None);
    }

    fn write_empty_file(directory: &Path, file_name: &str) {
        let mut file = std::fs::File::create(directory.join(file_name)).unwrap();
        file.write_all(b"not a real mpq").unwrap();
    }

    #[test]
    fn ordered_archives_puts_patches_after_base_in_numeric_order() {
        let temp_directory = std::env::temp_dir().join("modelview-mpq-test-ordered");
        let _ = std::fs::remove_dir_all(&temp_directory);
        std::fs::create_dir_all(&temp_directory).unwrap();
        for file_name in [
            "texture.MPQ",
            "model.MPQ",
            "patch-10.MPQ",
            "patch-2.MPQ",
            "patch.MPQ",
            "readme.txt",
        ] {
            write_empty_file(&temp_directory, file_name);
        }
        let ordered = ordered_archives(&temp_directory).expect("archives ordered");
        let names: Vec<String> = ordered
            .iter()
            .map(|(path, _)| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "model.MPQ",
                "texture.MPQ",
                "patch.MPQ",
                "patch-2.MPQ",
                "patch-10.MPQ"
            ]
        );
        let priorities: Vec<i32> = ordered.iter().map(|(_, priority)| *priority).collect();
        let mut sorted_priorities = priorities.clone();
        sorted_priorities.sort();
        assert_eq!(priorities, sorted_priorities, "priorities must ascend");
        let _ = std::fs::remove_dir_all(&temp_directory);
    }

    #[test]
    fn ordered_archives_errors_on_directory_without_mpqs() {
        let temp_directory = std::env::temp_dir().join("modelview-mpq-test-empty");
        let _ = std::fs::remove_dir_all(&temp_directory);
        std::fs::create_dir_all(&temp_directory).unwrap();
        assert!(ordered_archives(&temp_directory).is_err());
        let _ = std::fs::remove_dir_all(&temp_directory);
    }

    #[test]
    fn ordered_archives_errors_on_missing_directory() {
        let missing = Path::new("/definitely/not/a/real/data/dir");
        assert!(ordered_archives(missing).is_err());
    }

    #[test]
    fn sorted_unique_names_dedupes_case_insensitively() {
        // After sorting, case variants of the same name sit adjacent and collapse.
        let names = sorted_unique_names(vec![
            "Creature\\B.m2".to_string(),
            "creature\\b.M2".to_string(),
            "Creature\\A.m2".to_string(),
        ]);
        assert_eq!(
            names,
            vec!["Creature\\A.m2".to_string(), "Creature\\B.m2".to_string()]
        );
    }
}
