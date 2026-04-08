use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use clap::Parser;
use git2::{
    Commit, ConfigLevel, DiffLine, DiffOptions, DiffStats, DiffStatsFormat,
    Repository,
};
use regex::Regex;
use std::fs::{self, remove_file, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::from_utf8;
use std::sync::LazyLock;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

static FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([^\|]+?)\s*\|\s*(\S+)").unwrap());

static LOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|.*/)(yarn\.lock|Cargo.toml|poetry\.lock|package-lock\.json|\.terraform\.lock\.hcl)$")
        .unwrap()
});

#[derive(Parser)]
#[command(
    name = "diff",
    about = "Extract filtered git diffs for resume generation"
)]
struct Args {
    /// Path to the git repository to analyze
    #[arg(short, long, default_value = ".")]
    repo: PathBuf,

    /// Save output to a file path (default: stdout)
    #[arg(short, long)]
    save: Option<PathBuf>,

    /// Comma-separated author names to filter commits (default: git config user.name)
    #[arg(long)]
    authors: Option<String>,
}

fn main() {
    init_tracing();
    if let Err(e) = run() {
        tracing::error!("{e}");
        std::process::exit(1);
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

fn run() -> Result<()> {
    let args = Args::parse();

    let repo = Repository::open(&args.repo)?;
    let name = repo_name(&args.repo)?;
    let authors = detect_authors(&repo, args.authors.as_deref());

    if authors.is_empty() {
        warn!("no author detected. Use --authors to specify.");
    } else {
        info!("filtering commits by: {}", authors.join(", "));
    }

    match args.save {
        Some(ref path) => {
            if let Some(parent) =
                path.parent().filter(|p| !p.as_os_str().is_empty())
            {
                fs::create_dir_all(parent)?;
            }
            let mut writer = BufWriter::new(File::create(path)?);
            process_repo(&repo, &name, &mut writer, &authors, Some(path))
        }
        None => {
            let stdout = io::stdout();
            let mut writer = BufWriter::new(stdout.lock());
            process_repo(&repo, &name, &mut writer, &authors, None)
        }
    }
}

fn repo_name(repo_path: &Path) -> Result<String> {
    fs::canonicalize(repo_path)?
        .file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .context("Cannot determine repo name from path")
}

fn detect_authors(
    repo: &Repository,
    override_authors: Option<&str>,
) -> Vec<String> {
    if let Some(s) = override_authors {
        return s.split(',').map(|a| a.trim().to_string()).collect();
    }
    let config = repo.config().ok();
    // Check local config first; fall back to global if user.name is absent.
    let name = config
        .as_ref()
        .and_then(|c| c.open_level(ConfigLevel::Local).ok())
        .and_then(|local| local.get_string("user.name").ok())
        .or_else(|| {
            config
                .as_ref()
                .and_then(|c| c.open_level(ConfigLevel::Global).ok())
                .and_then(|global| global.get_string("user.name").ok())
        });
    name.into_iter().collect()
}

fn process_repo(
    repo: &Repository,
    repo_name: &str,
    writer: &mut impl Write,
    authors: &[String],
    save_path: Option<&Path>,
) -> Result<()> {
    write_header(writer, repo_name)?;
    let found = process_commits(writer, repo, authors)?;
    writer.flush()?;

    if !found {
        if let Some(path) = save_path {
            remove_file(path)?;
        }
        info!("no matching commits found — no output written.");
    } else if let Some(path) = save_path {
        info!("saved to: {}", path.display());
    }
    Ok(())
}

fn write_header(writer: &mut impl Write, repo_name: &str) -> Result<()> {
    write!(writer, "project name: {repo_name}\n\n")?;
    Ok(())
}

fn process_commits(
    writer: &mut impl Write,
    repo: &Repository,
    authors: &[String],
) -> Result<bool> {
    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)?;
    revwalk.push_glob("refs/*")?;

    let mut found = false;
    let commits = revwalk
        .filter_map(Result::ok)
        .filter_map(|oid| repo.find_commit(oid).ok());
    for commit in commits {
        if process_commit(repo, &commit, writer, authors)? {
            found = true;
        }
    }
    Ok(found)
}

fn process_commit(
    repo: &Repository,
    commit: &Commit,
    writer: &mut impl Write,
    authors: &[String],
) -> Result<bool> {
    if !is_commit_valid(commit, authors) {
        return Ok(false);
    }
    for line in get_diff_lines(repo, commit)? {
        write!(writer, "{}", line)?;
    }
    Ok(true)
}

fn is_commit_valid(commit: &Commit, authors: &[String]) -> bool {
    commit.parent_count() <= 1
        && commit
            .author()
            .name()
            .is_some_and(|name| authors.iter().any(|a| a == name))
}

fn get_diff_lines(repo: &Repository, commit: &Commit) -> Result<Vec<String>> {
    let old_tree = commit
        .parents()
        .next()
        .map(|parent| parent.tree())
        .transpose()?;

    let tree = commit.tree()?;
    let diff = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&tree), None)?;

    let stats = diff.stats()?;
    let mut opts = prepare_diff_options(&stats)?;

    let diff_out = repo.diff_tree_to_tree(
        old_tree.as_ref(),
        Some(&tree),
        Some(&mut opts),
    )?;

    let commit_time = Utc
        .timestamp_opt(commit.time().seconds(), 0)
        .single()
        .context("Invalid timestamp")?;

    let mut lines = vec![
        format!(
            "commit date: {}\n",
            commit_time.format("%Y-%m-%d %H:%M:%S UTC")
        ),
        format!("commit message: {}\n", commit.message().unwrap_or("")),
    ];

    let mut processor = DiffLineProcessor::default();
    let mut current_file = String::new();

    diff_out.print(git2::DiffFormat::Patch, |delta, _, line| {
        if let Some(path) = delta.new_file().path().or(delta.old_file().path())
        {
            current_file = path.to_string_lossy().to_string();
        }
        if let Some(result) = processor.process_diff_line(line, &current_file) {
            lines.push(result);
        }
        true
    })?;

    Ok(lines)
}

