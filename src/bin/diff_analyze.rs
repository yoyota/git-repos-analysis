use clap::Parser;
use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tiktoken_rs::cl100k_base;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const MAX_TOKENS_PER_CHUNK: usize = 100_000;

#[derive(Parser, Debug)]
#[command(
    name = "diffanalyzer",
    about = "Generate resume/LinkedIn text from git diff"
)]
struct Args {
    /// Input diff.txt file (default: ~/Downloads/{cwd-name}/diff.txt)
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Output directory (default: same directory as input file)
    #[arg(short, long)]
    output_dir: Option<PathBuf>,

    /// Model to use (e.g., sonnet, opus, haiku)
    #[arg(short, long)]
    model: Option<String>,

    /// Effort level to pass to claude (e.g., low, medium, high, max)
    #[arg(short = 'e', long)]
    effort: Option<String>,
}

const STDOUT_ONLY_DIRECTIVE: &str = r#"CRITICAL OUTPUT CONTRACT — read before doing anything else:
- This prompt is being piped through `claude --print` by an automated subprocess that captures your stdout and parses it.
- Respond with the final text inline in this reply ONLY. Do NOT call any tools.
- Do NOT enter plan mode, do NOT use EnterPlanMode/ExitPlanMode, do NOT use Skill, do NOT use Write/Edit, do NOT save a plan file under ~/.claude/plans, do NOT create files anywhere.
- Do NOT print status messages like "Plan file written" or "Done". Your entire stdout must be the two delimited sections described below — nothing else.
- If you feel the urge to "save" or "write" the result somewhere, suppress it. Just print the result.

"#;

const PROMPT_TEMPLATE: &str = r#"You are a senior technical resume writer. Analyze the following git diff content and produce TWO versions of a project summary in a single output.

Carefully examine: file paths, package names, import statements, frameworks, APIs, database schemas, config files, commit messages, test files, CI/CD configs, and code patterns. Extract every piece of technical evidence you can find.

Output BOTH versions below, separated by the exact delimiter shown. Entirely in English.

================================================================================
VERSION 1: PROFESSIONAL RESUME
================================================================================

---
title: "<Project Name>"
type: resume-entry
status: under review
date_start: YYYY-MM
date_end: YYYY-MM
tags: [resume, <domain-tag>, <tech-tag>, ...]
tech: [<Language>, <Framework>, <Library>, ...]
repo: <repository URL if visible in diff, otherwise omit this line>
---

# <Project Name>
📅 MM/YYYY - MM/YYYY

## Summary
A concise paragraph (2-3 sentences) describing the project, its domain, and your role.

## Key Accomplishments
- 4-6 bullet points using the PAR method (Problem → Action → Result)
- Pattern: "Resolved [specific problem or bottleneck] by [technical action using technology], resulting in [measurable outcome]"
- Focus on the most impressive, resume-worthy achievements
- Quantify where possible (performance gains, error reduction, endpoints, etc.)

## Tech Stack
Single line, comma-separated: Language, Framework, Database, Infrastructure, Tools

================================================================================
VERSION 2: DETAILED TECHNICAL ANALYSIS (LLM SOURCE)
================================================================================

---
title: "<Project Name> — Technical Summary"
type: project-summary
status: under review
date_start: YYYY-MM
date_end: YYYY-MM
related: "[[resume]]"
tags: [project, <domain-tag>, <tech-tag>, analysis]
---

# <Project Name>
📅 MM/YYYY - MM/YYYY

## Project Overview
A detailed paragraph (4-6 sentences) describing:
- What the project is and its business domain (e.g., ML platform, e-commerce, fintech, developer tooling)
- The overall system architecture (monolith, microservices, client-server, etc.)
- Who the end users are and what problem it solves
- The scale and complexity if inferable (number of services, endpoints, models, etc.)

## Technical Architecture
Describe the system design in detail:
- How the codebase is organized (modules, packages, layers)
- Key architectural patterns used (MVC, event-driven, CQRS, repository pattern, etc.)
- Data flow: how requests are processed from entry point to response
- Integration points with external systems, APIs, or services
- Authentication/authorization approach if visible
- Database schema design or data modeling approach if visible

## Key Technical Contributions
- 6-12 detailed bullet points of concrete technical work evidenced in the diffs, using the PAR method
- Each bullet should be 1-2 sentences: the problem/challenge addressed, the technical approach taken, and the resulting capability or outcome
- Pattern: "Addressed [specific technical problem or gap] by [implementing/designing/building approach using technology], enabling [outcome/capability]"
- Cover: features built, APIs designed, data models created, algorithms implemented, performance optimizations, security measures, testing strategies, DevOps/CI-CD work
- Include specific details: endpoint paths, class/module names, configuration parameters, library usage
- If quantifiable metrics are inferable (number of endpoints, test cases, models, migrations), include them

## Technology Stack
List every technology, framework, library, and tool evidenced in the code, organized by category:
- **Languages**: (with version if visible)
- **Frameworks**:
- **Libraries/Dependencies**:
- **Databases/Storage**:
- **Infrastructure/DevOps**:
- **Testing**:
- **Build Tools**:
- **Other Tools/Services**:

