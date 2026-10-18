use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wow_mpq::PatchChain;

pub struct DataDir {
    pub root: PathBuf,
    chain: PatchChain,
    pub archives: Vec<PathBuf>,
}

fn patch_number(name: &str) -> Option<i32> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".mpq")?;
    if stem == "patch" {
        return Some(0);
    }
    stem.strip_prefix("patch-")?.parse().ok()
}

pub fn ordered_archives(dir: &Path) -> Result<Vec<(PathBuf, i32)>> {
    let mut base: Vec<PathBuf> = Vec::new();
    let mut patches: BTreeMap<i32, PathBuf> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.to_ascii_lowercase().ends_with(".mpq") {
            continue;
        }
        match patch_number(name) {
            Some(n) => {
                patches.insert(n, path);
            }
            None => base.push(path),
        }
    }
    base.sort();
    if base.is_empty() && patches.is_empty() {
        bail!("no .MPQ archives found in {}", dir.display());
    }
    let mut out: Vec<(PathBuf, i32)> = base
        .into_iter()
        .enumerate()
        .map(|(i, p)| (p, i as i32))
        .collect();
    for (n, p) in patches {
        out.push((p, 1000 + n));
    }
    Ok(out)
}

impl DataDir {
    pub fn open(dir: &Path) -> Result<Self> {
        let ordered = ordered_archives(dir)?;
        let mut chain = PatchChain::new();
        let mut archives = Vec::new();
        for (path, priority) in &ordered {
            chain
                .add_archive(path, *priority)
                .with_context(|| format!("opening {}", path.display()))?;
            archives.push(path.clone());
        }
        Ok(Self {
            root: dir.to_path_buf(),
            chain,
            archives,
        })
    }

    pub fn read_file(&mut self, internal: &str) -> Result<Vec<u8>> {
        self.chain
            .read_file(internal)
            .with_context(|| format!("reading {internal}"))
    }

    pub fn contains(&self, internal: &str) -> bool {
        self.chain.contains_file(internal)
    }

    pub fn list_models(&mut self) -> Result<Vec<String>> {
        let entries = self.chain.list().context("listing archives")?;
        let mut models: Vec<String> = entries
            .into_iter()
            .map(|e| e.name)
            .filter(|n| n.to_ascii_lowercase().ends_with(".m2"))
            .collect();
        models.sort();
        models.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Ok(models)
    }

    pub fn list_all(&mut self) -> Result<Vec<String>> {
        let entries = self.chain.list().context("listing archives")?;
        let mut names: Vec<String> = entries.into_iter().map(|e| e.name).collect();
        names.sort();
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Ok(names)
    }
}
