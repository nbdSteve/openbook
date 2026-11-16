//! `openbook-eval`: the retrieval evaluation (`docs/evaluation.md`, `evals/README.md`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use openbook_eval::cache::{Cache, CacheKind};
use openbook_eval::report::{Baseline, check, improved};
use openbook_eval::{Error, default_evals_dir, offline, refresh};
use openbook_search::retrieval::Params;

const USAGE: &str = "\
Usage:
  openbook-eval offline [--dev-cache] [--check] [--write-baseline]
                        [--cache DIR] [--evals DIR] [--json FILE] [--markdown FILE]
                        [--rrf-k N] [--final-sources N] [--max-per-document N] [--gate-distance X]
  openbook-eval refresh [--dev-cache] [--yes] [--evals DIR]

offline   Runs the evaluation from the cache. No network, no AWS. Prints a Markdown table.
          --check            compares with evals/baseline.json and exits 1 on a regression
          --write-baseline   writes evals/baseline.json from a live cache
          --dev-cache        uses evals/cache-dev (a development cache; never a baseline)
          The four tuning flags change the numbers of fusion and the gate. They cannot be used
          with --check or --write-baseline.
refresh   Builds the cache.
          --dev-cache        no AWS: the PDF text layer and fake embeddings, into evals/cache-dev
          (no flag)          LIVE: real Bedrock calls. It prints the calls and the cost, and starts
                             only with --yes.
";

#[derive(Default)]
struct Args {
    command: String,
    dev_cache: bool,
    check: bool,
    write_baseline: bool,
    yes: bool,
    cache: Option<PathBuf>,
    evals: Option<PathBuf>,
    json: Option<PathBuf>,
    markdown: Option<PathBuf>,
    rrf_k: Option<f64>,
    final_sources: Option<usize>,
    max_per_document: Option<usize>,
    gate_distance: Option<f32>,
}

fn parse(argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args::default();
    let mut argv = argv.peekable();
    args.command = argv.next().ok_or("a command is needed")?;
    fn value(argv: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
        argv.next().ok_or_else(|| format!("{flag} needs a value"))
    }
    fn number<T: std::str::FromStr>(text: String, flag: &str) -> Result<T, String> {
        text.parse().map_err(|_| format!("{flag} needs a number"))
    }
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--dev-cache" => args.dev_cache = true,
            "--check" => args.check = true,
            "--write-baseline" => args.write_baseline = true,
            "--yes" => args.yes = true,
            "--cache" => args.cache = Some(value(&mut argv, &flag)?.into()),
            "--evals" => args.evals = Some(value(&mut argv, &flag)?.into()),
            "--json" => args.json = Some(value(&mut argv, &flag)?.into()),
            "--markdown" => args.markdown = Some(value(&mut argv, &flag)?.into()),
            "--rrf-k" => args.rrf_k = Some(number(value(&mut argv, &flag)?, &flag)?),
            "--final-sources" => {
                args.final_sources = Some(number(value(&mut argv, &flag)?, &flag)?)
            }
            "--max-per-document" => {
                args.max_per_document = Some(number(value(&mut argv, &flag)?, &flag)?);
            }
            "--gate-distance" => {
                args.gate_distance = Some(number(value(&mut argv, &flag)?, &flag)?)
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(args)
}

impl Args {
    fn tuned(&self) -> bool {
        self.rrf_k.is_some()
            || self.final_sources.is_some()
            || self.max_per_document.is_some()
            || self.gate_distance.is_some()
    }

    fn params(&self) -> Params {
        let default = Params::default();
        Params {
            rrf_k: self.rrf_k.unwrap_or(default.rrf_k),
            final_sources: self.final_sources.unwrap_or(default.final_sources),
            max_per_document: self.max_per_document.unwrap_or(default.max_per_document),
            gate_max_distance: self.gate_distance.unwrap_or(default.gate_max_distance),
        }
    }
}

fn write_file(path: &Path, text: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, &e))?;
    }
    std::fs::write(path, text).map_err(|e| Error::io(path, &e))
}

