use clap::Parser;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tiktoken_rs::cl100k_base;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const MAX_TOKENS_PER_CHUNK: usize = 100_000;

#[derive(Parser, Debug)]
#[command(name = "diffanalyzer", about = "Generate resume/LinkedIn text from git diff")]
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
}

const PROMPT_TEMPLATE: &str = r#"You are a senior technical resume writer. Analyze the following git diff content and produce THREE versions of a project summary in a single output.

Carefully examine: file paths, package names, import statements, frameworks, APIs, database schemas, config files, commit messages, test files, CI/CD configs, and code patterns. Extract every piece of technical evidence you can find.

Output ALL THREE versions below, separated by the exact delimiter shown. Entirely in English.

================================================================================
VERSION 1: PROFESSIONAL RESUME
================================================================================

# <Project Name>
📅 MM/YYYY - MM/YYYY

## Summary
A concise paragraph (2-3 sentences) describing the project, its domain, and your role.

## Key Accomplishments
- 4-6 bullet points, each one concise sentence
- Follow the pattern: "[Action verb] [what] using [technology], resulting in [impact]"
- Focus on the most impressive, resume-worthy achievements
- Quantify where possible (endpoints, models, performance gains, etc.)

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
- 6-12 detailed bullet points of concrete technical work evidenced in the diffs
- Each bullet should be specific and descriptive (1-2 sentences each)
- Follow the pattern: "Implemented/Designed/Built/Optimized [specific feature] by [technical approach], enabling [outcome/capability]"
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

================================================================================
VERSION 3: LINKEDIN PROJECT
================================================================================

# <Project Name>
<Your Role> | MM/YYYY – MM/YYYY

<2-3 sentences: what you built, your specific contributions, and one standout outcome. Write in first person, professional but conversational — suitable for LinkedIn's project section.>

**Tech:** Language, Framework, Key Libraries, Database, Infrastructure

Rules:
- Only claim what is directly evidenced in the diffs. Do not fabricate features or technologies.
- Version 1 should be polished and human-readable — ready to paste into a resume.
- Version 2 should be verbose and thorough — it will be consumed by an LLM for generating tailored resumes, cover letters, and interview prep. More detail is better.
- Version 3 should be concise and LinkedIn-optimized — 2-3 sentences max, first person.
- When uncertain about something, phrase it as "appears to" or "likely" rather than omitting it.
- Include raw technical details (class names, endpoint paths, config keys) in Version 2.
- If the diffs are too small for a full analysis, still extract every detail possible and note the limited scope.

Output ONLY the three versions with the delimiters, no additional commentary.

=== GIT DIFF CONTENT STARTS HERE ===
"#;

const CHUNK_PROMPT_TEMPLATE: &str = r#"Extract every piece of technical information from this git diff chunk. This will be merged with other chunks to build a comprehensive project summary for resume generation.

Be exhaustive — capture everything visible:

1. **Project/Repository**: name, path, organization (if visible)
2. **Date range**: commit dates (if visible)
3. **Technologies**: every language, framework, library, database, infrastructure tool, and service evidenced
4. **Features & APIs**: specific endpoints, controllers, services, data models, UI components implemented or modified
5. **Architecture**: patterns (MVC, microservices, event-driven, etc.), module organization, layer separation
6. **Data models**: schemas, entities, migrations, relationships
7. **Infrastructure**: Docker, CI/CD, cloud services, deployment configs
8. **Testing**: test files, testing frameworks, test patterns
9. **Technical complexity**: algorithms, business logic, error handling, security measures
10. **Domain context**: what business problem does this code solve? Who are the users?
11. **Code quality signals**: logging, documentation, error handling patterns, code organization

Output detailed structured bullet points under each category. Include specific class names, file paths, and endpoint paths where visible. Skip categories that have no evidence in this chunk.

=== GIT DIFF CHUNK ===
"#;

const MERGE_PROMPT_TEMPLATE: &str = r#"You are a senior technical resume writer. Based on the following partial analyses from different chunks of the same git diff file, create THREE versions of a unified project summary.

Deduplicate, synthesize, and enrich the information. Combine overlapping details and resolve any contradictions.

Output ALL THREE versions below, separated by the exact delimiter shown. Entirely in English.

================================================================================
VERSION 1: PROFESSIONAL RESUME
================================================================================

# <Project Name>
📅 MM/YYYY - MM/YYYY

## Summary
A concise paragraph (2-3 sentences) describing the project, its domain, and your role.

## Key Accomplishments
- 4-6 bullet points, each one concise sentence
- Follow the pattern: "[Action verb] [what] using [technology], resulting in [impact]"
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
- 6-12 detailed bullet points of concrete technical work
- Each bullet: 1-2 sentences, specific and descriptive
- Pattern: "Implemented/Designed/Built/Optimized [specific feature] by [technical approach], enabling [outcome/capability]"
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

================================================================================
VERSION 3: LINKEDIN PROJECT
================================================================================

# <Project Name>
<Your Role> | MM/YYYY – MM/YYYY

