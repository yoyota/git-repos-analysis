use git2::Repository;
use git2::{build::RepoBuilder, FetchOptions, RemoteCallbacks};

use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

fn main() {
    let file_path = "/home/yoyota/hobby/git-repos-analysis/projects.txt";
    let file = File::open(file_path).unwrap();
    let reader = io::BufReader::new(file);

    let fetch_options = create_fetch_options();
    let mut repo_builder = RepoBuilder::new();
    repo_builder.fetch_options(fetch_options).bare(false);

    let mut clone_path_file = OpenOptions::new()
        .write(true)
        .create(true)
        .open("clone_path.txt")
        .unwrap();

    for line in reader.lines() {
        let project_url = line.unwrap();
        let clone_path = create_clone_dir(project_url.clone());
        if let Some(repo) = repo_builder.clone(&project_url, &clone_path).ok() {
            fetch_all_remote_branch(repo);
            writeln!(clone_path_file, "{}", clone_path.to_string_lossy()).unwrap();
        } else {
            println!("{} clone fail", project_url);
        }
    }
}

fn create_clone_dir(project_url: String) -> PathBuf {
    let project_path = project_url.replace("https://gitlab.com/", "");
    let base_path = dirs::home_dir().expect("could not find home directory");
    let clone_path = base_path.join("Documents").join(project_path);
    if clone_path.exists() {
        fs::remove_dir_all(&clone_path).unwrap();
    }
    fs::create_dir_all(&clone_path).unwrap();
    clone_path
}

fn fetch_all_remote_branch(repo: Repository) {
    let mut remote = repo.find_remote("origin").unwrap();
    let mut fetch_options = create_fetch_options();
    remote
        .fetch(
            &["+refs/heads/*:refs/remotes/origin/*"],
            Some(&mut fetch_options),
            None,
        )
        .unwrap();
}

fn create_fetch_options<'a>() -> FetchOptions<'a> {
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|url, username_from_url, _allowed_types| {
        let config = git2::Config::open_default().expect("Failed to open Git config");
        git2::Cred::credential_helper(&config, url, username_from_url)
    });

    let mut fetch_options = FetchOptions::new();
    fetch_options.remote_callbacks(callbacks); // Transfer ownership of callbacks
    return fetch_options;
}
