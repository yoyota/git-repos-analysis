use anyhow::Result;
use git2::Repository;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};

const CLONE_PATH_FILE: &str = "/home/yoyota/hobby/git-repos-analysis/clone_path.txt";

fn main() {
    if let Err(e) = process_repos() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn process_repos() -> Result<()> {
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

fn process_commits(repo_path: &str) -> Result<HashSet<String>> {
    let repo = Repository::open(repo_path)?;
    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)?;
    revwalk.push_glob("refs/*")?;

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
