use git2::{build::RepoBuilder, FetchOptions, RemoteCallbacks};

use std::fs;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;

fn main() {
    let file_path = "/home/yoyota/hobby/gitlab-clone-to-local/projects.txt";
    let file = File::open(file_path).unwrap();
    let reader = io::BufReader::new(file);

    let fetch_options = create_fetch_options();
    let mut repo_builder = RepoBuilder::new();
    repo_builder.fetch_options(fetch_options).bare(false);

    for line in reader.lines().skip(2).take(1) {
        let project_url = line.unwrap();
        let project_path = project_url.clone().replace("https://gitlab.com/", "");
        let base_path = Path::new("/tmp");
        let clone_path = base_path.join(project_path);
        if clone_path.exists() {
            fs::remove_dir_all(&clone_path).unwrap();
        }
        fs::create_dir_all(&clone_path).unwrap();
        let repo = repo_builder.clone(&project_url, &clone_path).unwrap();
        println!("{}", clone_path.display());

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
