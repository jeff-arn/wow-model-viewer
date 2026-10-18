use modelview::mpq::DataDir;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: sweep <Data-dir> [loads]"),
    );
    let loads: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut data = DataDir::open(&dir)?;
    let models = data.list_models()?;
    println!("models: {}", models.len());
    let (mut ok, mut failed) = (0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();
    let mut reasons: std::collections::HashMap<String, usize> = Default::default();
    for m in &models {
        match data
            .read_file(m)
            .and_then(|b| modelview::model::parse_m2(&b, m))
        {
            Ok(_) => ok += 1,
            Err(e) => {
                failed += 1;
                *reasons.entry(format!("{e}")).or_default() += 1;
                if failures.len() < 10 {
                    failures.push(format!("{m}: {e:#}"));
                }
            }
        }
    }
    println!("parse ok={ok} failed={failed}");
    for f in &failures {
        println!("  FAIL {f}");
    }
    let mut rs: Vec<_> = reasons.into_iter().collect();
    rs.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (r, c) in &rs {
        println!("  reason x{c}: {r}");
    }
    if loads > 0 {
        let picks: Vec<&String> = models
            .iter()
            .step_by((models.len() / loads.max(1)).max(1))
            .take(loads)
            .collect();
        let mut loaded = 0usize;
        for m in &picks {
            match data
                .read_file(m)
                .and_then(|b| modelview::model::parse_m2(&b, m))
            {
                Ok(_) => loaded += 1,
                Err(e) => eprintln!("  load skipped {m}: {e:#}"),
            }
        }
        println!("consecutive loads ok: {loaded}");
    }
    Ok(())
}
