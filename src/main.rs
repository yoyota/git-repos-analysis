use git2::{build::RepoBuilder, FetchOptions, RemoteCallbacks};

use std::fs;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;

fn main() -> io::Result<()> {
    let file_path = "projects.txt";
    let file = File::open(file_path)?;
    let reader = io::BufReader::new(file);
    let mut repo_builder = create_repo_builder();

    for line in reader.lines().take(1) {
        let project_url = line?;
        let project_path = project_url.clone().replace("https://gitlab.com/", "");
        let base_path = Path::new("/tmp");
        let clone_path = base_path.join(project_path);
        fs::create_dir_all(&clone_path)?;

        match repo_builder.clone(&project_url, &clone_path) {
            Ok(repo) => println!("Successfully cloned to {}", repo.path().display()),
            Err(e) => eprintln!("Failed to clone: {}", e),
        }
    }

    Ok(())
}

fn create_repo_builder<'a>() -> RepoBuilder<'a> {
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|url, username_from_url, _allowed_types| {
        let config = git2::Config::open_default().expect("Failed to open Git config");
        git2::Cred::credential_helper(&config, url, username_from_url)
    });

    let mut fetch_options = FetchOptions::new();
    fetch_options.remote_callbacks(callbacks); // Transfer ownership of callbacks

    let mut repo_builder = RepoBuilder::new();
    repo_builder.fetch_options(fetch_options);

    repo_builder
}
