use chrono::{TimeZone, Utc};
use git2::{Commit, DiffLine, DiffOptions, DiffStats, DiffStatsFormat, Repository};
use regex::Regex;
use std::fs::{metadata, remove_file, rename, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::str::from_utf8;
use std::sync::LazyLock;

const CLONE_PATH_FILE: &str = "/home/yoyota/hobby/git-repos-analysis/clone_path.txt";
const OUTPUT_DIR: &str = "/home/yoyota/hobby/git-repos-analysis";

static FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([^\|]+?)\s*\|\s*(\S+)").unwrap());

static LOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|.*/)(yarn\.lock|poetry\.lock|package-lock\.json|\.terraform\.lock\.hcl)$")
        .unwrap()
});

fn other_err(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

fn main() {
    if let Err(e) = process_repos() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn process_repos() -> io::Result<()> {
    let reader = BufReader::new(File::open(CLONE_PATH_FILE)?);
    for line in reader.lines() {
        process_repo(line?.trim())?;
    }
    Ok(())
}

fn process_repo(repo_path: &str) -> io::Result<()> {
    let tmp_filename = format!("{}/{}.txt", OUTPUT_DIR, repo_path.replace("/", "|"));
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&tmp_filename)?;
    let mut writer = BufWriter::new(file);

    let header_len = write_header(&mut writer, repo_path)?;
    process_commits(&mut writer, repo_path)?;
    writer.flush()?;

    let file_size = metadata(&tmp_filename)?.len();
    if file_size == header_len {
        remove_file(&tmp_filename)?;
    } else {
        let new_filename = format!(
            "{}/{:0>10}_{}.txt",
            OUTPUT_DIR,
            file_size,
            repo_path.replace("/", "|")
        );
        rename(&tmp_filename, &new_filename)?;
    }
    Ok(())
}

fn write_header(writer: &mut BufWriter<File>, repo_path: &str) -> io::Result<u64> {
    let mut parts = repo_path.rsplit('/');
    let name = parts.next().unwrap_or("");
    let parent = parts.next().unwrap_or("");
    let header = format!("project name: {parent}/{name}\n\n");
    write!(writer, "{header}")?;
    Ok(header.len() as u64)
}

fn process_commits(writer: &mut BufWriter<File>, repo_path: &str) -> io::Result<()> {
    let repo = Repository::open(repo_path).map_err(other_err)?;
    let mut revwalk = repo.revwalk().map_err(other_err)?;
    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)
        .map_err(other_err)?;
    revwalk.push_glob("refs/*").map_err(other_err)?;

    for oid in revwalk.filter_map(Result::ok) {
        if let Ok(commit) = repo.find_commit(oid) {
            process_commit(&repo, &commit, writer)?;
        }
    }
    Ok(())
}

fn process_commit(
    repo: &Repository,
    commit: &Commit,
    writer: &mut BufWriter<File>,
) -> io::Result<()> {
    if !is_commit_valid(commit) {
        return Ok(());
    }
    for line in get_diff_lines(repo, commit)? {
        write!(writer, "{}", line)?;
    }
    Ok(())
}

fn is_commit_valid(commit: &Commit) -> bool {
    commit.parent_count() <= 1
        && commit
            .author()
            .name()
            .is_some_and(|name| name == "yoyota" || name == "YongTak Yoo")
}

fn get_diff_lines(repo: &Repository, commit: &Commit) -> io::Result<Vec<String>> {
    let old_tree = commit
        .parents()
        .next()
        .map(|parent| parent.tree())
        .transpose()
        .map_err(other_err)?;

    let tree = commit.tree().map_err(other_err)?;

    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&tree), None)
        .map_err(other_err)?;

    let stats = diff.stats().map_err(other_err)?;
    let mut opts = prepare_diff_options(&stats)?;

    let diff_out = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&tree), Some(&mut opts))
        .map_err(other_err)?;

    let commit_time = Utc
        .timestamp_opt(commit.time().seconds(), 0)
        .single()
        .ok_or_else(|| other_err("Invalid timestamp"))?;

    let mut lines = vec![
        format!(
            "commit date: {}\n",
            commit_time.format("%Y-%m-%d %H:%M:%S UTC")
        ),
        format!("commit message: {}\n", commit.message().unwrap_or("")),
    ];

    let mut processor = DiffLineProcessor::default();
    let mut current_file = String::new();

    diff_out
        .print(git2::DiffFormat::Patch, |delta, _, line| {
            if let Some(path) = delta.new_file().path().or(delta.old_file().path()) {
                current_file = path.to_string_lossy().to_string();
            }
            if let Some(result) = processor.process_diff_line(line, &current_file) {
                lines.push(result);
            }
            true
        })
        .map_err(other_err)?;

    Ok(lines)
}

fn prepare_diff_options(stats: &DiffStats) -> io::Result<DiffOptions> {
    let mut opts = DiffOptions::new();
    opts.context_lines(5)
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

fn filter_out_large_change_files(stats: &DiffStats) -> io::Result<Vec<String>> {
    if stats.insertions() + stats.deletions() > 2000 {
        return Ok(vec![]);
    }

    let buf = stats.to_buf(DiffStatsFormat::FULL, 100).map_err(other_err)?;
    let stats_str = buf
        .as_str()
        .ok_or_else(|| other_err("Invalid stats buffer"))?;

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
