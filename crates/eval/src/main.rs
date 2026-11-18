//! `openbook-eval`: the retrieval evaluation (`docs/evaluation.md`, `evals/README.md`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use openbook_core::domain::TutoringStyle;
use openbook_core::model::BedrockChat;
use openbook_eval::answer_report::{AnswerReport, Settings};
use openbook_eval::benchmarks::{self, Benchmarks};
use openbook_eval::cache::{Cache, CacheKind};
use openbook_eval::data::Kind;
use openbook_eval::judge::RUBRIC_VERSION;
use openbook_eval::offline::{GateKind, Tuning};
use openbook_eval::report::{Baseline, check_suite, suite_improved};
use openbook_eval::{Error, answers, corpus, data, default_evals_dir, grid, offline, refresh};
use openbook_search::headers::HeaderMode;

const USAGE: &str = "\
Usage:
  openbook-eval offline [--dev-cache] [--check] [--write-baseline] [--corpus ID]
                        [--cache DIR] [--evals DIR] [--json FILE] [--markdown FILE] [TUNING]
  openbook-eval grid    [--dev-cache] [--corpus ID] [--cache DIR] [--evals DIR] [--out DIR] [TUNING]
  openbook-eval refresh [--dev-cache] [--missing-only] [--yes] [--corpus ID] [--cache DIR] [--evals DIR]
  openbook-eval readme  [--check] [--evals DIR] [--readme FILE]
  openbook-eval answers [--kind KINDS] [--sample-covered N] [--seed N] [--style ID] [--model ID]
                        [--judge-model ID] [--catalog FILE] [--corpus ID] [--evals DIR] [--yes]
                        [--write-summary] [--prune] [--no-turn-note] [--check]

The corpora are the directories of evals/corpora. Without --corpus every command works on all of
them, and the report shows each corpus and all of them pooled. --cache needs --corpus.

TUNING (changes the numbers for one run; not allowed with --check or --write-baseline):
  --rrf-k N  --lexical-weight X  --final-sources N  --max-per-document N
  --dense-candidates N  --lexical-candidates N  --chunk-tokens N  --cap-off
  --gate any|dense|combined  --gate-distance X  --gate-coverage X  --gate-matched N
  --header-variant off|keep-first|drop-all

offline   Runs the evaluation from the caches. No network, no AWS. Prints a Markdown table.
          --check            compares with evals/baseline.json (every corpus and all pooled) and
                             exits 1 on a regression
          --write-baseline   writes evals/baseline.json and evals/benchmarks.json from the live
                             caches of all corpora (--check fails when benchmarks.json is stale)
          --dev-cache        uses the cache-dev of each corpus (a development cache; never a baseline)
          --cap-off  treats the whole PDF as one document (the report always shows both)
readme    Writes the benchmark tables of evals/benchmarks.json between the markers of README.md.
          --check            writes nothing and exits 1 when the README block is stale
grid      Runs the whole tuning grid offline and writes grid.md and grid.json (default: target/eval).
          Chunk targets other than the production one need their chunk embeddings in the cache.
answers   LIVE: answers every selected golden question with the real prompt and the default chat
          model, judges the answers, and writes target/eval/answers.md and answers.json. The answer
          cache is evals/corpora/<id>/answers. It prints the calls and a cost bound first, and
          calls only with --yes. When everything is cached it calls nothing and needs no --yes.
          --kind KINDS       comma list of covered, lookup, paraphrase, notation, followup,
                             not_covered, off_topic (default: all)
          --sample-covered N a seeded sample of N covered questions (the other kinds stay whole)
          --seed N           the seed of the sample (default 1)
          --style ID         tutoring style (default: the product default)
          --model ID         catalog ID of the chat model (default: the catalog default)
          --judge-model ID   catalog ID of the judge (default: the catalog fast model)
          --catalog FILE     the cdk.json with the model catalog (default: infra/cdk.json)
          --write-summary    writes evals/answers-summary.json (all questions, all corpora only)
          --no-turn-note     builds the prompt without the server note before the student message
                             (an ablation; its answers are cached under their own style label)
          --check            offline: fails when the current prompt has no cached answers or
                             judgments for all questions, or when answers-summary.json is stale
          --prune            removes the cached answers of the same style and the same questions that
                             this run does not use (stale prompts), and their judgments
refresh   Builds the caches.
          --dev-cache        no AWS: the PDF text layer and fake embeddings, into each cache-dev
          --missing-only     LIVE: fills what the caches lack, for all corpora in one plan. A corpus
                             with a cache gets only the embeddings and rewrites that it lacks. A
                             corpus without a cache gets the full live refresh, with page
                             transcription. It prints the calls and a cost bound first, and starts
                             only with --yes.
          (no flag)          LIVE: the full refresh of every corpus, with real Bedrock calls. It
                             prints the calls and the cost, and starts only with --yes.
