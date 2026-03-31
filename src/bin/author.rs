use git2::Repository;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader};

const CLONE_PATH_FILE: &str = "/home/yoyota/hobby/git-repos-analysis/clone_path.txt";

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
    let mut merged_set = HashSet::new();
    for line in reader.lines() {
        merged_set.extend(process_commits(line?.trim())?);
    }
    for k in merged_set {
        println!("{}", k);
    }
    Ok(())
}

fn process_commits(repo_path: &str) -> io::Result<HashSet<String>> {
    let repo = Repository::open(repo_path).map_err(other_err)?;
    let mut revwalk = repo.revwalk().map_err(other_err)?;
    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)
        .map_err(other_err)?;
    revwalk.push_glob("refs/*").map_err(other_err)?;

    let iids = revwalk
        .filter_map(Result::ok)
        .filter_map(|oid| repo.find_commit(oid).ok())
        .filter_map(|commit| {
            let author = commit.author();
            let iid = format!("{}|{}|{}", author.email()?, author.name()?, repo_path);
            Some(iid)
        })
        .collect();

    Ok(iids)
}
