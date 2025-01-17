use regex::Regex;

use git2::{DiffOptions, DiffStatsFormat, Oid, Repository};

use std::fs::File;
use std::io::{self, BufRead};

fn main() {
    let file_path = "/home/yoyota/hobby/gitlab-clone-to-local/clone_path.txt";
    let file = File::open(file_path).unwrap();
    let reader = io::BufReader::new(file);

    for line in reader.lines().take(1) {
        let repo_path = line.unwrap();
        let repo = Repository::open(repo_path).unwrap();
        let mut revwalk = repo.revwalk().unwrap();
        revwalk.push_glob("refs/*").unwrap();
        for oid in revwalk.skip(6).take(1) {
            print_diff(&repo, oid.unwrap());
        }
    }
}

fn print_diff(repo: &Repository, oid: Oid) {
    let commit = repo.find_commit(oid).unwrap();
    if let Some(parent_commit) = commit.parents().next() {
        let tree = commit.tree().unwrap();
        let parent_tree = parent_commit.tree().unwrap();

        let mut diff_options = DiffOptions::new();
        diff_options
            .context_lines(5)
            .ignore_blank_lines(true)
            .ignore_whitespace(true)
            .ignore_whitespace_change(true)
            .ignore_whitespace_eol(true);

        let diff = repo
            .diff_tree_to_tree(Some(&parent_tree), Some(&tree), Some(&mut diff_options))
            .unwrap();

        let stats = diff.stats().unwrap();

        let b = stats.to_buf(DiffStatsFormat::FULL, 100).unwrap();

        let s = b.as_str().unwrap();
        for l in s.split("\n") {
            println!("{}", l);
            let re = Regex::new(r"([^\|]+?)\s*\|\s*(\S+)").unwrap();
            if let Some(caps) = re.captures(l) {
                let file_name = caps[1].trim(); // Capture file name and trim whitespace
                let status = caps[2].trim(); // Capture status and trim whitespace
                println!("File: '{}', Status: '{}'", file_name, status);
            }
        }

        // if let Err(e) = diff.print(git2::DiffFormat::Patch, |_, _, line| {
        //     if printed_lines >= max_lines {
        //         return false; // Stop printing further lines
        //     }
        //     let text = str::from_utf8(line.content()).unwrap();
        //     print!("{}{}", line.origin(), text);
        //     printed_lines += line.num_lines();

        //     true
        // }) {
        //     if e.code() == ErrorCode::User {
        //         return;
        //     }
        //     panic!("Error printing diff: {}", e);
        // }
    }
}

//         // Print the diff
//     }
//  else {
//     // If there's no parent, show the diff for the initial commit
//     let mut diff_options = DiffOptions::new();
//     let diff = repo
//         .diff_tree_to_tree(None, Some(&tree), Some(&mut diff_options))
//         .unwrap();

//     diff.print(git2::DiffFormat::Patch, |delta, _, line| {
//         println!(
//             "{}: {:?}",
//             delta.new_file().path().unwrap().to_string_lossy(),
//             line.content()
//         );
//         true
//     })
//     .unwrap();
// }