async fn offline_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    let check_mode = args.check || args.write_baseline;
    if args.dev_cache && check_mode {
        return Err(Error::Run(
            "a dev cache never gives a baseline: do not use --check or --write-baseline with --dev-cache".into(),
        ));
    }
    if args.tuned() && check_mode {
        return Err(Error::Run(
            "--check and --write-baseline use the production numbers: do not combine them with tuning flags".into(),
        ));
    }
    let cache_dir = args
        .cache
        .clone()
        .unwrap_or_else(|| evals.join(if args.dev_cache { "cache-dev" } else { "cache" }));
    let params = args.params();
    let report = offline::run_dir(evals, &cache_dir, &params).await?;
    if args.dev_cache && report.cache_kind != CacheKind::Dev {
        return Err(Error::Run(
            "--dev-cache points at a cache that is not a dev cache".into(),
        ));
    }
    if !args.dev_cache && report.cache_kind == CacheKind::Dev && check_mode {
        return Err(Error::Run(
            "this cache is a dev cache: it cannot give or check a baseline".into(),
        ));
    }

    let markdown = report.to_markdown();
    println!("{markdown}");
    let out = evals.parent().map_or_else(
        || PathBuf::from("target/eval"),
        |root| root.join("target/eval"),
    );
    let json_path = args.json.clone().unwrap_or_else(|| out.join("report.json"));
    let md_path = args
        .markdown
        .clone()
        .unwrap_or_else(|| out.join("report.md"));
    write_file(&json_path, &report.to_json()?)?;
    write_file(&md_path, &markdown)?;
    eprintln!("Wrote {} and {}", json_path.display(), md_path.display());

    let baseline_path = evals.join("baseline.json");
    if args.write_baseline {
        write_file(&baseline_path, &Baseline::from_report(&report).to_json()?)?;
        eprintln!("Wrote {}", baseline_path.display());
    }
    if args.check {
        let baseline = Baseline::load(&baseline_path)?;
        if baseline.cache_kind != CacheKind::Live {
            return Err(Error::Run(
                "the baseline comes from a dev cache and cannot be used".into(),
            ));
        }
        if baseline.embedding_model != report.embedding_model {
            return Err(Error::Run(format!(
                "the baseline is for embedding model {:?} and the cache has {:?}",
                baseline.embedding_model, report.embedding_model
            )));
        }
        let regressions = check(&baseline.hybrid, &report.hybrid);
        if regressions.is_empty() {
            eprintln!(
                "Check passed: no hybrid metric is below the baseline by more than the tolerance."
            );
            if improved(&baseline.hybrid, &report.hybrid) {
                eprintln!(
                    "Some metrics improved. Update the baseline in the same commit: openbook-eval offline --write-baseline"
                );
            }
        } else {
            eprintln!("CHECK FAILED: retrieval got worse than the baseline.");
            for regression in &regressions {
                eprintln!("  {regression}");
            }
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn refresh_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    if args.dev_cache {
        let cache = refresh::build_dev(evals).await?;
        let dir = args
            .cache
            .clone()
            .unwrap_or_else(|| evals.join("cache-dev"));
        cache.save(&dir)?;
        eprintln!(
            "Wrote the DEV cache to {} ({} pages, {} embeddings). It is for development only.",
            dir.display(),
            cache.pages.len(),
            cache.embeddings.len()
        );
        return Ok(ExitCode::SUCCESS);
    }
    let (plan, pages) = refresh::plan_live(evals).await?;
    println!("{}", plan.describe());
    if !args.yes {
        eprintln!("Nothing was called. Run again with --yes to start.");
        return Ok(ExitCode::FAILURE);
    }
    let cache = refresh::build_live(evals, &plan, pages).await?;
    let dir = args.cache.clone().unwrap_or_else(|| evals.join("cache"));
    cache.save(&dir)?;
    // Read it back, so a broken write shows now and not in CI.
    let back = Cache::load(&dir)?;
    eprintln!(
        "Wrote the live cache to {} ({} pages, {} transcribed, {} embeddings). Review the diff and commit it.",
        dir.display(),
        back.pages.len(),
        back.meta.transcribed_pages,
        back.embeddings.len()
    );
    Ok(ExitCode::SUCCESS)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(problem) => {
            eprintln!("{problem}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let evals = args.evals.clone().unwrap_or_else(default_evals_dir);
    let result = match args.command.as_str() {
        "offline" => offline_command(&args, &evals).await,
        "refresh" => refresh_command(&args, &evals).await,
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("unknown command {other}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, String> {
        parse(list.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn the_flags_are_read() {
        let a = args(&[
            "offline",
            "--dev-cache",
            "--rrf-k",
            "30",
            "--gate-distance",
            "0.8",
        ])
        .unwrap();
        assert!(a.dev_cache && a.tuned());
        let p = a.params();
        assert_eq!(p.rrf_k, 30.0);
        assert_eq!(p.gate_max_distance, 0.8);
        assert_eq!(p.final_sources, Params::default().final_sources);
        assert!(!args(&["offline", "--check"]).unwrap().tuned());
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(args(&[]).is_err());
        assert!(args(&["offline", "--nope"]).is_err());
        assert!(args(&["offline", "--rrf-k"]).is_err());
        assert!(args(&["offline", "--rrf-k", "x"]).is_err());
    }
}