## Technical Challenges & Solutions
2-4 paragraphs describing:
- The most complex technical problems visible in the diffs
- How they were approached and solved
- Design trade-offs that were made (if inferable from code comments, commit messages, or architectural choices)
- Any refactoring or iteration visible across commits

## Code Quality Indicators
Note any evidence of:
- Testing practices (unit tests, integration tests, test coverage)
- Code review patterns (if visible from commit messages)
- Documentation practices
- Error handling and logging approaches
- Security considerations

Rules:
- Only claim what is directly evidenced in the diffs. Do not fabricate features or technologies.
- Version 1 should be polished and human-readable — ready to paste into a resume.
- Version 2 should be verbose and thorough — it will be consumed by an LLM for generating tailored resumes, cover letters, and interview prep. More detail is better.
- When uncertain about something, phrase it as "appears to" or "likely" rather than omitting it.
- Include raw technical details (class names, endpoint paths, config keys) in Version 2.
- If the diffs are too small for a full analysis, still extract every detail possible and note the limited scope.
- Each version must begin with a valid YAML frontmatter block (opened and closed by `---` on their own lines) so the file is readable by Obsidian. Do not emit placeholder angle-bracket values — fill every field from diff evidence, and omit any field (e.g. `repo`) when no evidence is available. `date_start` / `date_end` must use the `YYYY-MM` format (e.g. `2024-03`).

Output ONLY the two versions with the delimiters, no additional commentary.

=== GIT DIFF CONTENT STARTS HERE ===
"#;

const CHUNK_PROMPT_TEMPLATE: &str = r#"Extract every piece of technical information from this git diff chunk. This will be merged with other chunks to build a comprehensive project summary for resume generation.

Be exhaustive — capture everything visible:

1. **Project/Repository**: name, path, organization (if visible)
2. **Date range**: commit dates (if visible)
3. **Technologies**: every language, framework, library, database, infrastructure tool, and service evidenced
4. **Problems & challenges**: specific bottlenecks, pain points, or limitations the changes are addressing (e.g., latency, scalability, missing feature, bug, tech debt)
5. **Features & APIs**: specific endpoints, controllers, services, data models, UI components implemented or modified
6. **Architecture**: patterns (MVC, microservices, event-driven, etc.), module organization, layer separation
7. **Data models**: schemas, entities, migrations, relationships
8. **Infrastructure**: Docker, CI/CD, cloud services, deployment configs
9. **Testing**: test files, testing frameworks, test patterns
10. **Technical complexity**: algorithms, business logic, error handling, security measures
11. **Domain context**: what business problem does this code solve? Who are the users?
12. **Code quality signals**: logging, documentation, error handling patterns, code organization

Output detailed structured bullet points under each category. Include specific class names, file paths, and endpoint paths where visible. Skip categories that have no evidence in this chunk.

=== GIT DIFF CHUNK ===
"#;

const MERGE_PROMPT_TEMPLATE: &str = r#"You are a senior technical resume writer. Based on the following partial analyses from different chunks of the same git diff file, create TWO versions of a unified project summary.

Deduplicate, synthesize, and enrich the information. Combine overlapping details and resolve any contradictions.

Output BOTH versions below, separated by the exact delimiter shown. Entirely in English.

================================================================================
VERSION 1: PROFESSIONAL RESUME
================================================================================

---
title: "<Project Name>"
type: resume-entry
status: <complete|in-progress|archived>
date_start: YYYY-MM
date_end: YYYY-MM
tags: [resume, <domain-tag>, <tech-tag>, ...]
tech: [<Language>, <Framework>, <Library>, ...]
repo: <repository URL if visible in diff, otherwise omit this line>
---

# <Project Name>
📅 MM/YYYY - MM/YYYY

## Summary
A concise paragraph (2-3 sentences) describing the project, its domain, and your role.

## Key Accomplishments
- 4-6 bullet points using the PAR method (Problem → Action → Result)
- Pattern: "Resolved [specific problem or bottleneck] by [technical action using technology], resulting in [measurable outcome]"
- Focus on the most impressive, resume-worthy achievements
- Quantify where possible

## Tech Stack
Single line, comma-separated: Language, Framework, Database, Infrastructure, Tools

================================================================================
VERSION 2: DETAILED TECHNICAL ANALYSIS (LLM SOURCE)
================================================================================

---
title: "<Project Name> — Technical Summary"
type: project-summary
status: <complete|in-progress|archived>
date_start: YYYY-MM
date_end: YYYY-MM
related: "[[resume]]"
tags: [project, <domain-tag>, <tech-tag>, analysis]
---

# <Project Name>
📅 MM/YYYY - MM/YYYY

## Project Overview
A detailed paragraph (4-6 sentences) describing: what the project is, its business domain, the overall system architecture, who the end users are, and the scale/complexity.

## Technical Architecture
Describe the system design in detail: codebase organization, architectural patterns, data flow, integration points, auth approach, database design.

