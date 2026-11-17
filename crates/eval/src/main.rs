//! `openbook-eval`: the retrieval evaluation (`docs/evaluation.md`, `evals/README.md`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use openbook_eval::cache::{Cache, CacheKind};
use openbook_eval::offline::{GateKind, Tuning};
use openbook_eval::report::{Baseline, check_report, improved};
use openbook_eval::{Error, default_evals_dir, grid, offline, refresh};

const USAGE: &str = "\
Usage:
  openbook-eval offline [--dev-cache] [--check] [--write-baseline]
                        [--cache DIR] [--evals DIR] [--json FILE] [--markdown FILE] [TUNING]
  openbook-eval grid    [--dev-cache] [--cache DIR] [--evals DIR] [--out DIR] [TUNING]
  openbook-eval refresh [--dev-cache] [--missing-only] [--yes] [--evals DIR]

TUNING (changes the numbers for one run; not allowed with --check or --write-baseline):
  --rrf-k N  --lexical-weight X  --final-sources N  --max-per-document N
  --dense-candidates N  --lexical-candidates N  --chunk-tokens N  --cap-off
  --gate any|dense|combined  --gate-distance X  --gate-coverage X  --gate-matched N

offline   Runs the evaluation from the cache. No network, no AWS. Prints a Markdown table.
          --check            compares with evals/baseline.json and exits 1 on a regression
          --write-baseline   writes evals/baseline.json from a live cache
          --dev-cache        uses evals/cache-dev (a development cache; never a baseline)
          --cap-off  treats the whole PDF as one document (the report always shows both)
grid      Runs the whole tuning grid offline and writes grid.md and grid.json (default: target/eval).
          Chunk targets other than the production one need their chunk embeddings in the cache.
refresh   Builds the cache.
          --dev-cache        no AWS: the PDF text layer and fake embeddings, into evals/cache-dev
          --missing-only     LIVE: reuses evals/cache/pages, does no transcription, and calls Bedrock
                             only for the embeddings and rewrites that the cache lacks
          (no flag)          LIVE: real Bedrock calls. It prints the calls and the cost, and starts
                             only with --yes.
";

#[derive(Default)]
struct Args {
    command: String,
    dev_cache: bool,
    check: bool,
    write_baseline: bool,
    missing_only: bool,
    yes: bool,
    cache: Option<PathBuf>,
    evals: Option<PathBuf>,
    json: Option<PathBuf>,
    markdown: Option<PathBuf>,
    out: Option<PathBuf>,
    tuning: Tuning,
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
        let t = &mut args.tuning;
        match flag.as_str() {
            "--dev-cache" => args.dev_cache = true,
            "--check" => args.check = true,
            "--write-baseline" => args.write_baseline = true,
            "--missing-only" => args.missing_only = true,
            "--yes" => args.yes = true,
            "--cache" => args.cache = Some(value(&mut argv, &flag)?.into()),
            "--evals" => args.evals = Some(value(&mut argv, &flag)?.into()),
            "--json" => args.json = Some(value(&mut argv, &flag)?.into()),
            "--markdown" => args.markdown = Some(value(&mut argv, &flag)?.into()),
            "--out" => args.out = Some(value(&mut argv, &flag)?.into()),
            "--rrf-k" => t.rrf_k = Some(number(value(&mut argv, &flag)?, &flag)?),
            "--lexical-weight" => {
                t.lexical_weight = Some(number(value(&mut argv, &flag)?, &flag)?);
            }
            "--final-sources" => t.final_sources = Some(number(value(&mut argv, &flag)?, &flag)?),
            "--max-per-document" => {
                t.max_per_document = Some(number(value(&mut argv, &flag)?, &flag)?);
            }
            "--dense-candidates" => t.dense_top_k = Some(number(value(&mut argv, &flag)?, &flag)?),
            "--lexical-candidates" => {
                t.lexical_top_k = Some(number(value(&mut argv, &flag)?, &flag)?);
            }
            "--chunk-tokens" => t.chunk_tokens = Some(number(value(&mut argv, &flag)?, &flag)?),
            "--cap-off" => t.cap_off = true,
            "--gate" => {
                let text = value(&mut argv, &flag)?;
                t.gate_kind = Some(
                    GateKind::parse(&text)
                        .ok_or_else(|| "--gate needs any, dense, or combined".to_owned())?,
                );
            }
            "--gate-distance" => t.gate_distance = Some(number(value(&mut argv, &flag)?, &flag)?),
            "--gate-coverage" => t.gate_coverage = Some(number(value(&mut argv, &flag)?, &flag)?),
            "--gate-matched" => t.gate_matched = Some(number(value(&mut argv, &flag)?, &flag)?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(args)
}

fn write_file(path: &Path, text: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, &e))?;
    }
    std::fs::write(path, text).map_err(|e| Error::io(path, &e))
}

