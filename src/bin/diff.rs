use git2::{DiffOptions, DiffStatsFormat, ErrorCode, Oid, Repository};
use regex::Regex;
use std::fs::File;
use std::io::{self, BufRead};
use std::str::from_utf8;

fn main() {
    let file_path = "/home/yoyota/hobby/gitlab-clone-to-local/clone_path.txt";
    let file = File::open(file_path).unwrap();
    let reader = io::BufReader::new(file);

    for line in reader.lines().take(1).flatten() {
        let repo = Repository::open(line).unwrap();
        let mut revwalk = repo.revwalk().unwrap();
        revwalk.push_glob("refs/*").unwrap();
        for oid in revwalk.skip(1).take(1) {
            print_diff(&repo, oid.unwrap());
        }
    }
}

fn print_diff(repo: &Repository, oid: Oid) {
    let commit = repo.find_commit(oid).unwrap();
    println!("{}", commit.id());

    let old_tree = commit.parents().next().map(|p| p.tree().unwrap());
    let tree = commit.tree().unwrap();

    let mut diff_options = DiffOptions::new();
    diff_options
        .context_lines(5)
        .ignore_blank_lines(true)
        .ignore_whitespace(true)
        .ignore_whitespace_change(true)
        .ignore_whitespace_eol(true);

    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&tree), Some(&mut diff_options))
        .unwrap();
    let stats = diff.stats().unwrap();

    let s = stats
        .to_buf(DiffStatsFormat::FULL, 100)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    let re = Regex::new(r"([^\|]+?)\s*\|\s*(\S+)").unwrap();

    s.split('\n')
        .filter_map(|line| re.captures(line))
        .filter_map(|caps| {
            let file_name = caps[1].trim().to_string();
            let changes_stat_str = caps[2].trim();
            changes_stat_str
                .parse::<u32>()
                .map_or_else(|_| None, |cs| (cs < 1000).then(|| file_name))
        })
        .for_each(|file_name| {
            diff_options.pathspec(&file_name);
        });

    diff_options.pathspec(" ");
    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&tree), Some(&mut diff_options))
        .unwrap();
    if let Err(e) = diff.print(git2::DiffFormat::Patch, |_, _, line| {
        let text = from_utf8(line.content()).unwrap();
        print!("{}{}", line.origin(), text);
        true
    }) {
        if e.code() == ErrorCode::User {
            return;
        }
        panic!("Error printing diff: {}", e);
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
