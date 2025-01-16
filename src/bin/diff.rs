use git2::{Commit, DiffOptions, Oid, Repository};
use std::str;

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
        for oid in revwalk {
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
        let diff = repo
            .diff_tree_to_tree(Some(&parent_tree), Some(&tree), Some(&mut diff_options))
            .unwrap();
        diff.print(git2::DiffFormat::Patch, |_, _, line| {
            let text = str::from_utf8(line.content()).unwrap();
            if text.len() != 0 {
                println!("{}{}", line.origin(), text);
            }
            true
        })
        .unwrap();
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