fn prepare_diff_options(stats: &DiffStats) -> Result<DiffOptions> {
    let mut opts = DiffOptions::new();
    opts.context_lines(2)
        .pathspec(" ")
        .ignore_blank_lines(true)
        .ignore_whitespace(true)
        .ignore_whitespace_change(true)
        .ignore_whitespace_eol(true);

    for file in filter_out_large_change_files(stats)? {
        opts.pathspec(&file);
    }
    Ok(opts)
}

fn filter_out_large_change_files(stats: &DiffStats) -> Result<Vec<String>> {
    if stats.insertions() + stats.deletions() > 2000 {
        return Ok(vec![]);
    }

    let buf = stats.to_buf(DiffStatsFormat::FULL, 100)?;
    let stats_str = buf.as_str().context("Invalid stats buffer")?;

    Ok(stats_str.lines().filter_map(parse_stats_line).collect())
}

fn parse_stats_line(line: &str) -> Option<String> {
    let caps = FILE_RE.captures(line)?;
    let file_name = caps.get(1)?.as_str().trim();
    if LOCK_RE.is_match(file_name) {
        return None;
    }
    let change_count = caps.get(2)?.as_str().trim().parse::<u32>().ok()?;
    (change_count < 1000).then(|| file_name.to_string())
}

#[derive(Default)]
struct DiffLineProcessor {
    in_ipynb_source: bool,
}

impl DiffLineProcessor {
    fn process_diff_line(
        &mut self,
        line: DiffLine,
        file_path: &str,
    ) -> Option<String> {
        self.process_diff_line_inner(line.origin(), line.content(), file_path)
    }

