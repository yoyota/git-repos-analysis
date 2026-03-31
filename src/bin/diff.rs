use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use clap::Parser;
use git2::{Commit, DiffLine, DiffOptions, DiffStats, DiffStatsFormat, Repository};
use regex::Regex;
use std::fs::{self, metadata, remove_file, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::from_utf8;
use std::sync::LazyLock;
use tracing::{info, warn};

static FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([^\|]+?)\s*\|\s*(\S+)").unwrap());

static LOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|.*/)(yarn\.lock|poetry\.lock|package-lock\.json|\.terraform\.lock\.hcl)$")
        .unwrap()
});

#[derive(Parser)]
#[command(name = "diff", about = "Extract filtered git diffs for resume generation")]
struct Args {
    /// Path to the git repository to analyze
    #[arg(short, long, default_value = ".")]
    repo: PathBuf,

    /// Output directory (default: ~/Downloads/{repo-name})
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Comma-separated author names to filter commits (default: git config user.name)
    #[arg(long)]
    authors: Option<String>,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    if let Err(e) = run() {
        tracing::error!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();

    let name = repo_name(&args.repo)?;
    let output_dir = match args.output {
        Some(p) => p,
        None => default_output_dir(&name)?,
    };
    fs::create_dir_all(&output_dir)?;

    let repo = Repository::open(&args.repo)?;
    let authors = detect_authors(&repo, args.authors.as_deref());

    if authors.is_empty() {
        warn!("no author detected. Use --authors to specify.");
    } else {
        info!("filtering commits by: {}", authors.join(", "));
    }

    process_repo(&repo, &name, &output_dir, &authors)
}

fn repo_name(repo_path: &Path) -> Result<String> {
    fs::canonicalize(repo_path)?
        .file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .context("Cannot determine repo name from path")
}

fn default_output_dir(name: &str) -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME env var not set")?;
    Ok(PathBuf::from(home).join("Downloads").join(name))
}

fn detect_authors(repo: &Repository, override_authors: Option<&str>) -> Vec<String> {
    if let Some(s) = override_authors {
        return s.split(',').map(|a| a.trim().to_string()).collect();
    }
    let mut authors = Vec::new();
    if let Ok(config) = repo.config() {
        if let Ok(name) = config.get_string("user.name") {
            authors.push(name);
        }
    }
    authors
}

fn process_repo(repo: &Repository, repo_name: &str, output_dir: &Path, authors: &[String]) -> Result<()> {
    let out_path = output_dir.join("diff.txt");
    let file = OpenOptions::new().create(true).write(true).open(&out_path)?;
    let mut writer = BufWriter::new(file);

    let header_len = write_header(&mut writer, repo_name)?;
    process_commits(&mut writer, repo, authors)?;
    writer.flush()?;

    let file_size = metadata(&out_path)?.len();
    if file_size == header_len {
        remove_file(&out_path)?;
        info!("no matching commits found — no output written.");
    } else {
        info!("output: {}", out_path.display());
    }
    Ok(())
}

fn write_header(writer: &mut BufWriter<File>, repo_name: &str) -> Result<u64> {
    let header = format!("project name: {repo_name}\n\n");
    write!(writer, "{header}")?;
    Ok(header.len() as u64)
}

fn process_commits(writer: &mut BufWriter<File>, repo: &Repository, authors: &[String]) -> Result<()> {
    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)?;
    revwalk.push_glob("refs/*")?;

    for oid in revwalk.filter_map(Result::ok) {
        if let Ok(commit) = repo.find_commit(oid) {
            process_commit(repo, &commit, writer, authors)?;
        }
    }
    Ok(())
}

fn process_commit(
    repo: &Repository,
    commit: &Commit,
    writer: &mut BufWriter<File>,
    authors: &[String],
) -> Result<()> {
    if !is_commit_valid(commit, authors) {
        return Ok(());
    }
    for line in get_diff_lines(repo, commit)? {
        write!(writer, "{}", line)?;
    }
    Ok(())
}