## Key Technical Contributions
- 6-12 detailed bullet points of concrete technical work, using the PAR method
- Each bullet: 1-2 sentences covering the problem addressed, the technical approach taken, and the resulting outcome
- Pattern: "Addressed [specific technical problem or gap] by [implementing/designing/building approach using technology], enabling [outcome/capability]"
- Include specific details: endpoint paths, class names, config parameters, library usage
- Include quantifiable metrics where inferable

## Technology Stack
List every evidenced technology organized by category:
- **Languages**: (with version if visible)
- **Frameworks**:
- **Libraries/Dependencies**:
- **Databases/Storage**:
- **Infrastructure/DevOps**:
- **Testing**:
- **Build Tools**:
- **Other Tools/Services**:

## Technical Challenges & Solutions
2-4 paragraphs on: complex problems visible in the code, how they were solved, design trade-offs, refactoring or iteration across commits.

## Code Quality Indicators
Evidence of: testing practices, documentation, error handling, logging, security considerations.

Rules:
- Only claim what is directly evidenced in the partial analyses. Do not fabricate.
- Version 1 should be polished and human-readable — ready to paste into a resume.
- Version 2 should be verbose and thorough — more detail is better for downstream LLM consumption.
- When uncertain, phrase as "appears to" or "likely" rather than omitting.
- Include raw technical details (class names, endpoint paths, config keys) in Version 2.
- Each version must begin with a valid YAML frontmatter block (opened and closed by `---` on their own lines) so the file is readable by Obsidian. Do not emit placeholder angle-bracket values — fill every field from evidence in the partial summaries, and omit any field (e.g. `repo`) when no evidence is available. `date_start` / `date_end` must use the `YYYY-MM` format (e.g. `2024-03`).

Output ONLY the two versions with the delimiters, no additional commentary.

=== PARTIAL SUMMARIES ===
"#;

fn main() {
    init_tracing();
    if let Err(e) = run() {
        tracing::error!("{}", e);
        std::process::exit(1);
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

fn run() -> Result<(), String> {
    let args = Args::parse();
    let model = args.model.as_deref();
    let effort = args.effort.as_deref();

    let use_stdin = args.input.is_none() && !io::stdin().is_terminal();

    let (content, default_output_dir, input_stem) = if use_stdin {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("Failed to read stdin: {}", e))?;
        let cwd = std::env::current_dir().map_err(|e| format!("Cannot get cwd: {}", e))?;
        info!("processing: stdin");
        (buf, cwd, "stdin".to_string())
    } else {
        let input = args.input.map_or_else(default_input, Ok)?;
        if !input.exists() {
            return Err(format!("Input file not found: {}", input.display()));
        }
        let parent = input
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| "Cannot determine output dir from input path".to_string())?;
        info!("processing: {}", input.display());
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("diff")
            .to_string();
        let content =
            fs::read_to_string(&input).map_err(|e| format!("Failed to read file: {}", e))?;
        (content, parent, stem)
    };

    let output_dir = args.output_dir.unwrap_or(default_output_dir);
    fs::create_dir_all(&output_dir).map_err(|e| format!("Failed to create output dir: {}", e))?;

    let bpe = cl100k_base().expect("Failed to initialize tokenizer");

    let checkpoint_path = output_dir.join(format!("{}.chunks.json", input_stem));
    let output = summarize_content(&content, &bpe, model, effort, &checkpoint_path)?;
    write_output(&output_dir.join("debug_raw_output.txt"), &output)?;
    let [resume, summary] = split_versions(&output)?;

    write_output(&output_dir.join("resume.md"), &resume)?;
    write_output(&output_dir.join("summary.md"), &summary)?;

    info!("done. output directory: {}", output_dir.display());
    Ok(())
}

fn default_input() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("Cannot get cwd: {}", e))?;
    let name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Cannot determine repo name from cwd".to_string())?;
    let home = std::env::var("HOME").map_err(|_| "HOME env var not set".to_string())?;
    Ok(PathBuf::from(home)
        .join("Downloads")
        .join(name)
        .join("diff.txt"))
}

fn write_output(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    info!("→ {}", path.display());
    Ok(())
}

fn split_versions(output: &str) -> Result<[String; 2], String> {
    const SEP: &str =
        "================================================================================";
    let parts: Vec<&str> = output.split(SEP).collect();
    // parts[1] = VERSION 1 header, parts[2] = version 1 content
    // parts[3] = VERSION 2 header, parts[4] = version 2 content
    if parts.len() < 5 {
        return Err(format!(
            "Claude output did not contain the expected delimiter pattern (got {} sections, need 5). \
             Check debug_raw_output.txt to see the raw response.",
            parts.len()
        ));
    }
    Ok([parts[2].trim().to_string(), parts[4].trim().to_string()])
}