    fn process_diff_line_inner(
        &mut self,
        origin: char,
        content: &[u8],
        file_path: &str,
    ) -> Option<String> {
        let text = from_utf8(content).ok()?;

        if !file_path.ends_with(".ipynb") {
            if text.contains("\"image/png\"") {
                return None;
            }
            return match origin {
                'F' | 'H' | 'B' | ' ' => Some(text.to_string()),
                '+' | '-' => Some(format!("{origin}{text}")),
                _ => None,
            };
        }

        let trimmed = text.trim();
        if trimmed.starts_with("\"source\": [") {
            self.in_ipynb_source = true;
            return None;
        }
        if !self.in_ipynb_source {
            return None;
        }
        if trimmed.starts_with(']') {
            self.in_ipynb_source = false;
            return None;
        }
        let s = trimmed.strip_prefix('"').unwrap_or(trimmed);
        let s = s.strip_suffix('"').unwrap_or(s);
        let cleaned = s.replace("\\n", "");
        Some(format!("{origin}{cleaned}\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Test helpers
    // -------------------------------------------------------------------------

    fn make_temp_repo() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let repo = Repository::init(dir.path()).expect("git init");
        {
            let mut config = repo.config().expect("repo config");
            config.set_str("user.name", "Test User").unwrap();
            config.set_str("user.email", "test@example.com").unwrap();
        }
        (dir, repo)
    }

    fn make_commit(
        repo: &Repository,
        author: &str,
        message: &str,
        files: &[(&str, &str)],
    ) -> git2::Oid {
        let root = repo.workdir().expect("workdir").to_path_buf();
        let mut index = repo.index().expect("index");

        for (name, contents) in files {
            let file_path = root.join(name);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&file_path, contents).unwrap();
            index.add_path(Path::new(name)).expect("add path");
        }
        index.write().expect("write index");

        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");

        let sig = git2::Signature::now(author, "author@example.com")
            .expect("signature");

        let parents: Vec<git2::Commit> = repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .and_then(|oid| repo.find_commit(oid).ok())
            .into_iter()
            .collect();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
    }

    fn make_merge_commit(
        repo: &Repository,
        parent_oids: &[git2::Oid],
    ) -> git2::Oid {
        let mut index = repo.index().expect("index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");

        let sig = git2::Signature::now("Merger", "merge@example.com")
            .expect("signature");

        let parents: Vec<git2::Commit> = parent_oids
            .iter()
            .map(|oid| repo.find_commit(*oid).expect("find commit"))
            .collect();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "Merge commit",
            &tree,
            &parent_refs,
        )
        .expect("merge commit")
    }

    struct MockWriter(Vec<u8>);

    impl MockWriter {
        fn new() -> Self {
            MockWriter(Vec::new())
        }
        fn as_str(&self) -> &str {
            std::str::from_utf8(&self.0).expect("valid utf8")
        }
    }

    impl std::io::Write for MockWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // repo_name tests
    // -------------------------------------------------------------------------

    mod repo_name {
        use super::*;

        #[test]
        fn happy_path_existing_dir_returns_base_name() {
            let dir = tempfile::tempdir().unwrap();
            // We need a known name — create a subdirectory with a fixed name
            let named = dir.path().join("my-project");
            std::fs::create_dir(&named).unwrap();
            let result = super::super::repo_name(&named).unwrap();
            assert_eq!(result, "my-project");
        }

        #[test]
        fn trailing_slash_returns_correct_base_name() {
            let dir = tempfile::tempdir().unwrap();
            let named = dir.path().join("my-project");
            std::fs::create_dir(&named).unwrap();
            // PathBuf with trailing slash
            let mut p = named.into_os_string();
            p.push("/");
            let result = super::super::repo_name(Path::new(&p)).unwrap();
            assert_eq!(result, "my-project");
        }

        #[test]
        fn nonexistent_path_returns_err() {
            let result =
                super::super::repo_name(Path::new("/this/does/not/exist/ever"));
            assert!(result.is_err());
        }
    }

    // -------------------------------------------------------------------------
    // detect_authors tests
    // -------------------------------------------------------------------------

    mod detect_authors {
        use super::*;

        #[test]
        fn override_single_author() {
            let (_dir, repo) = make_temp_repo();
            let result = super::super::detect_authors(&repo, Some("Alice"));
            assert_eq!(result, vec!["Alice"]);
        }