<2-3 sentences: what you built, your specific contributions, and one standout outcome. Write in first person, professional but conversational.>

**Tech:** Language, Framework, Key Libraries, Database, Infrastructure

Rules:
- Only claim what is directly evidenced in the partial analyses. Do not fabricate.
- Version 1 should be polished and human-readable — ready to paste into a resume.
- Version 2 should be verbose and thorough — more detail is better for downstream LLM consumption.
- Version 3 should be concise and LinkedIn-optimized — 2-3 sentences max, first person.
- When uncertain, phrase as "appears to" or "likely" rather than omitting.
- Include raw technical details (class names, endpoint paths, config keys) in Version 2.

Output ONLY the three versions with the delimiters, no additional commentary.

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

    let input = match args.input {
        Some(p) => p,
        None => default_input()?,
    };

    if !input.exists() {
        return Err(format!("Input file not found: {}", input.display()));
    }

    let output_dir = match args.output_dir {
        Some(p) => p,
        None => input
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| "Cannot determine output dir from input path".to_string())?,
    };

    fs::create_dir_all(&output_dir).map_err(|e| format!("Failed to create output dir: {}", e))?;

    let bpe = cl100k_base().expect("Failed to initialize tokenizer");

    info!("processing: {}", input.display());

    let output = summarize_file(&input, &bpe, model)?;
    let [resume, summary, linkedin] = split_versions(&output);

    write_output(&output_dir.join("resume.txt"), &resume)?;
    write_output(&output_dir.join("summary.txt"), &summary)?;
    write_output(&output_dir.join("linkedin.txt"), &linkedin)?;

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
    Ok(PathBuf::from(home).join("Downloads").join(name).join("diff.txt"))
}

fn write_output(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    info!("→ {}", path.display());
    Ok(())
}

fn split_versions(output: &str) -> [String; 3] {
    const SEP: &str = "================================================================================";
    let parts: Vec<&str> = output.split(SEP).collect();
    // parts[1] = VERSION 1 header, parts[2] = version 1 content
    // parts[3] = VERSION 2 header, parts[4] = version 2 content
    // parts[5] = VERSION 3 header, parts[6] = version 3 content
    let get = |i: usize| parts.get(i).unwrap_or(&"").trim().to_string();
    [get(2), get(4), get(6)]
}

fn summarize_file(
    file_path: &Path,
    bpe: &tiktoken_rs::CoreBPE,
    model: Option<&str>,
) -> Result<String, String> {
    let file_content =
        fs::read_to_string(file_path).map_err(|e| format!("Failed to read file: {}", e))?;

    let token_count = bpe.encode_with_special_tokens(&file_content).len();
    info!(token_count, "file token count");

    if token_count <= MAX_TOKENS_PER_CHUNK {
        return summarize_content_direct(&file_content, model);
    }

    info!(token_count, max = MAX_TOKENS_PER_CHUNK, "large file, processing in chunks");

    let chunks = split_into_chunks_by_tokens(&file_content, bpe, MAX_TOKENS_PER_CHUNK);
    info!(chunks = chunks.len(), "split into chunks");

    let mut partial_summaries = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let chunk_tokens = bpe.encode_with_special_tokens(chunk).len();
        info!(chunk = i + 1, total = chunks.len(), chunk_tokens, "processing chunk");
        match summarize_chunk(chunk, model) {
            Ok(summary) => partial_summaries.push(summary),
            Err(e) => warn!(chunk = i + 1, error = %e, "chunk failed"),
        }
    }

    if partial_summaries.is_empty() {
        return Err("All chunks failed to process".to_string());
    }

    info!(count = partial_summaries.len(), "merging partial summaries");
    merge_summaries(&partial_summaries, model)
}

fn summarize_content_direct(content: &str, model: Option<&str>) -> Result<String, String> {
    call_claude(&format!("{}{}", PROMPT_TEMPLATE, content), model)
}

fn summarize_chunk(chunk: &str, model: Option<&str>) -> Result<String, String> {
    call_claude(&format!("{}{}", CHUNK_PROMPT_TEMPLATE, chunk), model)
}

fn merge_summaries(summaries: &[String], model: Option<&str>) -> Result<String, String> {
    let combined = summaries.join("\n\n---\n\n");
    call_claude(&format!("{}{}", MERGE_PROMPT_TEMPLATE, combined), model)
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
        let line_with_newline = format!("{}\n", line);
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

fn call_claude(prompt: &str, model: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.arg("--print");

    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn claude: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| format!("Failed to write to stdin: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for claude: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let exit_code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let is_rate_limit = stdout.contains("hit your limit") || stderr.contains("hit your limit");

        if is_rate_limit {
            return Err(format!("RATE_LIMIT: Claude exited with code {exit_code}."));
        }
        return Err(format!("Claude exited with code {exit_code}.\nstdout: {stdout}\nstderr: {stderr}"));
    }

    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if result.is_empty() {
        return Err("Empty response from Claude".to_string());
    }
    Ok(result)
}