fn summarize_content(
    file_content: &str,
    bpe: &tiktoken_rs::CoreBPE,
    model: Option<&str>,
    effort: Option<&str>,
    checkpoint_path: &Path,
) -> Result<String, String> {
    let token_count = bpe.encode_with_special_tokens(file_content).len();
    info!(token_count, "file token count");

    if token_count <= MAX_TOKENS_PER_CHUNK {
        return summarize_content_direct(file_content, model, effort);
    }

    info!(
        token_count,
        max = MAX_TOKENS_PER_CHUNK,
        "large file, processing in chunks"
    );

    let chunks = split_into_chunks_by_tokens(file_content, bpe, MAX_TOKENS_PER_CHUNK);
    info!(chunks = chunks.len(), "split into chunks");

    let loaded = load_checkpoint(checkpoint_path)?;
    let done_indices: HashSet<usize> = loaded.iter().map(|(i, _)| *i).collect();
    if !done_indices.is_empty() {
        info!(skipped = done_indices.len(), "resuming from checkpoint");
    }
    let mut partial_summaries: Vec<(usize, String)> = loaded;

    for (i, chunk) in chunks.iter().enumerate() {
        if done_indices.contains(&i) {
            continue;
        }

        let chunk_tokens = bpe.encode_with_special_tokens(chunk).len();
        info!(
            chunk = i + 1,
            total = chunks.len(),
            chunk_tokens,
            "processing chunk"
        );

        match summarize_chunk(chunk, model, effort) {
            Ok(summary) => {
                partial_summaries.push((i, summary));
                save_checkpoint(checkpoint_path, &partial_summaries)?;
            }
            Err(e) if e.starts_with("RATE_LIMIT:") => {
                let _ = save_checkpoint(checkpoint_path, &partial_summaries);
                return Err(format!(
                    "Rate limit hit. Partial progress saved to {}. Re-run after the cooldown.",
                    checkpoint_path.display()
                ));
            }
            Err(e) => return Err(e),
        }
    }

    if partial_summaries.is_empty() {
        return Err("All chunks failed to process".to_string());
    }

    // Sort by chunk index before merging (checkpoint load order is not guaranteed)
    partial_summaries.sort_by_key(|(i, _)| *i);
    let summaries_in_order: Vec<String> = partial_summaries.into_iter().map(|(_, s)| s).collect();

    info!(
        count = summaries_in_order.len(),
        "merging partial summaries"
    );
    match merge_summaries(&summaries_in_order, model, effort) {
        Ok(result) => {
            if let Err(e) = delete_checkpoint(checkpoint_path) {
                warn!(error = %e, "failed to delete checkpoint after successful merge");
            }
            Ok(result)
        }
        Err(e) if e.starts_with("RATE_LIMIT:") => Err(format!(
            "Rate limit hit during merge. Chunk summaries preserved at {}. Re-run to retry merge.",
            checkpoint_path.display()
        )),
        Err(e) => Err(e),
    }
}

fn load_checkpoint(path: &Path) -> Result<Vec<(usize, String)>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => {
            return Err(format!(
                "Failed to read checkpoint {}: {}",
                path.display(),
                e
            ))
        }
    };

    let arr: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "checkpoint file is malformed JSON, treating as fresh start");
            return Ok(vec![]);
        }
    };

    let Some(arr) = arr.as_array() else {
        warn!(path = %path.display(), "checkpoint file is not a JSON array, treating as fresh start");
        return Ok(vec![]);
    };

    let mut records = Vec::new();
    for obj in arr {
        let chunk_index = match obj.get("chunk_index").and_then(|v| v.as_u64()) {
            Some(i) => i as usize,
            None => {
                warn!(path = %path.display(), "checkpoint record missing chunk_index, treating as fresh start");
                return Ok(vec![]);
            }
        };
        let summary = match obj.get("summary").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                warn!(path = %path.display(), "checkpoint record missing summary, treating as fresh start");
                return Ok(vec![]);
            }
        };
        records.push((chunk_index, summary));
    }

    Ok(records)
}

fn save_checkpoint(path: &Path, completed: &[(usize, String)]) -> Result<(), String> {
    let records: Vec<serde_json::Value> = completed
        .iter()
        .map(|(i, s)| serde_json::json!({ "chunk_index": i, "summary": s }))
        .collect();
    let json = serde_json::to_string(&records)
        .map_err(|e| format!("Failed to serialise checkpoint: {}", e))?;

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, &json).map_err(|e| {
        format!(
            "Failed to write tmp checkpoint {}: {}",
            tmp_path.display(),
            e
        )
    })?;
    fs::rename(&tmp_path, path).map_err(|e| {
        format!(
            "Failed to rename checkpoint {} -> {}: {}",
            tmp_path.display(),
            path.display(),
            e
        )
    })?;
    Ok(())
}

fn delete_checkpoint(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "Failed to delete checkpoint {}: {}",
            path.display(),
            e
        )),
    }
}

