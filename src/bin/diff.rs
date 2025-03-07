use chrono::{TimeZone, Utc};
use git2::{Commit, DiffOptions, DiffStats, DiffStatsFormat, Repository};
use regex::Regex;
use std::fs::{metadata, remove_file, rename, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::str::from_utf8;

const CLONE_PATH_FILE: &str = "/home/yoyota/hobby/git-repos-analysis/clone_path.txt";
const OUTPUT_DIR: &str = "/home/yoyota/hobby/git-repos-analysis";

fn main() {
    if let Err(e) = process_repos() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

/// Reads repository paths (up to 10) from the clone path file and processes each.
fn process_repos() -> io::Result<()> {
    let file = File::open(CLONE_PATH_FILE)?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let repo_path = line?;
        process_repo(repo_path.trim())?;
    }
    Ok(())
}

/// Processes a single repository:
/// 1. Writes a header to a temporary file.
/// 2. Processes commits to append diff output.
/// 3. Checks file size and either renames or removes the file.
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

/// Writes the project header to the writer and returns its byte length.
fn write_header(writer: &mut BufWriter<File>, repo_path: &str) -> io::Result<u64> {
    let header = format!(
        "project name: {}/{}\n\n",
        repo_path.rsplit('/').nth(1).unwrap_or(""),
        repo_path.rsplit('/').next().unwrap_or("")
    );
    write!(writer, "{}", header)?;
    Ok(header.as_bytes().len() as u64)
}

/// Processes commits in the repository by iterating through them, validating each commit,
/// and processing valid commits.
fn process_commits(writer: &mut BufWriter<File>, repo_path: &str) -> io::Result<()> {
    let repo = Repository::open(repo_path).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let mut revwalk = repo
        .revwalk()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    revwalk
        .push_glob("refs/*")
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    for oid in revwalk.filter_map(Result::ok) {
        if let Ok(commit) = repo.find_commit(oid) {
            process_commit(&repo, &commit, writer)?;
        }
    }
    Ok(())
}

/// Processes a single commit if it meets the validation criteria.
/// If valid, retrieves the diff lines and writes them to the writer.
fn process_commit(
    repo: &Repository,
    commit: &Commit,
    writer: &mut BufWriter<File>,
) -> io::Result<()> {
    if is_commit_valid(commit) {
        let diff_lines = get_diff_lines(repo, commit)?;
        for line in diff_lines {
            write!(writer, "{}", line)?;
        }
    }
    Ok(())
}

/// Determines if a commit is valid based on having at most one parent and a matching author.
fn is_commit_valid(commit: &Commit) -> bool {
    commit.parent_count() <= 1
        && commit
            .author()
            .name()
            .map_or(false, |name| name == "yoyota" || name == "YongTak Yoo")
}

/// Returns diff lines for a commit including commit date, message, and diff output.
fn get_diff_lines(repo: &Repository, commit: &Commit) -> io::Result<Vec<String>> {
    let old_tree = commit
        .parents()
        .next()
        .map(|parent| {
            parent
                .tree()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
        })
        .transpose()?;
    let tree = commit
        .tree()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&tree), None)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let stats = diff
        .stats()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let mut opts = prepare_diff_options(&stats)?;
    let commit_time = Utc
        .timestamp_opt(commit.time().seconds(), 0)
        .single()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Invalid timestamp"))?;

    let mut lines = vec![
        format!(
            "commit date: {}\n",
            commit_time.format("%Y-%m-%d %H:%M:%S UTC")
        ),
        format!("commit message: {}\n", commit.message().unwrap_or("")),
    ];

    let diff_out = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&tree), Some(&mut opts))
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    diff_out
        .print(git2::DiffFormat::Patch, |_, _, line| {
            if let Some(result) = process_diff_line(line) {
                lines.push(result);
            }
            true
        })
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(lines)
}

/// Processes a single diff line and returns a formatted string if it should be kept.
/// Returns None if the line should be skipped.
fn process_diff_line(line: git2::DiffLine) -> Option<String> {
    let text = from_utf8(line.content()).ok()?;
    if text.contains("\"image/png\"") {
        None
    } else {
        Some(format!("{}{}", line.origin(), text))
    }
}

/// Prepares diff options including context lines and file filtering.
fn prepare_diff_options(stats: &DiffStats) -> io::Result<DiffOptions> {
    let mut opts = DiffOptions::new();
    opts.context_lines(5)
        .ignore_blank_lines(true)
        .ignore_whitespace(true)
        .ignore_whitespace_change(true)
        .ignore_whitespace_eol(true);

    let files = filter_out_large_change_files(stats)?;
    for file in files {
        opts.pathspec(&file);
    }
    Ok(opts)
}

/// Filters out files with large diffs or matching lock file patterns.
fn filter_out_large_change_files(stats: &DiffStats) -> io::Result<Vec<String>> {
    let buf = stats
        .to_buf(DiffStatsFormat::FULL, 100)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let stats_str = buf
        .as_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Invalid stats buffer"))?;

    let file_re = Regex::new(r"([^\|]+?)\s*\|\s*(\S+)")
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let lock_re =
        Regex::new(r"(^|.*/)(yarn\.lock|poetry\.lock|package-lock\.json|\.terraform\.lock\.hcl)$")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let files: Vec<String> = stats_str
        .lines()
        .filter_map(|line| parse_stats_line(line, &file_re, &lock_re))
        .collect();

    Ok(files)
}

fn parse_stats_line(line: &str, file_re: &Regex, lock_re: &Regex) -> Option<String> {
    let caps = file_re.captures(line)?;
    let file_name = caps.get(1)?.as_str().trim();
    if lock_re.is_match(file_name) {
        return None;
    }
    let change_count = caps.get(2)?.as_str().trim().parse::<u32>().ok()?;
    if change_count < 1000 {
        Some(file_name.to_string())
    } else {
        None
    }
}