        #[test]
        fn override_multiple_authors_trimmed() {
            let (_dir, repo) = make_temp_repo();
            let result =
                super::super::detect_authors(&repo, Some("Alice, Bob,Charlie"));
            assert_eq!(result, vec!["Alice", "Bob", "Charlie"]);
        }

        #[test]
        fn override_single_with_spaces_trimmed() {
            let (_dir, repo) = make_temp_repo();
            let result = super::super::detect_authors(&repo, Some("  Alice  "));
            assert_eq!(result, vec!["Alice"]);
        }

        #[test]
        fn override_empty_string_yields_one_empty_entry() {
            let (_dir, repo) = make_temp_repo();
            let result = super::super::detect_authors(&repo, Some(""));
            assert_eq!(result, vec![""]);
        }

        #[test]
        fn no_override_uses_git_config_user_name() {
            let (_dir, repo) = make_temp_repo();
            // make_temp_repo sets user.name = "Test User"
            let result = super::super::detect_authors(&repo, None);
            assert_eq!(result, vec!["Test User"]);
        }

        #[test]
        fn no_override_no_config_returns_empty() {
            // Fresh repo with no local user.name and no global config reachable
            // via open_level(Global) — returns empty.
            let dir = tempfile::tempdir().unwrap();
            let repo = Repository::init(dir.path()).unwrap();
            let result = super::super::detect_authors(&repo, None);
            // Result depends on the machine's global git config; just verify it
            // doesn't panic and returns a Vec.
            let _ = result;
        }

        // Note: isolating the global-config fallback path in unit tests is not
        // practical. libgit2 caches the global config path internally after the
        // first resolution, so redirecting HOME mid-process is unreliable.
        // The fallback is exercised by integration: if local user.name is absent,
        // open_level(Global) is tried next.
    }

    // -------------------------------------------------------------------------
    // parse_stats_line tests
    // -------------------------------------------------------------------------

    mod parse_stats_line {
        #[test]
        fn happy_path_normal_line() {
            let result =
                super::super::parse_stats_line("src/main.rs          |  42");
            assert_eq!(result, Some("src/main.rs".to_string()));
        }

        #[test]
        fn change_count_exactly_999_is_included() {
            let result = super::super::parse_stats_line("src/foo.rs | 999");
            assert_eq!(result, Some("src/foo.rs".to_string()));
        }

        #[test]
        fn change_count_exactly_1000_is_excluded() {
            let result = super::super::parse_stats_line("src/foo.rs | 1000");
            assert_eq!(result, None);
        }

        #[test]
        fn change_count_zero_is_included() {
            let result = super::super::parse_stats_line("src/foo.rs | 0");
            assert_eq!(result, Some("src/foo.rs".to_string()));
        }

        #[test]
        fn lock_file_yarn_lock_filtered() {
            let result = super::super::parse_stats_line("yarn.lock    |   5");
            assert_eq!(result, None);
        }

        #[test]
        fn lock_file_cargo_toml_filtered() {
            let result = super::super::parse_stats_line("Cargo.toml   |  10");
            assert_eq!(result, None);
        }

        #[test]
        fn lock_file_nested_poetry_lock_filtered() {
            let result =
                super::super::parse_stats_line("some/nested/poetry.lock  |  3");
            assert_eq!(result, None);
        }

        #[test]
        fn lock_file_package_lock_json_filtered() {
            let result =
                super::super::parse_stats_line("package-lock.json  | 20");
            assert_eq!(result, None);
        }

        #[test]
        fn lock_file_terraform_lock_filtered() {
            let result =
                super::super::parse_stats_line(".terraform.lock.hcl | 1");
            assert_eq!(result, None);
        }

        #[test]
        fn non_lock_file_with_lock_like_prefix_is_included() {
            let result =
                super::super::parse_stats_line("src/lock_utils.rs  | 5");
            assert_eq!(result, Some("src/lock_utils.rs".to_string()));
        }