async fn offline_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    if args.check && args.write_baseline {
        return Err(Error::Run(
            "--check and --write-baseline cannot be used together: check first, then write the baseline in the same commit as the change".into(),
        ));
    }
    let check_mode = args.check || args.write_baseline;
    if args.dev_cache && check_mode {
        return Err(Error::Run(
            "a dev cache never gives a baseline: do not use --check or --write-baseline with --dev-cache".into(),
        ));
    }
    if args.tuning.is_tuned() && check_mode {
        return Err(Error::Run(
            "--check and --write-baseline use the production numbers: do not combine them with tuning flags".into(),
        ));
    }
    let cache_dir = args
        .cache
        .clone()
        .unwrap_or_else(|| evals.join(if args.dev_cache { "cache-dev" } else { "cache" }));
    let report = offline::run_dir(evals, &cache_dir, &args.tuning).await?;
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
        write_file(&baseline_path, &Baseline::from_report(&report)?.to_json()?)?;
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
        let regressions = check_report(&baseline, &report);
        if regressions.is_empty() {
            eprintln!(
                "Check passed: no hybrid metric is below the baseline by more than the tolerance."
            );
            if improved(&baseline.hybrid, &report.hybrid)
                || report
                    .cap_off
                    .as_ref()
                    .is_some_and(|c| improved(&baseline.cap_off, &c.hybrid))
            {
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
    if args.missing_only {
        return refresh_missing_command(args, evals).await;
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

/// `refresh --missing-only`: reuses the cached pages and calls Bedrock only for what the cache lacks.
async fn refresh_missing_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    let dir = args.cache.clone().unwrap_or_else(|| evals.join("cache"));
    let (plan, cache) = refresh::plan_missing(evals, &dir)?;
    println!("{}", plan.describe());
    if plan.is_empty() {
        eprintln!("The cache has everything. Nothing to call.");
        return Ok(ExitCode::SUCCESS);
    }
    if !args.yes {
        eprintln!("Nothing was called. Run again with --yes to start.");
        return Ok(ExitCode::FAILURE);
    }
    let cache = refresh::run_missing(evals, &plan, cache).await?;
    cache.save(&dir)?;
    // Read it back, so a broken write shows now and not in CI.
    let back = Cache::load(&dir)?;
    eprintln!(
        "Updated the live cache in {} ({} embeddings, {} rewrites). Review the diff and commit it.",
        dir.display(),
        back.embeddings.len(),
        back.rewrites.len()
    );
    Ok(ExitCode::SUCCESS)
}

async fn grid_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    let cache_dir = args
        .cache
        .clone()
        .unwrap_or_else(|| evals.join(if args.dev_cache { "cache-dev" } else { "cache" }));
    let grid = grid::run(evals, &cache_dir, &args.tuning).await?;
    let production = grid::Knobs::production(&args.tuning.options(&grid.embedding_model));
    let markdown = grid.to_markdown(&production, 20);
    let out = args.out.clone().unwrap_or_else(|| {
        evals.parent().map_or_else(
            || PathBuf::from("target/eval"),
            |root| root.join("target/eval"),
        )
    });
    write_file(&out.join("grid.md"), &markdown)?;
    write_file(&out.join("grid.json"), &grid.to_json()?)?;
    println!("{markdown}");
    eprintln!("Wrote {} and grid.json", out.join("grid.md").display());
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
        "grid" => grid_command(&args, &evals).await,
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
    use openbook_search::retrieval::{GateRule, Params};

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
            "--gate",
            "combined",
            "--gate-distance",
            "0.8",
            "--gate-matched",
            "4",
            "--chunk-tokens",
            "600",
            "--cap-off",
        ])
        .unwrap();
        assert!(a.dev_cache && a.tuning.is_tuned());
        let o = a.tuning.options("amazon.titan-embed-text-v2:0");
        assert_eq!(o.params.rrf_k, 30.0);
        assert_eq!(o.chunk_tokens(), 600);
        assert!(o.cap_off);
        assert_eq!(
            o.params.gate,
            GateRule::DenseOrLexical {
                max_distance: 0.8,
                min_coverage: 0.75,
                min_matched: 4
            }
        );
        assert_eq!(o.params.final_sources, Params::default().final_sources);
        assert!(!args(&["offline", "--check"]).unwrap().tuning.is_tuned());
        let m = args(&["refresh", "--missing-only", "--yes"]).unwrap();
        assert!(m.missing_only && m.yes);
    }

    #[test]
    fn a_distance_alone_keeps_the_rule_of_the_model() {
        let a = args(&["offline", "--gate-distance", "0.7"]).unwrap();
        assert_eq!(
            a.tuning.options("fake").params.gate,
            GateRule::AnyLexicalHit { max_distance: 0.7 }
        );
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(args(&[]).is_err());
        assert!(args(&["offline", "--nope"]).is_err());
        assert!(args(&["offline", "--rrf-k"]).is_err());
        assert!(args(&["offline", "--rrf-k", "x"]).is_err());
        assert!(args(&["offline", "--gate", "maybe"]).is_err());
    }
}
