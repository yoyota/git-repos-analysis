# Unit Test Plan: diff.rs → diff.test.rs

## Summary of What diff.rs Does

`diff.rs` is a CLI binary that:
1. Opens a git repository via `git2`
2. Detects the author(s) to filter by (from `--authors` flag or git config `user.name`)
3. Walks all commits across all refs in chronological order
4. For each commit authored by a matching author (non-merge commits only):
   - Computes the diff to its parent (or root if none)
   - Filters out lock files and files with >= 1000 changed lines
   - Also drops the entire diff if total insertions + deletions > 2000
   - For `.ipynb` files: extracts only the `"source"` array content
   - For all other files: emits file headers, hunk headers, blank context lines, and `+`/`-` diff lines, while suppressing lines containing `"image/png"`
5. Writes results to stdout or a file (creating parent dirs as needed); deletes the file if no commits matched

---

## Functions / Units to Test

### 1. `repo_name(repo_path: &Path) -> Result<String>`

Resolves the canonical path and extracts its final component as a `String`.

**Test cases:**
- [ ] Happy path: a valid existing directory returns its directory name as a `String`
- [ ] Happy path: path with trailing slash still returns the correct base name
- [ ] Error case: a non-existent path returns an `Err` (canonicalize fails)
- [ ] Edge case: path whose final component is not valid UTF-8 returns an `Err` via the `.context(...)` branch

---

### 2. `detect_authors(repo: &Repository, override_authors: Option<&str>) -> Vec<String>`

Returns a parsed list of author names.

**Test cases:**
- [ ] Happy path (override provided): `"Alice"` → `vec!["Alice"]`
- [ ] Happy path (override provided, multiple): `"Alice, Bob,Charlie"` → `vec!["Alice", "Bob", "Charlie"]` (whitespace trimmed)
- [ ] Happy path (override provided, single with spaces): `"  Alice  "` → `vec!["Alice"]`
- [ ] Happy path (override is empty string): `""` → `vec![""]`
- [ ] Happy path (override is None): uses git config `user.name`; test against a real temp repo initialised with a known user name
- [ ] Edge case (override is None, no git config): returns an empty `Vec`

---

### 3. `parse_stats_line(line: &str) -> Option<String>`

Parses one line from `git diff --stat` output and decides whether to include the file.

**Test cases:**
- [ ] Happy path: `"src/main.rs          |  42"` → `Some("src/main.rs")`
- [ ] Happy path: change count exactly 999 → `Some(file_name)`
- [ ] Edge case: change count exactly 1000 → `None` (threshold is `< 1000`)
- [ ] Edge case: change count 0 → `Some(file_name)` (0 < 1000)
- [ ] Lock file filtered — `yarn.lock` in root: `"yarn.lock    |   5"` → `None`
- [ ] Lock file filtered — `Cargo.toml` in root: `"Cargo.toml   |  10"` → `None`
- [ ] Lock file filtered — nested path: `"some/nested/poetry.lock  |  3"` → `None`
- [ ] Lock file filtered — `package-lock.json`: `"package-lock.json  | 20"` → `None`
- [ ] Lock file filtered — `.terraform.lock.hcl`: `".terraform.lock.hcl | 1"` → `None`
- [ ] Non-lock file with a path containing lock-like prefix: `"src/lock_utils.rs  | 5"` → `Some("src/lock_utils.rs")` (regex requires exact filename match)
- [ ] Edge case: change count is a non-numeric token (`+--`) → `None` (parse fails)
- [ ] Edge case: line does not match FILE_RE pattern (no `|`) → `None`
- [ ] Edge case: line is an empty string → `None`

---

### 4. `DiffLineProcessor::process_diff_line(&mut self, line: DiffLine, file_path: &str) -> Option<String>`

This is the most complex pure-logic unit. It branches on file extension and origin character.

Because `DiffLine` is a `git2` struct that cannot be constructed directly, tests must use a mock or a thin wrapper. The recommended approach is to extract the pure-logic core into a helper that accepts `(origin: char, content: &[u8], file_path: &str)` and test that. The plan describes the desired behaviour; the implementor chooses how to expose it for testing.

#### 4a. Non-`.ipynb` files

**Test cases (origin character routing):**
- [ ] Happy path, origin `'F'` (file header): content returned verbatim as `Some(text)`
- [ ] Happy path, origin `'H'` (hunk header): content returned verbatim as `Some(text)`
- [ ] Happy path, origin `'B'` (binary): content returned verbatim as `Some(text)`
- [ ] Happy path, origin `' '` (context line): content returned verbatim as `Some(text)`
- [ ] Happy path, origin `'+'` (addition): `Some("+<content>")`
- [ ] Happy path, origin `'-'` (deletion): `Some("-<content>")`
- [ ] Edge case, origin `'\\'` (no newline at EOF marker): `None`
- [ ] Edge case, any unrecognised origin character: `None`
- [ ] Filter: any line whose content contains `"image/png"` (regardless of origin) → `None`
- [ ] Filter: `"image/png"` present inside a `+` line → `None` (suppressed before origin check)
- [ ] Non-filter: content contains `"image/PNG"` (different case) → passes through normally

#### 4b. `.ipynb` files — state machine

The processor carries `in_ipynb_source: bool` state across calls.