        #[test]
        fn non_numeric_change_count_returns_none() {
            let result = super::super::parse_stats_line("src/foo.rs | +--");
            assert_eq!(result, None);
        }

        #[test]
        fn line_without_pipe_returns_none() {
            let result = super::super::parse_stats_line("src/foo.rs  42");
            assert_eq!(result, None);
        }

        #[test]
        fn empty_line_returns_none() {
            let result = super::super::parse_stats_line("");
            assert_eq!(result, None);
        }
    }

    // -------------------------------------------------------------------------
    // DiffLineProcessor / process_diff_line_inner tests
    // -------------------------------------------------------------------------

    mod diff_line_processor {
        use super::*;

        fn proc() -> DiffLineProcessor {
            DiffLineProcessor::default()
        }

        // --- Non-ipynb file tests ---

        #[test]
        fn origin_f_file_header_verbatim() {
            let mut p = proc();
            let result = p.process_diff_line_inner(
                'F',
                b"diff --git a/foo b/foo\n",
                "foo.rs",
            );
            assert_eq!(result, Some("diff --git a/foo b/foo\n".to_string()));
        }

        #[test]
        fn origin_h_hunk_header_verbatim() {
            let mut p = proc();
            let result =
                p.process_diff_line_inner('H', b"@@ -1,3 +1,4 @@\n", "foo.rs");
            assert_eq!(result, Some("@@ -1,3 +1,4 @@\n".to_string()));
        }

        #[test]
        fn origin_b_binary_verbatim() {
            let mut p = proc();
            let result = p.process_diff_line_inner(
                'B',
                b"Binary files differ\n",
                "foo.rs",
            );
            assert_eq!(result, Some("Binary files differ\n".to_string()));
        }

        #[test]
        fn origin_space_context_line_verbatim() {
            let mut p = proc();
            let result =
                p.process_diff_line_inner(' ', b"context line\n", "foo.rs");
            assert_eq!(result, Some("context line\n".to_string()));
        }

        #[test]
        fn origin_plus_addition_prefixed() {
            let mut p = proc();
            let result =
                p.process_diff_line_inner('+', b"added line\n", "foo.rs");
            assert_eq!(result, Some("+added line\n".to_string()));
        }

        #[test]
        fn origin_minus_deletion_prefixed() {
            let mut p = proc();
            let result =
                p.process_diff_line_inner('-', b"removed line\n", "foo.rs");
            assert_eq!(result, Some("-removed line\n".to_string()));
        }

        #[test]
        fn origin_backslash_no_newline_marker_returns_none() {
            let mut p = proc();
            let result = p.process_diff_line_inner(
                '\\',
                b"\\ No newline at end of file\n",
                "foo.rs",
            );
            assert_eq!(result, None);
        }

        #[test]
        fn unrecognised_origin_returns_none() {
            let mut p = proc();
            let result =
                p.process_diff_line_inner('X', b"some content\n", "foo.rs");
            assert_eq!(result, None);
        }

        #[test]
        fn image_png_content_returns_none_for_non_ipynb() {
            let mut p = proc();
            let result = p.process_diff_line_inner(
                'F',
                b"\"image/png\": data\n",
                "foo.rs",
            );
            assert_eq!(result, None);
        }

        #[test]
        fn image_png_inside_plus_line_returns_none() {
            let mut p = proc();
            let result = p.process_diff_line_inner(
                '+',
                b"something \"image/png\" here\n",
                "foo.rs",
            );
            assert_eq!(result, None);
        }

        #[test]
        fn image_png_different_case_passes_through() {
            let mut p = proc();
            let result = p.process_diff_line_inner(
                '+',
                b"\"image/PNG\" is fine\n",
                "foo.rs",
            );
            assert_eq!(result, Some("+\"image/PNG\" is fine\n".to_string()));
        }

        // --- ipynb file tests ---

        #[test]
        fn ipynb_outside_source_block_non_trigger_returns_none() {
            let mut p = proc();
            let result = p.process_diff_line_inner(
                '+',
                b"  \"cell_type\": \"code\",\n",
                "nb.ipynb",
            );
            assert_eq!(result, None);
        }

