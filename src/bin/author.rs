use git2::Repository;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
const CLONE_PATH_FILE: &str = "/home/yoyota/hobby/git-repos-analysis/clone_path.txt";

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
    let mut merged_set = HashSet::new();
    for line in reader.lines() {
        let repo_path = line?;
        let iids = process_commits(repo_path.trim()).unwrap();
        merged_set = merged_set.union(&iids).cloned().collect();
    }
    for k in merged_set {
        println!("{}", k);
    }
    Ok(())
}

fn process_commits(repo_path: &str) -> io::Result<HashSet<String>> {
    let repo = Repository::open(repo_path).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let mut revwalk = repo
        .revwalk()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    revwalk
        .push_glob("refs/*")
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let mut iids = HashSet::new();
    for oid in revwalk.filter_map(Result::ok) {
        if let Ok(commit) = repo.find_commit(oid) {
            let author = commit.author();
            let email = author.email().unwrap();
            let name = author.name().unwrap();
            let iid = format!("{}|{}|{}", email, name, repo_path);
            iids.insert(iid);
        }
    }
    Ok(iids)
}
