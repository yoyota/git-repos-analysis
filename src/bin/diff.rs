use chrono::{TimeZone, Utc};
use git2::{Commit, DiffOptions, DiffStats, DiffStatsFormat, Repository};
use regex::Regex;
use std::fs::{metadata, remove_file, rename, File, OpenOptions};
use std::io::{self, BufRead, BufWriter, Write};
use std::str::from_utf8; // For date formatting

fn main() {
    let file_path = "/home/yoyota/hobby/git-repos-analysis/clone_path.txt";
    let file = File::open(file_path).unwrap();
    let reader = io::BufReader::new(file);

    for line in reader.lines().flatten() {
        let write_file_path = format!(
            "/home/yoyota/hobby/git-repos-analysis/{}.txt",
            line.replace("/", "|")
        );
        let open_opotions = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&write_file_path)
            .unwrap();

        let mut writer = BufWriter::new(open_opotions);

        let repo = Repository::open(&line).unwrap();
        let mut revwalk = repo.revwalk().unwrap();
        revwalk.push_glob("refs/*").unwrap();

        for commit in revwalk
            .filter_map(|r| r.ok())
            .filter_map(|oid| repo.find_commit(oid).ok())
            .filter(|commit| {
                commit
                    .author()
                    .name()
                    .map_or(false, |name| name == "yoyota" || name == "YongTak Yoo")
            })
            .filter(|commit| commit.parent_count() <= 1)
        {
            let lines = get_diff_lines(&repo, commit);
            lines.iter().for_each(|line| {
                write!(writer, "{}", line).unwrap();
            });
        }

        let file_size = metadata(&write_file_path).unwrap().len();

        if file_size == 0 {
            remove_file(&write_file_path).unwrap();
        } else {
            // Create new file name with size prepended
            let new_file_path = format!(
                "/home/yoyota/hobby/git-repos-analysis/{:0>10}_{}.txt",
                file_size,
                line.replace("/", "|"),
            );

            // Rename the file
            rename(&write_file_path, &new_file_path).unwrap();
        }
    }
}

fn get_diff_lines(repo: &Repository, commit: Commit) -> Vec<String> {
    let old_tree = commit.parents().next().map(|p| p.tree().unwrap());
    let tree = commit.tree().unwrap();

    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&tree), None)
        .unwrap();

    let stats = diff.stats().unwrap();

    let mut diff_options = DiffOptions::new();
    diff_options
        .pathspec(" ")
        .context_lines(5)
        .ignore_blank_lines(true)
        .ignore_whitespace(true)
        .ignore_whitespace_change(true)
        .ignore_whitespace_eol(true);

    filter_out_large_chang_files(stats)
        .iter()
        .for_each(|file_name| {
            diff_options.pathspec(&file_name);
        });

    let mut diff_lines = Vec::new();

    let timestamp = commit.time().seconds();
    let datetime = Utc.timestamp_opt(timestamp, 0).single().unwrap();
    let formatted_date = datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string();

    diff_lines.push(formatted_date + "\n");
    diff_lines.push(commit.message().unwrap_or("").to_string() + "\n");

    let filtered_diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&tree), Some(&mut diff_options))
        .unwrap();

    let _ = filtered_diff.print(git2::DiffFormat::Patch, |_, _, line| {
        if let Ok(text) = from_utf8(line.content()) {
            if !text.contains(&"\"image/png\"".to_string()) {
                diff_lines.push(format!("{}{}", line.origin(), text));
            }
        }
        true
    });
    diff_lines
}

fn filter_out_large_chang_files(stats: DiffStats) -> Vec<String> {
    let s = stats
        .to_buf(DiffStatsFormat::FULL, 100)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    let re = Regex::new(r"([^\|]+?)\s*\|\s*(\S+)").unwrap();
    let lock_regex =
        Regex::new(r"(^|.*/)(yarn\.lock|poetry\.lock|package-lock\.json|\.terraform\.lock\.hcl)$")
            .unwrap();

    s.split('\n')
        .filter_map(|line| re.captures(line))
        .filter_map(|caps| {
            let file_name = caps[1].trim().to_string();
            if lock_regex.is_match(&file_name) {
                return None;
            }
            let changes_stat_str = caps[2].trim();
            changes_stat_str
                .parse::<u32>()
                .map_or_else(|_| None, |cs| (cs < 1000).then(|| file_name))
        })
        .collect()
}