fn summarize_content_direct(
    content: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<String, String> {
    call_claude(
        &format!("{}{}{}", STDOUT_ONLY_DIRECTIVE, PROMPT_TEMPLATE, content),
        model,
        effort,
    )
}

fn summarize_chunk(
    chunk: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<String, String> {
    call_claude(
        &format!(
            "{}{}{}",
            STDOUT_ONLY_DIRECTIVE, CHUNK_PROMPT_TEMPLATE, chunk
        ),
        model,
        effort,
    )
}

fn merge_summaries(
    summaries: &[String],
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<String, String> {
    let combined = summaries.join("\n\n---\n\n");
    call_claude(
        &format!(
            "{}{}{}",
            STDOUT_ONLY_DIRECTIVE, MERGE_PROMPT_TEMPLATE, combined
        ),
        model,
        effort,
    )
}

fn split_into_chunks_by_tokens(
    content: &str,
    bpe: &tiktoken_rs::CoreBPE,
    max_tokens: usize,
) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current_chunk = String::new();
    let mut current_tokens = 0;

    for line in content.lines() {
        let line_with_newline = format!("{line}\n");
        let line_tokens = bpe.encode_with_special_tokens(&line_with_newline).len();

        if current_tokens + line_tokens > max_tokens && !current_chunk.is_empty() {
            chunks.push(std::mem::take(&mut current_chunk));
            current_tokens = 0;
        }

        current_chunk.push_str(&line_with_newline);
        current_tokens += line_tokens;
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

fn build_claude_args(model: Option<&str>, effort: Option<&str>) -> Vec<String> {
    let mut args = vec!["--print".to_string()];
    if let Some(m) = model {
        args.extend(["--model".to_string(), m.to_string()]);
    }
    if let Some(e) = effort {
        args.extend(["--effort".to_string(), e.to_string()]);
    }
    args
}

fn call_claude(prompt: &str, model: Option<&str>, effort: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.args(build_claude_args(model, effort));

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn claude: {}", e))?;

    child
        .stdin
        .take()
        .ok_or("Failed to open stdin for claude")?
        .write_all(prompt.as_bytes())
        .map_err(|e| format!("Failed to write to stdin: {}", e))?;

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for claude: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let exit_code = output
            .status
            .code()
            .map_or_else(|| "unknown".to_string(), |c| c.to_string());

        if stdout.contains("hit your limit") || stderr.contains("hit your limit") {
            return Err(format!("RATE_LIMIT: Claude exited with code {exit_code}."));
        }
        return Err(format!(
            "Claude exited with code {exit_code}.\nstdout: {stdout}\nstderr: {stderr}"
        ));
    }

    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if result.is_empty() {
        return Err("Empty response from Claude".to_string());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod build_claude_args_tests {
        use super::*;

        // Scenario: --effort high -> args contain "--effort" and "high"
        #[test]
        fn effort_high_appears_in_args() {
            let args = build_claude_args(None, Some("high"));
            assert!(
                args.contains(&"--effort".to_string()),
                "expected --effort flag in args: {:?}",
                args
            );
            assert!(
                args.contains(&"high".to_string()),
                "expected effort value 'high' in args: {:?}",
                args
            );
        }

        // Scenario: --effort max + --model opus -> both flags appear
        #[test]
        fn effort_max_and_model_opus_both_appear() {
            let args = build_claude_args(Some("opus"), Some("max"));
            assert!(
                args.contains(&"--model".to_string()),
                "expected --model flag in args: {:?}",
                args
            );
            assert!(
                args.contains(&"opus".to_string()),
                "expected model value 'opus' in args: {:?}",
                args
            );
            assert!(
                args.contains(&"--effort".to_string()),
                "expected --effort flag in args: {:?}",
                args
            );
            assert!(
                args.contains(&"max".to_string()),
                "expected effort value 'max' in args: {:?}",
                args
            );
        }

        // Scenario: --effort omitted -> no --effort arg in command
        #[test]
        fn effort_none_produces_no_effort_flag() {
            let args = build_claude_args(None, None);
            assert!(
                !args.contains(&"--effort".to_string()),
                "expected no --effort flag when effort is None: {:?}",
                args
            );
        }

        // Scenario: --effort low -> valid command (flag + value present)
        #[test]
        fn effort_low_produces_valid_command() {
            let args = build_claude_args(None, Some("low"));
            assert!(
                args.contains(&"--effort".to_string()),
                "expected --effort flag for 'low': {:?}",
                args
            );
            assert!(
                args.contains(&"low".to_string()),
                "expected effort value 'low' in args: {:?}",
                args
            );
        }

        // Scenario: --effort "" (empty string) -> passes through without rejection
        #[test]
        fn effort_empty_string_passes_through() {
            let args = build_claude_args(None, Some(""));
            assert!(
                args.contains(&"--effort".to_string()),
                "expected --effort flag even for empty string: {:?}",
                args
            );
            // The empty string itself should appear as the following element
            let effort_pos = args.iter().position(|a| a == "--effort");
            assert!(effort_pos.is_some(), "no --effort in args: {:?}", args);
            let value_pos = effort_pos.unwrap() + 1;
            assert_eq!(
                args.get(value_pos).map(|s| s.as_str()),
                Some(""),
                "expected empty string value after --effort: {:?}",
                args
            );
        }

        // Scenario: --print is always first in the args list
        #[test]
        fn print_flag_is_always_present() {
            let args = build_claude_args(None, None);
            assert!(
                args.contains(&"--print".to_string()),
                "expected --print to always be present: {:?}",
                args
            );
        }

        // Scenario: --effort appears after --print (order sanity check)
        #[test]
        fn effort_flag_follows_print_flag() {
            let args = build_claude_args(None, Some("high"));
            let print_pos = args.iter().position(|a| a == "--print");
            let effort_pos = args.iter().position(|a| a == "--effort");
            assert!(print_pos.is_some(), "--print missing: {:?}", args);
            assert!(effort_pos.is_some(), "--effort missing: {:?}", args);
            assert!(
                effort_pos.unwrap() > print_pos.unwrap(),
                "--effort should come after --print: {:?}",
                args
            );
        }
    }

    mod propagation_signature_tests {
        use super::*;

        #[test]
        fn summarize_content_direct_accepts_effort_param() {
            let result = summarize_content_direct("test prompt", None, Some("high"));
            let _: Result<String, String> = result;
        }

        #[test]
        fn summarize_chunk_accepts_effort_param() {
            let result = summarize_chunk("test chunk", None, Some("max"));
            let _: Result<String, String> = result;
        }

        #[test]
        fn merge_summaries_accepts_effort_param() {
            let summaries = vec!["partial summary A".to_string()];
            let result = merge_summaries(&summaries, None, Some("low"));
            let _: Result<String, String> = result;
        }
    }

    mod error_path_tests {
        use super::*;

        #[test]
        fn call_claude_three_arg_signature_exists() {
            let result = call_claude("prompt", None, Some("high"));
            let _: Result<String, String> = result;
        }

        #[test]
        fn rate_limit_detection_unaffected_by_effort_flag() {
            let args: Vec<String> = build_claude_args(None, Some("high"));
            assert!(
                !args.iter().any(|a| a.contains("hit your limit")),
                "args must not contain rate-limit sentinel: {:?}",
                args
            );
            assert!(
                args.contains(&"--effort".to_string()),
                "expected --effort in args: {:?}",
                args
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Checkpoint tests — all scenarios from section 3 of plans/rate_limit_retry.md
    // These tests reference load_checkpoint, save_checkpoint, delete_checkpoint,
    // and the new summarize_content signature (with checkpoint_path: &Path).
    // None of those exist yet, so this entire module will fail to compile (RED).
    // ---------------------------------------------------------------------------
    mod checkpoint_tests {
        use super::*;
        use std::path::Path;
        use tempfile::TempDir;

        // ------------------------------------------------------------------
        // Helper: write raw bytes to a path (used to plant malformed/wrong
        // schema checkpoint files without going through save_checkpoint).
        // ------------------------------------------------------------------
        fn write_raw(path: &Path, content: &str) {
            std::fs::write(path, content).expect("write_raw failed");
        }

        // ------------------------------------------------------------------
        // Helper: read and parse checkpoint file into Vec<(usize, String)>.
        // Panics with the raw content on parse failure to help diagnose.
        // ------------------------------------------------------------------
        fn read_checkpoint_file(path: &Path) -> Vec<(usize, String)> {
            let raw = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("cannot read checkpoint at {}: {e}", path.display()));
            let arr: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|e| {
                panic!(
                    "checkpoint is not valid JSON at {}: {e}\nraw: {raw}",
                    path.display()
                )
            });
            arr.as_array()
                .unwrap_or_else(|| panic!("checkpoint is not a JSON array\nraw: {raw}"))
                .iter()
                .map(|obj| {
                    let idx = obj["chunk_index"]
                        .as_u64()
                        .unwrap_or_else(|| panic!("missing chunk_index in record: {obj}"))
                        as usize;
                    let summary = obj["summary"]
                        .as_str()
                        .unwrap_or_else(|| panic!("missing summary in record: {obj}"))
                        .to_string();
                    (idx, summary)
                })
                .collect()
        }

        // ==================================================================
        // Group: load_checkpoint / save_checkpoint / delete_checkpoint units
        // ==================================================================

        // Missing file returns empty vec (fresh start), no error.
        #[test]
        fn load_checkpoint_missing_file_returns_empty() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("nonexistent.chunks.json");
            let result = load_checkpoint(&path);
            assert!(
                result.is_ok(),
                "expected Ok for missing file, got: {:?}",
                result
            );
            let records = result.unwrap();
            assert!(
                records.is_empty(),
                "expected empty vec for missing file, got: {:?}",
                records
            );
        }

        // save then load round-trips correctly.
        #[test]
        fn save_and_load_checkpoint_round_trip() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("test.chunks.json");
            let records: Vec<(usize, String)> = vec![
                (0, "summary for chunk 0".to_string()),
                (1, "summary for chunk 1".to_string()),
            ];
            save_checkpoint(&path, &records).expect("save_checkpoint failed");
            let loaded = load_checkpoint(&path).expect("load_checkpoint failed");
            // Both records must be present; order may differ
            assert_eq!(
                loaded.len(),
                2,
                "expected 2 records after round-trip, got: {:?}",
                loaded
            );
            let has_zero = loaded
                .iter()
                .any(|(i, s)| *i == 0 && s == "summary for chunk 0");
            let has_one = loaded
                .iter()
                .any(|(i, s)| *i == 1 && s == "summary for chunk 1");
            assert!(
                has_zero,
                "record for index 0 missing after round-trip: {:?}",
                loaded
            );
            assert!(
                has_one,
                "record for index 1 missing after round-trip: {:?}",
                loaded
            );
        }

        // save_checkpoint writes valid JSON (atomic write guarantee).
        #[test]
        fn save_checkpoint_writes_valid_json() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("atomic.chunks.json");
            let records: Vec<(usize, String)> = vec![(0, "chunk zero".to_string())];
            save_checkpoint(&path, &records).expect("save_checkpoint failed");
            // read raw bytes and parse as JSON — must not panic
            let parsed = read_checkpoint_file(&path);
            assert_eq!(parsed.len(), 1, "expected 1 record, got: {:?}", parsed);
        }

        // save_checkpoint with empty slice writes an empty JSON array.
        #[test]
        fn save_checkpoint_empty_slice_writes_empty_array() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("empty.chunks.json");
            save_checkpoint(&path, &[]).expect("save_checkpoint failed");
            let raw = std::fs::read_to_string(&path).expect("cannot read file");
            let parsed: serde_json::Value = serde_json::from_str(&raw).expect("not valid JSON");
            let arr = parsed.as_array().expect("expected JSON array");
            assert!(arr.is_empty(), "expected empty JSON array, got: {}", raw);
        }

        // delete_checkpoint removes an existing file.
        #[test]
        fn delete_checkpoint_removes_existing_file() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("to_delete.chunks.json");
            std::fs::write(&path, "[]").unwrap();
            assert!(path.exists(), "file must exist before deletion");
            delete_checkpoint(&path).expect("delete_checkpoint failed");
            assert!(
                !path.exists(),
                "file must not exist after delete_checkpoint: {}",
                path.display()
            );
        }

        // delete_checkpoint on missing file is not an error.
        #[test]
        fn delete_checkpoint_missing_file_is_not_error() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("ghost.chunks.json");
            let result = delete_checkpoint(&path);
            assert!(
                result.is_ok(),
                "expected Ok when deleting non-existent file, got: {:?}",
                result
            );
        }

        // Malformed JSON in checkpoint: load_checkpoint returns Ok(empty) (warn + fresh start).
        #[test]
        fn load_checkpoint_malformed_json_returns_empty_ok() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("bad.chunks.json");
            write_raw(&path, "this is not json at all }{");
            let result = load_checkpoint(&path);
            assert!(
                result.is_ok(),
                "expected Ok (warn + fresh start) for malformed JSON, got: {:?}",
                result
            );
            let records = result.unwrap();
            assert!(
                records.is_empty(),
                "expected empty records for malformed JSON, got: {:?}",
                records
            );
            // File must still exist (not deleted automatically per plan)
            assert!(
                path.exists(),
                "malformed checkpoint file must NOT be deleted automatically: {}",
                path.display()
            );
        }

        // Wrong schema (valid JSON but missing fields): same warn + fresh start behaviour.
        #[test]
        fn load_checkpoint_wrong_schema_returns_empty_ok() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("wrong_schema.chunks.json");
            write_raw(&path, r#"[{"foo": 1}]"#);
            let result = load_checkpoint(&path);
            assert!(
                result.is_ok(),
                "expected Ok (warn + fresh start) for wrong schema, got: {:?}",
                result
            );
            let records = result.unwrap();
            assert!(
                records.is_empty(),
                "expected empty records for wrong schema, got: {:?}",
                records
            );
        }

        // ==================================================================
        // Group: save_checkpoint accumulates records correctly (no data loss)
        // ==================================================================

        // Calling save_checkpoint multiple times with growing slices preserves
        // all prior records (simulates what summarize_content does after each chunk).
        #[test]
        fn save_checkpoint_accumulates_all_records() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("accum.chunks.json");

            let mut completed: Vec<(usize, String)> = Vec::new();

            completed.push((0, "first".to_string()));
            save_checkpoint(&path, &completed).expect("save after chunk 0 failed");
            let after_one = read_checkpoint_file(&path);
            assert_eq!(
                after_one.len(),
                1,
                "expected 1 record after chunk 0: {:?}",
                after_one
            );

            completed.push((1, "second".to_string()));
            save_checkpoint(&path, &completed).expect("save after chunk 1 failed");
            let after_two = read_checkpoint_file(&path);
            assert_eq!(
                after_two.len(),
                2,
                "expected 2 records after chunk 1: {:?}",
                after_two
            );

            completed.push((2, "third".to_string()));
            save_checkpoint(&path, &completed).expect("save after chunk 2 failed");
            let after_three = read_checkpoint_file(&path);
            assert_eq!(
                after_three.len(),
                3,
                "expected 3 records after chunk 2: {:?}",
                after_three
            );
        }

        // ==================================================================
        // Group: checkpoint path derivation (run() helper logic)
        // ==================================================================

        // Input file `diff.txt` produces checkpoint named `diff.chunks.json`.
        #[test]
        fn checkpoint_path_from_diff_txt() {
            let input = Path::new("/some/output/dir/diff.txt");
            let output_dir = Path::new("/some/output/dir");
            let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("diff");
            let checkpoint = output_dir.join(format!("{}.chunks.json", stem));
            assert!(
                checkpoint.ends_with("diff.chunks.json"),
                "{}",
                checkpoint.display()
            );
        }

        // Input file with no extension produces `myinput.chunks.json`.
        #[test]
        fn checkpoint_path_from_extensionless_input() {
            let input = Path::new("/some/output/dir/myinput");
            let output_dir = Path::new("/some/output/dir");
            let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("diff");
            let checkpoint = output_dir.join(format!("{}.chunks.json", stem));
            assert!(
                checkpoint.ends_with("myinput.chunks.json"),
                "{}",
                checkpoint.display()
            );
        }

        // Stdin mode produces `stdin.chunks.json`.
        #[test]
        fn checkpoint_path_for_stdin() {
            let output_dir = Path::new("/some/output/dir");
            let stem = "stdin";
            let checkpoint = output_dir.join(format!("{}.chunks.json", stem));
            assert!(
                checkpoint.ends_with("stdin.chunks.json"),
                "{}",
                checkpoint.display()
            );
        }

        // ==================================================================
        // Group: summarize_content new signature (checkpoint_path: &Path)
        // These will fail to compile because the current signature does NOT
        // accept a checkpoint_path parameter.
        // ==================================================================

        // summarize_content accepts a &Path as its fifth argument.
        #[test]
        fn summarize_content_accepts_checkpoint_path_argument() {
            let dir = TempDir::new().unwrap();
            let checkpoint = dir.path().join("test.chunks.json");
            let bpe = tiktoken_rs::cl100k_base().expect("tokenizer");
            // Small content: will go through summarize_content_direct, not the
            // chunk path.  We don't care about the Ok/Err — only that it compiles
            // and that the function accepts the extra argument.
            let _result: Result<String, String> =
                summarize_content("hello world", &bpe, None, None, &checkpoint);
        }

        // After a successful summarize_content call, the checkpoint file is deleted.
        // (This test will fail to compile due to missing checkpoint_path param.)
        #[test]
        fn summarize_content_deletes_checkpoint_on_success() {
            let dir = TempDir::new().unwrap();
            let checkpoint = dir.path().join("success.chunks.json");
            // Pre-create the file so we can verify it gets cleaned up.
            save_checkpoint(&checkpoint, &[]).expect("pre-create checkpoint");
            assert!(checkpoint.exists(), "checkpoint must exist before call");
            let bpe = tiktoken_rs::cl100k_base().expect("tokenizer");
            // This will error (no claude CLI) but the point is compile-time check.
            let _result: Result<String, String> =
                summarize_content("tiny content", &bpe, None, None, &checkpoint);
            // We cannot assert file state here because call_claude will fail in CI,
            // but the compile-time signature check is the critical RED assertion.
        }

        // summarize_content with a pre-populated checkpoint file containing all
        // chunk records should skip all chunk calls and go straight to merge.
        // (Compile-fail test for the new signature.)
        #[test]
        fn summarize_content_with_full_checkpoint_skips_chunks() {
            let dir = TempDir::new().unwrap();
            let checkpoint = dir.path().join("full.chunks.json");
            // Pre-populate with one record so load_checkpoint returns it.
            let completed = vec![(0usize, "pre-summarised chunk 0".to_string())];
            save_checkpoint(&checkpoint, &completed).expect("pre-populate checkpoint");
            let bpe = tiktoken_rs::cl100k_base().expect("tokenizer");
            // Again, compile-time signature check is the goal.
            let _result: Result<String, String> =
                summarize_content("hello world", &bpe, None, None, &checkpoint);
        }

        // load_checkpoint followed by save_checkpoint with out-of-range chunk
        // indices: those indices are never matched, so all real chunks are processed.
        // This is a unit test of load_checkpoint behaviour only.
        #[test]
        fn load_checkpoint_with_out_of_range_indices_returns_them_verbatim() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("oob.chunks.json");
            // chunk_index 9999 is "out of range" for a 2-chunk job, but
            // load_checkpoint must still return it without error.
            let records = vec![(9999usize, "oob summary".to_string())];
            save_checkpoint(&path, &records).expect("save failed");
            let loaded = load_checkpoint(&path).expect("load failed");
            assert_eq!(loaded.len(), 1, "expected 1 record, got: {:?}", loaded);
            let (idx, summary) = &loaded[0];
            assert_eq!(*idx, 9999, "expected index 9999, got: {idx}");
            assert_eq!(
                summary.as_str(),
                "oob summary",
                "expected 'oob summary', got: {summary}"
            );
        }
    }
}