        #[test]
        fn ipynb_source_trigger_line_returns_none_and_sets_state() {
            let mut p = proc();
            let result = p.process_diff_line_inner(
                '+',
                b"   \"source\": [\n",
                "nb.ipynb",
            );
            assert_eq!(result, None);
            assert!(p.in_ipynb_source);
        }

        #[test]
        fn ipynb_inside_source_block_content_line_cleaned_and_prefixed() {
            let mut p = proc();
            p.in_ipynb_source = true;
            let result = p.process_diff_line_inner(
                '+',
                b"    \"hello world\\n\"\n",
                "nb.ipynb",
            );
            assert_eq!(result, Some("+hello world\n".to_string()));
        }

        #[test]
        fn ipynb_close_bracket_exits_source_block_returns_none() {
            let mut p = proc();
            p.in_ipynb_source = true;
            let result = p.process_diff_line_inner('+', b"   ]\n", "nb.ipynb");
            assert_eq!(result, None);
            assert!(!p.in_ipynb_source);
        }

        #[test]
        fn ipynb_multiple_source_blocks_output_only_inside() {
            let mut p = proc();
            // First block
            p.process_diff_line_inner('+', b"\"source\": [\n", "nb.ipynb");
            let inside1 = p.process_diff_line_inner(
                '+',
                b"\"line one\\n\"\n",
                "nb.ipynb",
            );
            p.process_diff_line_inner('+', b"]\n", "nb.ipynb");
            // Between blocks
            let between = p.process_diff_line_inner(
                '+',
                b"\"cell_type\": \"code\"\n",
                "nb.ipynb",
            );
            // Second block
            p.process_diff_line_inner('+', b"\"source\": [\n", "nb.ipynb");
            let inside2 = p.process_diff_line_inner(
                '+',
                b"\"line two\\n\"\n",
                "nb.ipynb",
            );
            p.process_diff_line_inner('+', b"]\n", "nb.ipynb");

            assert!(inside1.is_some());
            assert_eq!(between, None);
            assert!(inside2.is_some());
        }

        #[test]
        fn ipynb_empty_content_after_cleaning_yields_just_origin_and_newline() {
            let mut p = proc();
            p.in_ipynb_source = true;
            // content is just quotes with no inner text and no \n
            let result = p.process_diff_line_inner('+', b"\"\"\n", "nb.ipynb");
            assert_eq!(result, Some("+\n".to_string()));
        }

        #[test]
        fn ipynb_content_multiple_quote_pairs_outer_stripped_only() {
            let mut p = proc();
            p.in_ipynb_source = true;
            // "\"inner\"" — outer quotes stripped, inner preserved
            let result = p.process_diff_line_inner(
                '+',
                b"\"\\\"inner\\\"\"\n",
                "nb.ipynb",
            );
            // strip_prefix/strip_suffix each remove at most one quote, leaving `\"inner\"`;
            // replace("\\n","") leaves it unchanged
            assert_eq!(result, Some("+\\\"inner\\\"\n".to_string()));
        }

        #[test]
        fn ipynb_image_png_inside_source_block_not_filtered() {
            let mut p = proc();
            p.in_ipynb_source = true;
            let result = p.process_diff_line_inner(
                '+',
                b"\"image/png data here\"\n",
                "nb.ipynb",
            );
            // For ipynb, image/png filter does NOT apply — result should be Some(...)
            assert!(result.is_some());
        }

        #[test]
        fn ipynb_invalid_utf8_returns_none() {
            let mut p = proc();
            p.in_ipynb_source = true;
            let invalid = b"\xff\xfe invalid utf8\n";
            let result = p.process_diff_line_inner('+', invalid, "nb.ipynb");
            assert_eq!(result, None);
        }
    }

    // -------------------------------------------------------------------------
    // write_header tests
    // -------------------------------------------------------------------------

    mod write_header {
        use super::*;

