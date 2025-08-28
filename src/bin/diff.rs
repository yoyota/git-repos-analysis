use chrono::{TimeZone, Utc};
use git2::{Commit, Diff, DiffOptions, DiffStats, DiffStatsFormat, Repository, Tree};
use regex::Regex;
use std::fs::{metadata, remove_file, rename, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::str::from_utf8;
use std::vec;

const CLONE_PATH_FILE: &str = "/home/yoyota/hobby/git-repos-analysis/clone_path.txt";
const OUTPUT_DIR: &str = "/home/yoyota/hobby/git-repos-analysis";

fn main() {
    if let Err(e) = process_repos() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn process_repos() -> io::Result<()> {
    let file = File::open(CLONE_PATH_FILE)?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let repo_path = line?;
        process_repo(repo_path.trim())?;
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
    let header = format!(
        "project name: {}/{}\n\n",
        repo_path.rsplit('/').nth(1).unwrap_or(""),
        repo_path.rsplit('/').next().unwrap_or("")
    );
    write!(writer, "{}", header)?;
    Ok(header.as_bytes().len() as u64)
}

fn process_commits(writer: &mut BufWriter<File>, repo_path: &str) -> io::Result<()> {
    let repo = Repository::open(repo_path).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    DiffProcessor::new(&repo).run(writer)
}

struct DiffProcessor<'repo> {
    repo: &'repo Repository,
}

impl<'repo> DiffProcessor<'repo> {
    fn new(repo: &'repo Repository) -> Self {
        Self { repo }
    }

    fn run(&self, writer: &mut BufWriter<File>) -> io::Result<()> {
        let mut revwalk = self
            .repo
            .revwalk()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        revwalk
            .set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        revwalk
            .push_glob("refs/*")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        for oid in revwalk.filter_map(Result::ok) {
            if let Ok(commit) = self.repo.find_commit(oid) {
                self.process_commit(&commit, writer)?;
            }
        }
        Ok(())
    }

    fn process_commit(
        &self,
        commit: &Commit<'repo>,
        writer: &mut BufWriter<File>,
    ) -> io::Result<()> {
        if self.is_commit_valid(commit) {
            let diff_lines = self.get_diff_lines(commit)?;
            for line in diff_lines {
                write!(writer, "{}", line)?;
            }
        }
        Ok(())
    }

    fn is_commit_valid(&self, commit: &Commit<'repo>) -> bool {
        commit.parent_count() <= 1
            && commit
                .author()
                .name()
                .map_or(false, |name| name == "yoyota" || name == "YongTak Yoo")
    }

    fn get_diff_lines(&self, commit: &Commit<'repo>) -> io::Result<Vec<String>> {
        let old_tree = self.get_parent_tree(commit)?;
        let new_tree = commit
            .tree()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let diff = self
            .repo
            .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let stats = diff
            .stats()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let mut opts = self.prepare_diff_options(&stats)?;

        let diff_with_opts = self
            .repo
            .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let mut lines = self.format_commit_header(commit)?;
        lines.extend(self.format_patch(&diff_with_opts)?);

        Ok(lines)
    }

    fn get_parent_tree(&self, commit: &Commit<'repo>) -> io::Result<Option<Tree<'repo>>> {
        commit
            .parents()
            .next()
            .map(|p| p.tree())
            .transpose()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn format_commit_header(&self, commit: &Commit<'repo>) -> io::Result<Vec<String>> {
        let commit_time = Utc
            .timestamp_opt(commit.time().seconds(), 0)
            .single()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Invalid timestamp"))?;
        Ok(vec![
            format!(
                "commit date: {}\n",
                commit_time.format("%Y-%m-%d %H:%M:%S UTC")
            ),
            format!("commit message: {}\n", commit.message().unwrap_or("")),
        ])
    }

    fn format_patch(&self, diff: &Diff) -> io::Result<Vec<String>> {
        let mut lines = Vec::new();
        let mut processor = DiffLineProcessor::new();
        let mut current_file = String::new();

        diff.print(git2::DiffFormat::Patch, |delta, _, line| {
            if let Some(path) = delta.new_file().path().or(delta.old_file().path()) {
                current_file = path.to_string_lossy().to_string();
            }
            if let Some(result) = processor.process_diff_line(line, &current_file) {
                lines.push(result);
            }
            true
        })
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(lines)
    }

    fn prepare_diff_options(&self, stats: &DiffStats) -> io::Result<DiffOptions> {
        let mut opts = DiffOptions::new();
        opts.context_lines(5)
            .pathspec(" ")
            .ignore_blank_lines(true)
            .ignore_whitespace(true)
            .ignore_whitespace_change(true)
            .ignore_whitespace_eol(true);

        let files = self.filter_out_large_change_files(stats)?;
        for file in files {
            opts.pathspec(&file);
        }
        Ok(opts)
    }

    fn filter_out_large_change_files(&self, stats: &DiffStats) -> io::Result<Vec<String>> {
        let buf = stats
            .to_buf(DiffStatsFormat::FULL, 100)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        if stats.insertions() + stats.deletions() > 2000 {
            return Ok(vec![]);
        }

        let stats_str = buf
            .as_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Invalid stats buffer"))?;

        let file_re = Regex::new(r"([^\\|]+?)\\s*\\|\\s*(\\S+)")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let lock_re = Regex::new(
            r"(^|.*/)(yarn\\.lock|poetry\\.lock|package-lock\\.json|\\.terraform\\.lock\\.hcl)$",
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let files: Vec<String> = stats_str
            .lines()
            .filter_map(|line| self.parse_stats_line(line, &file_re, &lock_re))
            .collect();

        Ok(files)
    }

    fn parse_stats_line(&self, line: &str, file_re: &Regex, lock_re: &Regex) -> Option<String> {
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
}

use git2::DiffLine;

struct DiffLineProcessor {
    in_ipynb_source: bool,
}

impl DiffLineProcessor {
    fn new() -> Self {
        DiffLineProcessor {
            in_ipynb_source: false,
        }
    }

    fn process_diff_line(&mut self, line: DiffLine, file_path: &str) -> Option<String> {
        if file_path.ends_with(".ipynb") {
            return self.process_ipynb_line(line);
        }

        let text = from_utf8(line.content()).ok()?;
        if text.contains("\"image/png\"") {
            return None;
        }

        match line.origin() {
            'F' | 'H' | 'B' | ' ' => Some(text.to_string()),
            '+' | '-' => Some(format!("{}{}", line.origin(), text)),
            _ => None,
        }
    }

    fn process_ipynb_line(&mut self, line: DiffLine) -> Option<String> {
        let text = from_utf8(line.content()).ok()?;
        let trimmed = text.trim();

        if self.in_ipynb_source {
            if trimmed.starts_with(']') {
                self.in_ipynb_source = false;
                None
            } else {
                let cleaned_line = trimmed.trim_matches('"').replace("\\n", "");
                Some(format!("{}{}\n", line.origin(), cleaned_line))
            }
        } else {
            if trimmed.starts_with("\"source\": [") {
                self.in_ipynb_source = true;
            }
            None
        }
    }
}