";

#[derive(Default)]
struct Args {
    command: String,
    dev_cache: bool,
    check: bool,
    write_baseline: bool,
    missing_only: bool,
    readme: Option<PathBuf>,
    /// `grid` only: two rows, for the tests.
    tiny: bool,
    yes: bool,
    corpus: Option<String>,
    cache: Option<PathBuf>,
    evals: Option<PathBuf>,
    json: Option<PathBuf>,
    markdown: Option<PathBuf>,
    out: Option<PathBuf>,
    tuning: Tuning,
    kinds: Vec<Kind>,
    sample_covered: Option<usize>,
    seed: Option<u64>,
    style: Option<String>,
    model: Option<String>,
    judge_model: Option<String>,
    catalog: Option<PathBuf>,
    write_summary: bool,
    prune: bool,
    no_turn_note: bool,
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
            "--corpus" => args.corpus = Some(value(&mut argv, &flag)?),
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
            "--tiny" => args.tiny = true,
            "--kind" => args.kinds.extend(
                answers::parse_kinds(&value(&mut argv, &flag)?).map_err(|e| e.to_string())?,
            ),
            "--sample-covered" => {
                args.sample_covered = Some(number(value(&mut argv, &flag)?, &flag)?);
            }
            "--seed" => args.seed = Some(number(value(&mut argv, &flag)?, &flag)?),
            "--style" => args.style = Some(value(&mut argv, &flag)?),
            "--model" => args.model = Some(value(&mut argv, &flag)?),
            "--judge-model" => args.judge_model = Some(value(&mut argv, &flag)?),
            "--catalog" => args.catalog = Some(value(&mut argv, &flag)?.into()),
            "--write-summary" => args.write_summary = true,
            "--prune" => args.prune = true,
            "--no-turn-note" => args.no_turn_note = true,
            "--readme" => args.readme = Some(value(&mut argv, &flag)?.into()),
            "--header-variant" => {
                let text = value(&mut argv, &flag)?;
                t.header_variant = Some(HeaderMode::parse(&text).ok_or_else(|| {
                    "--header-variant needs off, keep-first, or drop-all".to_owned()
                })?);
            }
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
    if args.corpus.is_some() && check_mode {
        return Err(Error::Run(
            "--check and --write-baseline cover every corpus: do not combine them with --corpus"
                .into(),
        ));
    }
    let corpora = corpus::select(evals, args.corpus.as_deref())?;
    check_cache_flag(args, &corpora)?;
    let report = offline::run_suite(
        &corpora,
        args.dev_cache,
        args.cache.as_deref(),
        &args.tuning,
    )
    .await?;
    let kind = report.combined.cache_kind;
    if args.dev_cache && kind != CacheKind::Dev {
        return Err(Error::Run(
            "--dev-cache points at a cache that is not a dev cache".into(),
        ));
    }
    if !args.dev_cache && kind == CacheKind::Dev && check_mode {
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
        write_file(&baseline_path, &Baseline::from_suite(&report)?.to_json()?)?;
        eprintln!("Wrote {}", baseline_path.display());
        let benchmarks_path = evals.join("benchmarks.json");
        write_file(
            &benchmarks_path,
            &Benchmarks::from_suite(&report, &corpora)?.to_json()?,
        )?;
        eprintln!("Wrote {}", benchmarks_path.display());
    }
    if args.check {
        let baseline = Baseline::load(&baseline_path)?;
        if baseline.cache_kind != CacheKind::Live {
            return Err(Error::Run(
                "the baseline comes from a dev cache and cannot be used".into(),
            ));
        }
        if baseline.embedding_model != report.combined.embedding_model {
            return Err(Error::Run(format!(
                "the baseline is for embedding model {:?} and the caches have {:?}",
                baseline.embedding_model, report.combined.embedding_model
            )));
        }
        let benchmarks_path = evals.join("benchmarks.json");
        let stale =
            Benchmarks::load(&benchmarks_path)? != Benchmarks::from_suite(&report, &corpora)?;
        if stale {
            eprintln!(
                "CHECK FAILED: {} is stale. Run `openbook-eval offline --write-baseline` and `openbook-eval readme`, and commit them.",
                benchmarks_path.display()
            );
            return Ok(ExitCode::FAILURE);
        }
        let regressions = check_suite(&baseline, &report)?;
        if regressions.is_empty() {
            eprintln!(
                "Check passed: no hybrid metric of any corpus, or of all pooled, is below the baseline by more than the tolerance."
            );
            if suite_improved(&baseline, &report) {
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

/// `--cache` names one cache directory, so it needs one corpus.
fn check_cache_flag(args: &Args, corpora: &[corpus::Corpus]) -> Result<(), Error> {
    if args.cache.is_some() && corpora.len() != 1 {
        return Err(Error::Run(
            "--cache names the cache of one corpus: add --corpus ID".into(),
        ));
    }
    Ok(())
}

async fn refresh_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    let corpora = corpus::select(evals, args.corpus.as_deref())?;
    check_cache_flag(args, &corpora)?;
    if args.dev_cache {
        for corpus in &corpora {
            let cache = refresh::build_dev(corpus).await?;
            let dir = args.cache.clone().unwrap_or_else(|| corpus.cache_dir(true));
            cache.save(&dir)?;
            eprintln!(
                "Wrote the DEV cache of {} to {} ({} pages, {} embeddings). It is for development only.",
                corpus.id,
                dir.display(),
                cache.pages.len(),
                cache.embeddings.len()
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    let plan = refresh::plan_refresh(&corpora, args.missing_only).await?;
    println!("{}", plan.describe());
    if args.missing_only && plan.is_empty() {
        eprintln!("The caches have everything. Nothing to call.");
        return Ok(ExitCode::SUCCESS);
    }
    if !args.yes {
        eprintln!("Nothing was called. Run again with --yes to start.");
        return Ok(ExitCode::FAILURE);
    }
    for (corpus, cache) in refresh::run_refresh(plan).await? {
        let dir = args
            .cache
            .clone()
            .unwrap_or_else(|| corpus.cache_dir(false));
        cache.save(&dir)?;
        // Read it back, so a broken write shows now and not in CI.
        let back = Cache::load(&dir)?;
        eprintln!(
            "Wrote the live cache of {} to {} ({} pages, {} transcribed, {} embeddings, {} rewrites). Review the diff and commit it.",
            corpus.id,
            dir.display(),
            back.pages.len(),
            back.meta.transcribed_pages,
            back.embeddings.len(),
            back.rewrites.len()
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// `readme`: puts the tables of `evals/benchmarks.json` between the markers of the README.
fn readme_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    let benchmarks = Benchmarks::load(&evals.join("benchmarks.json"))?;
    let path = args.readme.clone().unwrap_or_else(|| {
        evals
            .parent()
            .map_or_else(|| PathBuf::from("README.md"), |root| root.join("README.md"))
    });
    let old = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, &e))?;
    let new = benchmarks::replace_block(&old, &benchmarks.to_markdown())?;
    if new == old {
        eprintln!("The benchmark block of {} is current.", path.display());
        return Ok(ExitCode::SUCCESS);
    }
    if args.check {
        eprintln!(
            "CHECK FAILED: the benchmark block of {} is stale. Run `make benchmarks` and commit it.",
            path.display()
        );
        return Ok(ExitCode::FAILURE);
    }
    write_file(&path, &new)?;
    eprintln!("Wrote the benchmark block of {}", path.display());
    Ok(ExitCode::SUCCESS)
}

async fn grid_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    let corpora = corpus::select(evals, args.corpus.as_deref())?;
    check_cache_flag(args, &corpora)?;
    let grid = grid::run(
        &corpora,
        args.dev_cache,
        args.cache.as_deref(),
        &args.tuning,
        &if args.tiny {
            grid::Axes::tiny()
        } else {
            grid::Axes::full()
        },
    )
    .await?;
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

async fn answers_command(args: &Args, evals: &Path) -> Result<ExitCode, Error> {
    if args.dev_cache {
        return Err(Error::Run(
            "the answer evaluation needs the live cache: do not use --dev-cache".into(),
        ));
    }
    let selection = answers::Selection {
        kinds: args.kinds.clone(),
        sample_covered: args.sample_covered,
        seed: args.seed,
    };
    if args.write_summary && (!selection.is_full() || args.corpus.is_some()) {
        return Err(Error::Run(
            "--write-summary needs every question of every corpus: do not combine it with --kind, --sample-covered, or --corpus".into(),
        ));
    }
    let corpora = corpus::select(evals, args.corpus.as_deref())?;
    let catalog_path = args
        .catalog
        .clone()
        .unwrap_or_else(|| answers::default_catalog_path(evals));
    let catalog = answers::load_catalog(&catalog_path)?;
    let choice =
        answers::choose_models(&catalog, args.model.as_deref(), args.judge_model.as_deref())?;
    let style = match &args.style {
        Some(text) => answers::parse_style(text)?,
        None => TutoringStyle::default(),
    };
    let variant = answers::PromptVariant {
        style,
        turn_note: !args.no_turn_note,
    };
    if args.check
        && (!selection.is_full()
            || args.corpus.is_some()
            || args.style.is_some()
            || args.no_turn_note)
    {
        return Err(Error::Run(
            "--check covers all questions of all corpora in the default style with the server note: do not combine it with --kind, --sample-covered, --corpus, --style, or --no-turn-note".into(),
        ));
    }
    let mut pool = Vec::new();
    for corpus in &corpora {
        let sections = data::load_sections(&corpus.sections_path())?;
        let golden = data::load_golden(&corpus.golden_path(), data::page_total(&sections))?;
        pool.extend(golden.into_iter().map(|q| (corpus.id.clone(), q)));
    }
    let picked = selection.pick(pool);
    let mut runs = Vec::new();
    for corpus in &corpora {
        let questions: Vec<_> = picked
            .iter()
            .filter(|(id, _)| *id == corpus.id)
            .map(|(_, q)| q.clone())
            .collect();
        if questions.is_empty() {
            continue;
        }
        let prepared = answers::prepare_corpus(corpus, &questions, variant, &choice.chat).await?;
        let (answer_store, judgment_store) =
            answers::open_stores(corpus, &choice.chat, &choice.judge)?;
        runs.push(answers::CorpusRun {
            corpus: corpus.clone(),
            prepared,
            answers: answer_store,
            judgments: judgment_store,
        });
    }
    if runs.is_empty() {
        return Err(Error::Run("the selection holds no question".into()));
    }
    let region = std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| format!("{} (default)", refresh::DEFAULT_REGION));
    let profile = std::env::var("AWS_PROFILE")
        .unwrap_or_else(|_| "(none: the default credential chain)".into());
    let region_name = std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| refresh::DEFAULT_REGION.to_owned());
    let plan = answers::make_plan(&runs, &choice, variant, &selection, region, profile)?;
    println!("{}", plan.describe());
    if args.check {
        answers::check_complete(&plan)?;
    }
    if !plan.is_empty() {
        answers::check_region(&choice.chat, &region_name)?;
        answers::check_region(&choice.judge, &region_name)?;
        if !args.yes {
            eprintln!("Nothing was called. Run again with --yes to start.");
            return Ok(ExitCode::FAILURE);
        }
        let sdk = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(
                aws_config::meta::region::RegionProviderChain::default_provider()
                    .or_else(refresh::DEFAULT_REGION),
            )
            .load()
            .await;
        let client = aws_sdk_bedrockruntime::Client::new(&sdk);
        let models = answers::Models {
            chat: std::sync::Arc::new(BedrockChat::new(client.clone(), &choice.chat)),
            judge: std::sync::Arc::new(BedrockChat::new(client, &choice.judge)),
        };
        answers::run_calls(&mut runs, &models, &choice, variant).await?;
    }
    if args.prune {
        let removed = answers::prune(&mut runs, variant, &choice.judge.model_id)?;
        eprintln!("Removed {removed} cache entries that this run does not use.");
    }
    let scored = answers::score_all(&runs, &choice.judge.model_id)?;
    let settings = Settings {
        chat_model: choice.chat.model_id.clone(),
        judge_model: choice.judge.model_id.clone(),
        style: style.id().to_owned(),
        rubric_version: RUBRIC_VERSION,
        prompt_fingerprint: answers::prompt_fingerprint(&runs),
        selection: selection.describe(),
        turn_note: variant.turn_note,
    };
    let report = AnswerReport::build(settings, &scored);
    if args.check {
        let path = evals.join("answers-summary.json");
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, &e))?;
        let committed: AnswerReport = serde_json::from_str(&text)
            .map_err(|e| Error::Data(format!("{}: {e}", path.display())))?;
        if committed != report.summary() {
            eprintln!(
                "CHECK FAILED: {} is stale. Run `openbook-eval answers --write-summary --yes` and commit it.",
                path.display()
            );
            return Ok(ExitCode::FAILURE);
        }
        eprintln!(
            "Check passed: the cache holds every answer and judgment of the current prompt, and the summary matches."
        );
        return Ok(ExitCode::SUCCESS);
    }
    let markdown = report.to_markdown();
    println!("{markdown}");
    let out = evals.parent().map_or_else(
        || PathBuf::from("target/eval"),
        |root| root.join("target/eval"),
    );
    write_file(&out.join("answers.md"), &markdown)?;
    write_file(&out.join("answers.json"), &report.to_json()?)?;
    write_file(
        &out.join("answers-detail.jsonl"),
        &answers::details(&runs, &choice.judge.model_id)?,
    )?;
    eprintln!(
        "Wrote {} and answers.json",
        out.join("answers.md").display()
    );
    if args.write_summary {
        let path = evals.join("answers-summary.json");
        write_file(&path, &report.summary().to_json()?)?;
        eprintln!("Wrote {}", path.display());
    }
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
        "answers" => answers_command(&args, &evals).await,
        "readme" => readme_command(&args, &evals),
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
        let m = args(&["refresh", "--missing-only", "--yes", "--corpus", "x"]).unwrap();
        assert!(m.missing_only && m.yes);
        assert_eq!(m.corpus.as_deref(), Some("x"));
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