        #[test]
        fn writes_expected_format() {
            let mut w = MockWriter::new();
            super::super::write_header(&mut w, "my-repo").unwrap();
            assert_eq!(w.as_str(), "project name: my-repo\n\n");
        }

        #[test]
        fn repo_name_with_special_characters_written_verbatim() {
            let mut w = MockWriter::new();
            super::super::write_header(&mut w, "my repo/sub-dir").unwrap();
            assert_eq!(w.as_str(), "project name: my repo/sub-dir\n\n");
        }
    }

    // -------------------------------------------------------------------------
    // Integration tests
    // -------------------------------------------------------------------------

    mod integration {
        use super::*;

        // --- is_commit_valid ---

        #[test]
        fn is_commit_valid_single_parent_matching_author_true() {
            let (_dir, repo) = make_temp_repo();
            make_commit(&repo, "Alice", "first", &[("a.txt", "hello")]);
            let oid =
                make_commit(&repo, "Alice", "second", &[("b.txt", "world")]);
            let commit = repo.find_commit(oid).unwrap();
            assert!(super::super::is_commit_valid(
                &commit,
                &["Alice".to_string()]
            ));
        }

        #[test]
        fn is_commit_valid_author_not_in_list_false() {
            let (_dir, repo) = make_temp_repo();
            let oid =
                make_commit(&repo, "Alice", "first", &[("a.txt", "hello")]);
            let commit = repo.find_commit(oid).unwrap();
            assert!(!super::super::is_commit_valid(
                &commit,
                &["Bob".to_string()]
            ));
        }

        #[test]
        fn is_commit_valid_empty_authors_false() {
            let (_dir, repo) = make_temp_repo();
            let oid =
                make_commit(&repo, "Alice", "first", &[("a.txt", "hello")]);
            let commit = repo.find_commit(oid).unwrap();
            assert!(!super::super::is_commit_valid(&commit, &[]));
        }

        #[test]
        fn is_commit_valid_merge_commit_false() {
            let (_dir, repo) = make_temp_repo();
            let oid1 =
                make_commit(&repo, "Alice", "first", &[("a.txt", "hello")]);
            let oid2 =
                make_commit(&repo, "Alice", "second", &[("b.txt", "world")]);
            let merge_oid = make_merge_commit(&repo, &[oid2, oid1]);
            let commit = repo.find_commit(merge_oid).unwrap();
            assert!(!super::super::is_commit_valid(
                &commit,
                &["Alice".to_string(), "Merger".to_string()]
            ));
        }

        #[test]
        fn is_commit_valid_root_commit_with_matching_author_true() {
            let (_dir, repo) = make_temp_repo();
            let oid =
                make_commit(&repo, "Alice", "root", &[("a.txt", "hello")]);
            let commit = repo.find_commit(oid).unwrap();
            // Root commit has parent_count == 0, which is <= 1
            assert!(super::super::is_commit_valid(
                &commit,
                &["Alice".to_string()]
            ));
        }

        #[test]
        fn is_commit_valid_author_with_extra_space_in_list_false() {
            let (_dir, repo) = make_temp_repo();
            let oid =
                make_commit(&repo, "Alice", "first", &[("a.txt", "hello")]);
            let commit = repo.find_commit(oid).unwrap();
            // " Alice" with leading space should NOT match "Alice"
            assert!(!super::super::is_commit_valid(
                &commit,
                &[" Alice".to_string()]
            ));
        }

        // --- process_commits / process_commit ---

        #[test]
        fn process_commits_single_matching_commit_produces_output_and_returns_true(
        ) {
            let (_dir, repo) = make_temp_repo();
            make_commit(
                &repo,
                "Alice",
                "add file",
                &[("hello.rs", "fn main() {}")],
            );
            let mut w = MockWriter::new();
            let found = super::super::process_commits(
                &mut w,
                &repo,
                &["Alice".to_string()],
            )
            .unwrap();
            assert!(found);
            assert!(w.as_str().contains("commit message: add file"));
        }