fn is_commit_valid(commit: &Commit, authors: &[String]) -> bool {
    commit.parent_count() <= 1
        && commit
            .author()
            .name()
            .is_some_and(|name| authors.iter().any(|a| a == name))
}

fn get_diff_lines(repo: &Repository, commit: &Commit) -> Result<Vec<String>> {
    let old_tree = commit
        .parents()
        .next()
        .map(|parent| parent.tree())
        .transpose()?;

    let tree = commit.tree()?;
    let diff = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&tree), None)?;

    let stats = diff.stats()?;
    let mut opts = prepare_diff_options(&stats)?;

    let diff_out = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&tree), Some(&mut opts))?;

    let commit_time = Utc
        .timestamp_opt(commit.time().seconds(), 0)
        .single()
        .context("Invalid timestamp")?;

    let mut lines = vec![
        format!(
            "commit date: {}\n",
            commit_time.format("%Y-%m-%d %H:%M:%S UTC")
        ),
        format!("commit message: {}\n", commit.message().unwrap_or("")),
    ];

    let mut processor = DiffLineProcessor::default();
    let mut current_file = String::new();

    diff_out.print(git2::DiffFormat::Patch, |delta, _, line| {
        if let Some(path) = delta.new_file().path().or(delta.old_file().path()) {
            current_file = path.to_string_lossy().to_string();
        }
        if let Some(result) = processor.process_diff_line(line, &current_file) {
            lines.push(result);
        }
        true
    })?;

    Ok(lines)
}

fn prepare_diff_options(stats: &DiffStats) -> Result<DiffOptions> {
    let mut opts = DiffOptions::new();
    opts.context_lines(2)
        .pathspec(" ")
        .ignore_blank_lines(true)
        .ignore_whitespace(true)
        .ignore_whitespace_change(true)
        .ignore_whitespace_eol(true);

    for file in filter_out_large_change_files(stats)? {
        opts.pathspec(&file);
    }
    Ok(opts)
}

fn filter_out_large_change_files(stats: &DiffStats) -> Result<Vec<String>> {
    if stats.insertions() + stats.deletions() > 2000 {
        return Ok(vec![]);
    }

    let buf = stats.to_buf(DiffStatsFormat::FULL, 100)?;
    let stats_str = buf.as_str().context("Invalid stats buffer")?;

    Ok(stats_str.lines().filter_map(parse_stats_line).collect())
}

fn parse_stats_line(line: &str) -> Option<String> {
    let caps = FILE_RE.captures(line)?;
    let file_name = caps.get(1)?.as_str().trim();
    if LOCK_RE.is_match(file_name) {
        return None;
    }
    let change_count = caps.get(2)?.as_str().trim().parse::<u32>().ok()?;
    (change_count < 1000).then(|| file_name.to_string())
}

#[derive(Default)]
struct DiffLineProcessor {
    in_ipynb_source: bool,
}

impl DiffLineProcessor {
    fn process_diff_line(&mut self, line: DiffLine, file_path: &str) -> Option<String> {
        let text = from_utf8(line.content()).ok()?;

        if !file_path.ends_with(".ipynb") {
            if text.contains("\"image/png\"") {
                return None;
            }
            return match line.origin() {
                'F' | 'H' | 'B' | ' ' => Some(text.to_string()),
                '+' | '-' => Some(format!("{}{}", line.origin(), text)),
                _ => None,
            };
        }

        let trimmed = text.trim();
        if trimmed.starts_with("\"source\": [") {
            self.in_ipynb_source = true;
            return None;
        }
        if !self.in_ipynb_source {
            return None;
        }
        if trimmed.starts_with(']') {
            self.in_ipynb_source = false;
            return None;
        }
        let cleaned = trimmed.trim_matches('"').replace("\\n", "");
        Some(format!("{}{}\n", line.origin(), cleaned))
    }
}