**Test cases:**
- [ ] Outside source block, line not matching `"source": [` → `None`
- [ ] Line `"source": [` (trimmed) sets `in_ipynb_source = true` → `None` returned for the trigger line itself
- [ ] Inside source block, regular content line: strips surrounding quotes, removes `\\n`, prefixes with origin, appends real newline → `Some(...)`
- [ ] Inside source block, line starting with `]` (trimmed): sets `in_ipynb_source = false` → `None` returned
- [ ] Transition: multiple source blocks in sequence (out → in → out → in → out) produces output only for lines inside blocks
- [ ] Edge case: content line inside source block that is empty after cleaning → `Some("<origin>\n")`
- [ ] Edge case: content has multiple `"..."` pairs — only outer quotes stripped by `trim_matches('"')`
- [ ] Edge case: file_path ends with `.ipynb` but content contains `"image/png"` inside a source block — the image/png filter does NOT apply to `.ipynb` (code skips the filter for `.ipynb` branch)
- [ ] Edge case: invalid UTF-8 bytes in content → `None` (from_utf8 fails)

---

### 5. `write_header(writer: &mut impl Write, repo_name: &str) -> Result<()>`

**Test cases:**
- [ ] Happy path: writes `"project name: <name>\n\n"` exactly to the writer
- [ ] Edge case: repo_name contains special characters (spaces, slashes) — written verbatim

---

### 6. `filter_out_large_change_files(stats: &DiffStats) -> Result<Vec<String>>`

Because `DiffStats` is a `git2` opaque type, this function is best tested via integration with a real temporary repository. However, the threshold logic can be verified indirectly through `parse_stats_line` unit tests. Integration tests should cover:

- [ ] Happy path: diff with total changes <= 2000 returns a non-empty file list
- [ ] Edge case: total insertions + deletions > 2000 returns an empty `Vec` (entire diff skipped)
- [ ] Edge case: total changes == 2000 (boundary) is NOT skipped (condition is `> 2000`)
- [ ] Edge case: stats contain lock files — those files are absent from the returned list

---

### 7. `is_commit_valid(commit: &Commit, authors: &[String]) -> bool`

**Test cases (integration with a real temp repo):**
- [ ] Happy path: single-parent commit, author in list → `true`
- [ ] Author not in list → `false`
- [ ] Authors list is empty → `false`
- [ ] Merge commit (parent_count == 2) → `false`
- [ ] Root commit (parent_count == 0, which is <= 1) with author in list → `true`
- [ ] Author name matches with leading/trailing space in list → `false` (exact equality)

---

### 8. `process_commits` / `process_commit` (integration)

These are tested through a real temp git repository.

- [ ] Happy path: repo with one commit from the target author produces output and returns `true`
- [ ] Repo with commits from multiple authors, filtered to one — only matching commits appear in output
- [ ] Repo with zero matching commits returns `false`
- [ ] Merge commits are skipped even when authored by the target author

---

### 9. `process_repo` (integration)

- [ ] Happy path (`save_path = None`): output is written to the provided writer, `Ok(())` returned
- [ ] Happy path (`save_path = Some(...)`, commits found): file is NOT deleted, log message emitted
- [ ] No commits found + `save_path = Some(...)`: file is deleted after creation
- [ ] No commits found + `save_path = None`: no deletion attempted, returns `Ok(())`

---

## File Structure for diff.test.rs

```
src/bin/
  diff.rs          (existing)
  diff_tests.rs    (new — add `#[cfg(test)] mod diff_tests;` at bottom of diff.rs,
                    OR make it a sibling integration test under tests/)
```

Because `diff.rs` is a binary crate (`src/bin/`), unit tests inside the same file are the simplest approach. A `#[cfg(test)]` module appended to `diff.rs` that imports the private items is preferred over a separate file. Alternatively, convert the logic into a library (`src/lib.rs`) and test that.

**Recommended module layout inside diff.rs (or diff_tests.rs via `mod`):**

```
#[cfg(test)]
mod tests {
    use super::*;

    mod repo_name { ... }
    mod detect_authors { ... }
    mod parse_stats_line { ... }
    mod diff_line_processor { ... }
    mod write_header { ... }
    mod integration { ... }   // tests requiring a real temp git repo
}
```

---

## Test Helpers and Fixtures Needed

### `make_temp_repo() -> (TempDir, Repository)`
Creates a temporary directory, initialises a git repo, sets `user.name` and `user.email` in its config, and returns both handles. Used by all integration tests.

### `make_commit(repo: &Repository, author: &str, message: &str, files: &[(&str, &str)]) -> Oid`
Adds the given files to the index and creates a commit with the specified author name. Returns the commit OID.

### `make_merge_commit(repo: &Repository, parents: &[Oid]) -> Oid`
Creates a commit with two parents to simulate a merge.

### `MockWriter`
A `Vec<u8>` wrapped in a struct implementing `Write` and exposing `as_str() -> &str`. Used to capture writer output in unit tests without touching the filesystem.

### `FakeDiffLine` (if DiffLineProcessor is refactored)
A plain struct holding `(origin: char, content: Vec<u8>)` so that `process_diff_line` can be tested without constructing a `git2::DiffLine`. Requires extracting the pure logic out of the git2 callback closure.

---

## Out of Scope

- Testing `main()`, `run()`, and `init_tracing()` directly
- CLI argument parsing (`clap` is tested by its own library)
- Output formatting of `chrono` timestamps (assumed correct by the `chrono` crate)
- Behaviour when the git repository is corrupted or has unusual object types
- Performance / throughput testing on large repositories
- Cross-platform path behaviour (Windows path separators)
- The `--save` flag's directory-creation logic beyond the happy path