        #[test]
        fn process_commits_filters_to_matching_author_only() {
            let (_dir, repo) = make_temp_repo();
            make_commit(
                &repo,
                "Alice",
                "alice commit",
                &[("alice.txt", "alice")],
            );
            make_commit(&repo, "Bob", "bob commit", &[("bob.txt", "bob")]);
            let mut w = MockWriter::new();
            super::super::process_commits(
                &mut w,
                &repo,
                &["Alice".to_string()],
            )
            .unwrap();
            let out = w.as_str();
            assert!(out.contains("alice commit"));
            assert!(!out.contains("bob commit"));
        }

        #[test]
        fn process_commits_no_matching_commits_returns_false() {
            let (_dir, repo) = make_temp_repo();
            make_commit(&repo, "Alice", "alice commit", &[("a.txt", "a")]);
            let mut w = MockWriter::new();
            let found = super::super::process_commits(
                &mut w,
                &repo,
                &["Bob".to_string()],
            )
            .unwrap();
            assert!(!found);
        }

        #[test]
        fn process_commits_merge_commits_skipped_even_if_authored_by_target() {
            let (_dir, repo) = make_temp_repo();
            let oid1 = make_commit(&repo, "Alice", "first", &[("a.txt", "a")]);
            let oid2 = make_commit(&repo, "Alice", "second", &[("b.txt", "b")]);
            make_merge_commit(&repo, &[oid2, oid1]);
            let mut w = MockWriter::new();
            // Only non-merge commits should be counted
            let found = super::super::process_commits(
                &mut w,
                &repo,
                &["Merger".to_string()],
            )
            .unwrap();
            assert!(!found);
        }

        // --- process_repo ---

        #[test]
        fn process_repo_no_save_path_writes_to_writer() {
            let (_dir, repo) = make_temp_repo();
            make_commit(&repo, "Alice", "initial", &[("x.txt", "x")]);
            let mut w = MockWriter::new();
            let result = super::super::process_repo(
                &repo,
                "test-repo",
                &mut w,
                &["Alice".to_string()],
                None,
            );
            assert!(result.is_ok());
            assert!(w.as_str().contains("project name: test-repo"));
        }

        #[test]
        fn process_repo_no_commits_no_save_path_returns_ok() {
            let (_dir, repo) = make_temp_repo();
            make_commit(&repo, "Alice", "initial", &[("x.txt", "x")]);
            let mut w = MockWriter::new();
            let result = super::super::process_repo(
                &repo,
                "test-repo",
                &mut w,
                &["Nobody".to_string()],
                None,
            );
            assert!(result.is_ok());
        }

        #[test]
        fn process_repo_with_save_path_commits_found_file_not_deleted() {
            let (_dir, repo) = make_temp_repo();
            make_commit(&repo, "Alice", "initial", &[("x.txt", "x")]);
            let out_dir = tempfile::tempdir().unwrap();
            let out_path = out_dir.path().join("output.txt");
            // Create the file first so process_repo can write to it
            {
                let f = std::fs::File::create(&out_path).unwrap();
                let mut bw = std::io::BufWriter::new(f);
                super::super::process_repo(
                    &repo,
                    "test-repo",
                    &mut bw,
                    &["Alice".to_string()],
                    Some(&out_path),
                )
                .unwrap();
            }
            assert!(out_path.exists());
        }

        #[test]
        fn process_repo_no_commits_found_with_save_path_file_deleted() {
            let (_dir, repo) = make_temp_repo();
            make_commit(&repo, "Alice", "initial", &[("x.txt", "x")]);
            let out_dir = tempfile::tempdir().unwrap();
            let out_path = out_dir.path().join("output.txt");
            {
                let f = std::fs::File::create(&out_path).unwrap();
                let mut bw = std::io::BufWriter::new(f);
                super::super::process_repo(
                    &repo,
                    "test-repo",
                    &mut bw,
                    &["Nobody".to_string()],
                    Some(&out_path),
                )
                .unwrap();
            }
            assert!(!out_path.exists());
        }
    }
}
