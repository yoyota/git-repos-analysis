use clap::Parser;
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

const PROMPT_TEMPLATE: &str = r#"You are a senior technical resume writer. Analyze the following git diff content and produce TWO versions of a project summary in a single output.

Carefully examine: file paths, package names, import statements, frameworks, APIs, database schemas, config files, commit messages, test files, CI/CD configs, and code patterns. Extract every piece of technical evidence you can find.

Output BOTH versions below, separated by the exact delimiter shown. Entirely in English.

================================================================================
VERSION 1: PROFESSIONAL RESUME
================================================================================

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

    let (content, default_output_dir) = if use_stdin {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("Failed to read stdin: {}", e))?;
        let cwd = std::env::current_dir().map_err(|e| format!("Cannot get cwd: {}", e))?;
        info!("processing: stdin");
        (buf, cwd)
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
        let content =
            fs::read_to_string(&input).map_err(|e| format!("Failed to read file: {}", e))?;
        (content, parent)
    };

    let output_dir = args.output_dir.unwrap_or(default_output_dir);
    fs::create_dir_all(&output_dir).map_err(|e| format!("Failed to create output dir: {}", e))?;

    let bpe = cl100k_base().expect("Failed to initialize tokenizer");

    let output = summarize_content(&content, &bpe, model, effort)?;
    write_output(&output_dir.join("debug_raw_output.txt"), &output)?;
    let [resume, summary] = split_versions(&output);

    write_output(&output_dir.join("resume.txt"), &resume)?;
    write_output(&output_dir.join("summary.txt"), &summary)?;

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

fn split_versions(output: &str) -> [String; 2] {
    const SEP: &str =
        "================================================================================";
    let parts: Vec<&str> = output.split(SEP).collect();
    // parts[1] = VERSION 1 header, parts[2] = version 1 content
    // parts[3] = VERSION 2 header, parts[4] = version 2 content
    let get = |i: usize| parts.get(i).unwrap_or(&"").trim().to_string();
    [get(2), get(4)]
}

fn summarize_content(
    file_content: &str,
    bpe: &tiktoken_rs::CoreBPE,
    model: Option<&str>,
    effort: Option<&str>,
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

    let mut partial_summaries = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let chunk_tokens = bpe.encode_with_special_tokens(chunk).len();
        info!(
            chunk = i + 1,
            total = chunks.len(),
            chunk_tokens,
            "processing chunk"
        );
        match summarize_chunk(chunk, model, effort) {
            Ok(summary) => partial_summaries.push(summary),
            Err(e) => warn!(chunk = i + 1, error = %e, "chunk failed"),
        }
    }

    if partial_summaries.is_empty() {
        return Err("All chunks failed to process".to_string());
    }

    info!(count = partial_summaries.len(), "merging partial summaries");
    merge_summaries(&partial_summaries, model, effort)
}

fn summarize_content_direct(
    content: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<String, String> {
    call_claude(&format!("{}{}", PROMPT_TEMPLATE, content), model, effort)
}

fn summarize_chunk(
    chunk: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<String, String> {
    call_claude(
        &format!("{}{}", CHUNK_PROMPT_TEMPLATE, chunk),
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
        &format!("{}{}", MERGE_PROMPT_TEMPLATE, combined),
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
}
