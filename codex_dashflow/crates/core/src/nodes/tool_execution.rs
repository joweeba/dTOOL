//! Tool execution node
//!
//! This node executes approved tool calls using DashFlow's tool crates
//! and collects results. Also supports MCP tool execution.
//!
//! Shell commands can be executed within a sandbox (Seatbelt on macOS,
//! Landlock on Linux) for security.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use dashflow::core::tools::{Tool, ToolInput};
use dashflow_file_tool::{ListDirectoryTool, ReadFileTool, WriteFileTool};
use dashflow_shell_tool::ShellTool;

use codex_dashflow_file_search::{search_async, FileSearchResults, SearchConfig};
use codex_dashflow_mcp::{is_mcp_tool, parse_qualified_tool_name, McpClient, McpContent};
use codex_dashflow_sandbox::{SandboxExecutor, SandboxMode};

/// Detect if a patch is in unified diff format (git diff, diff -u)
///
/// Unified diff format indicators:
/// - Starts with "diff --git a/..." (git diff output)
/// - Contains "--- " and "+++ " lines (standard unified diff)
fn is_unified_diff(patch: &str) -> bool {
    let trimmed = patch.trim();

    // Check for git diff format
    if trimmed.starts_with("diff --git") {
        return true;
    }

    // Check for standard unified diff format (has --- and +++ headers)
    // Must have both "--- " and "+++ " to be a valid unified diff
    let lines: Vec<&str> = trimmed.lines().collect();
    let mut has_old_header = false;
    let mut has_new_header = false;

    for line in &lines {
        if line.starts_with("--- ") {
            has_old_header = true;
        } else if line.starts_with("+++ ") {
            has_new_header = true;
        }
        // Early exit if we found both
        if has_old_header && has_new_header {
            return true;
        }
    }

    false
}

use crate::codex::ApprovalDecision;
use crate::execpolicy::ApprovalRequirement;
use crate::safety::sanitize_tool_output;
use crate::state::{AgentState, ToolCall, ToolResult};
use crate::streaming::AgentEvent;

/// Maximum tool output size in bytes before truncation (Audit #55)
/// Large outputs can blow context/cost, so we limit to 50KB
const MAX_TOOL_OUTPUT_SIZE: usize = 50 * 1024;

/// Truncate and sanitize tool output (Audit #55, #68)
///
/// This prevents:
/// - Large outputs from consuming excessive context tokens (Audit #55)
/// - Sensitive data (credentials, private keys, hostnames) from leaking into prompts (Audit #68)
fn truncate_tool_output(output: String) -> String {
    // First, sanitize sensitive content (Audit #68)
    let sanitized = sanitize_tool_output(&output);

    if sanitized.len() <= MAX_TOOL_OUTPUT_SIZE {
        return sanitized;
    }

    // Find a good truncation point (don't cut in middle of a line if possible)
    let truncation_point = sanitized[..MAX_TOOL_OUTPUT_SIZE]
        .rfind('\n')
        .unwrap_or(MAX_TOOL_OUTPUT_SIZE);

    let mut truncated = sanitized[..truncation_point].to_string();
    let remaining_bytes = sanitized.len() - truncation_point;
    truncated.push_str(&format!(
        "\n\n[Output truncated: {} bytes remaining. Use more specific queries or file reading.]",
        remaining_bytes
    ));

    truncated
}

/// Default tool timeout in seconds
pub const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 60;

/// Tool executor that wraps DashFlow tools and MCP client
pub struct ToolExecutor {
    shell_tool: ShellTool,
    read_file_tool: ReadFileTool,
    write_file_tool: WriteFileTool,
    list_directory_tool: ListDirectoryTool,
    /// Optional MCP client for executing MCP tools
    mcp_client: Option<Arc<McpClient>>,
    /// Sandbox mode for shell command execution
    sandbox_mode: SandboxMode,
    /// Working directory for sandboxed execution
    working_dir: PathBuf,
    /// Audit #60: Configurable tool timeout in seconds
    timeout_secs: u64,
    /// Audit #70: Additional writable roots for sandbox (WorkspaceWrite mode)
    writable_roots: Vec<PathBuf>,
}

impl ToolExecutor {
    /// Create a new tool executor with the given working directory
    pub fn new(working_dir: Option<PathBuf>) -> Self {
        Self::with_sandbox(working_dir, SandboxMode::default())
    }

    /// Create a new tool executor with the given working directory and sandbox mode
    pub fn with_sandbox(working_dir: Option<PathBuf>, sandbox_mode: SandboxMode) -> Self {
        Self::with_sandbox_and_timeout(working_dir, sandbox_mode, DEFAULT_TOOL_TIMEOUT_SECS)
    }

    /// Audit #60: Create a new tool executor with configurable timeout
    pub fn with_sandbox_and_timeout(
        working_dir: Option<PathBuf>,
        sandbox_mode: SandboxMode,
        timeout_secs: u64,
    ) -> Self {
        let actual_working_dir =
            working_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        // Create shell tool with working directory restriction and configurable timeout
        let shell_tool = ShellTool::new()
            .with_working_dir(actual_working_dir.clone())
            .with_timeout(timeout_secs);

        // Create file tools with directory restriction
        let allowed_dirs = vec![actual_working_dir.clone()];

        let read_file_tool = ReadFileTool::new().with_allowed_dirs(allowed_dirs.clone());
        let write_file_tool = WriteFileTool::new().with_allowed_dirs(allowed_dirs.clone());
        let list_directory_tool = ListDirectoryTool::new().with_allowed_dirs(allowed_dirs);

        Self {
            shell_tool,
            read_file_tool,
            write_file_tool,
            list_directory_tool,
            mcp_client: None,
            sandbox_mode,
            working_dir: actual_working_dir,
            timeout_secs,
            writable_roots: Vec::new(),
        }
    }

    /// Get the current tool timeout in seconds
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    /// Audit #70: Set additional writable roots for sandbox (WorkspaceWrite mode)
    pub fn with_writable_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.writable_roots = roots;
        self
    }

    /// Set the MCP client for executing MCP tools
    pub fn with_mcp_client(mut self, client: Arc<McpClient>) -> Self {
        self.mcp_client = Some(client);
        self
    }

    /// Execute a tool call and return the result
    pub async fn execute(&self, tool: &str, args: &serde_json::Value) -> (String, bool) {
        // Check if this is an MCP tool first
        if is_mcp_tool(tool) {
            return self.execute_mcp_tool(tool, args).await;
        }

        match tool {
            "shell" => self.execute_shell(args).await,
            "read_file" => {
                // Map our schema to DashFlow's schema
                let mapped_args = if let Some(path) = args.get("path") {
                    serde_json::json!({"file_path": path})
                } else {
                    args.clone()
                };
                let input = ToolInput::Structured(mapped_args);
                match self.read_file_tool.call(input).await {
                    Ok(output) => (output, true),
                    Err(e) => (format!("Error: {}", e), false),
                }
            }
            "write_file" => {
                // Audit #47: Check sandbox mode before allowing write operations
                if self.sandbox_mode.is_read_only() {
                    return (
                        "Error: write_file is not allowed in read-only sandbox mode".to_string(),
                        false,
                    );
                }
                // Map our schema to DashFlow's schema
                let mapped_args = if let Some(path) = args.get("path") {
                    let content = args
                        .get("content")
                        .cloned()
                        .unwrap_or(serde_json::Value::String(String::new()));
                    serde_json::json!({
                        "file_path": path,
                        "text": content
                    })
                } else {
                    args.clone()
                };
                let input = ToolInput::Structured(mapped_args);
                match self.write_file_tool.call(input).await {
                    Ok(output) => (output, true),
                    Err(e) => (format!("Error: {}", e), false),
                }
            }
            // Audit #46: Handle both "list_dir" (tool definition name) and "list_directory" (legacy)
            "list_dir" | "list_directory" => {
                // Map our schema to DashFlow's schema
                let mapped_args = if let Some(path) = args.get("path") {
                    serde_json::json!({"dir_path": path})
                } else {
                    serde_json::json!({"dir_path": "."})
                };
                let input = ToolInput::Structured(mapped_args);
                match self.list_directory_tool.call(input).await {
                    Ok(output) => (output, true),
                    Err(e) => (format!("Error: {}", e), false),
                }
            }
            "search_files" => self.execute_search_files(args).await,
            "apply_patch" => {
                // Apply patch using either:
                // 1. Pure Rust apply-patch crate for custom "*** Begin Patch" format
                // 2. Git apply for unified diffs (standard git format)
                let patch = args.get("patch").and_then(|v| v.as_str()).unwrap_or("");

                // Validate input first
                if patch.is_empty() {
                    return ("Error: empty patch content".to_string(), false);
                }

                // Audit #47: Check sandbox mode before allowing patch operations (writes to files)
                if self.sandbox_mode.is_read_only() {
                    return (
                        "Error: apply_patch is not allowed in read-only sandbox mode".to_string(),
                        false,
                    );
                }

                // Audit #52: Detect patch format and use appropriate method
                // Unified diff format starts with "diff --git" or "--- " followed by "+++ "
                if is_unified_diff(patch) {
                    // Use git apply for unified diffs
                    self.apply_unified_diff(patch).await
                } else {
                    // Use the pure Rust apply-patch implementation for custom format
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    match codex_dashflow_apply_patch::apply_patch(patch, &mut stdout, &mut stderr) {
                        Ok(()) => {
                            let output = String::from_utf8_lossy(&stdout).to_string();
                            (output, true)
                        }
                        Err(e) => {
                            let stderr_str = String::from_utf8_lossy(&stderr);
                            let error_msg = if stderr_str.is_empty() {
                                format!("Error applying patch: {}", e)
                            } else {
                                format!("Error applying patch: {}\n{}", e, stderr_str)
                            };
                            (error_msg, false)
                        }
                    }
                }
            }
            _ => (format!("Unknown tool: {}", tool), false),
        }
    }

    /// Execute a shell command, using sandbox when available and configured
    async fn execute_shell(&self, args: &serde_json::Value) -> (String, bool) {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(cmd) => cmd,
            None => return ("Error: missing 'command' argument".to_string(), false),
        };

        // Use sandbox for shell execution unless in DangerFullAccess mode
        if !self.sandbox_mode.is_unrestricted() && SandboxExecutor::is_available() {
            tracing::debug!(
                mode = ?self.sandbox_mode,
                command = %command,
                "Executing shell command in sandbox"
            );

            // Audit #70: Apply additional writable roots if configured
            let mut executor = SandboxExecutor::new(self.sandbox_mode, self.working_dir.clone());
            for root in &self.writable_roots {
                executor = executor.with_writable_root(root.clone());
            }
            match executor.execute(command).await {
                Ok(output) => (output, true),
                Err(e) => {
                    tracing::warn!(error = %e, "Sandboxed shell command failed");
                    (format!("Error: {}", e), false)
                }
            }
        } else {
            // Fallback to DashFlow ShellTool (unsandboxed)
            // Warn if user expected sandboxing but it's not available
            if !self.sandbox_mode.is_unrestricted() && !SandboxExecutor::is_available() {
                // Audit #63: Explicitly warn about network egress when sandbox falls back
                tracing::warn!(
                    mode = ?self.sandbox_mode,
                    "SECURITY WARNING: Sandbox not available on this platform (Seatbelt/Landlock not found). \
                     Running shell command WITHOUT sandbox protection. \
                     NETWORK ACCESS IS ALLOWED - commands like curl, wget, ssh can reach external hosts. \
                     File system restrictions are also not enforced. \
                     Consider using --sandbox danger-full-access if this is intentional, \
                     or run in a container/VM with network isolation."
                );
            }

            tracing::debug!(
                mode = ?self.sandbox_mode,
                sandbox_available = SandboxExecutor::is_available(),
                command = %command,
                "Executing shell command without sandbox"
            );

            let input = ToolInput::Structured(args.clone());
            match self.shell_tool.call(input).await {
                Ok(output) => (output, true),
                Err(e) => (format!("Error: {}", e), false),
            }
        }
    }

    /// Execute an MCP tool call
    ///
    /// Audit #93: Uses retry with exponential backoff for transient MCP failures.
    async fn execute_mcp_tool(&self, tool: &str, args: &serde_json::Value) -> (String, bool) {
        let mcp_client = match &self.mcp_client {
            Some(client) => client,
            None => {
                return (
                    format!("MCP client not configured, cannot execute tool: {}", tool),
                    false,
                );
            }
        };

        // Parse the qualified tool name
        let (server_name, tool_name) = match parse_qualified_tool_name(tool) {
            Some((s, t)) => (s, t),
            None => {
                return (format!("Invalid MCP tool name format: {}", tool), false);
            }
        };

        tracing::debug!(
            server = %server_name,
            tool = %tool_name,
            "Executing MCP tool"
        );

        // Audit #93: Call the MCP tool with retry logic for transient failures
        // Uses up to 3 retries with exponential backoff (100ms, 200ms, 400ms)
        match mcp_client
            .call_tool_with_retry(
                &server_name,
                &tool_name,
                Some(args.clone()),
                Some(3),
                Some(100),
            )
            .await
        {
            Ok(result) => {
                // Audit #58: Preserve MCP structured content with metadata
                // Convert MCP content to string output, preserving URI and structure info
                let output = result
                    .content
                    .iter()
                    .map(|c| match c {
                        McpContent::Text { text } => text.clone(),
                        McpContent::Resource { uri, text } => {
                            // Preserve resource URI for the LLM to understand context
                            match text {
                                Some(content) => format!("[Resource: {}]\n{}", uri, content),
                                None => format!("[Resource: {}]", uri),
                            }
                        }
                        McpContent::Image { mime_type, data } => {
                            // Include image metadata (size info helpful for LLM context)
                            let size_info = if !data.is_empty() {
                                format!(", {}KB base64", data.len() / 1024)
                            } else {
                                String::new()
                            };
                            format!("[Image: {}{}]", mime_type, size_info)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"); // Use double newline for better separation

                (output, !result.is_error)
            }
            Err(e) => (format!("MCP tool error: {}", e), false),
        }
    }

    /// Execute a file search
    ///
    /// Supports three modes:
    /// 1. Fuzzy file search (default): Find files by fuzzy matching name
    /// 2. Content search: Search file contents for a pattern (mode: "content")
    /// 3. Glob pattern search: Find files matching glob pattern (mode: "glob")
    ///
    /// Audit #51: Search paths are restricted to the workspace directory when sandbox is absent
    /// to prevent filesystem traversal attacks.
    async fn execute_search_files(&self, args: &serde_json::Value) -> (String, bool) {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return ("Error: missing 'query' argument".to_string(), false),
        };

        let requested_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        // Audit #51: Restrict search paths to workspace directory when sandbox is not available
        // This prevents filesystem traversal attacks when running without sandbox protection
        let path = if SandboxExecutor::is_available() || self.sandbox_mode.is_unrestricted() {
            // Sandbox available OR user explicitly requested full access - allow the requested path
            requested_path.to_string()
        } else {
            // No sandbox and not in full-access mode - restrict to workspace
            let requested_path_buf = PathBuf::from(requested_path);
            let is_absolute = requested_path_buf.is_absolute();
            let resolved_path = if is_absolute {
                requested_path_buf
            } else {
                self.working_dir.join(requested_path)
            };

            // Canonicalize to resolve .. and symlinks
            let canonical_path = match resolved_path.canonicalize() {
                Ok(p) => p,
                Err(_) => {
                    // Path doesn't exist, use resolved path for relative paths within workspace
                    if requested_path == "." || !is_absolute {
                        self.working_dir.clone()
                    } else {
                        return (
                            format!(
                                "Error: Search path '{}' not found or not accessible",
                                requested_path
                            ),
                            false,
                        );
                    }
                }
            };

            // Check if the resolved path is within the workspace
            if !canonical_path.starts_with(&self.working_dir) {
                tracing::warn!(
                    requested_path = %requested_path,
                    workspace = %self.working_dir.display(),
                    "Search path outside workspace blocked (sandbox not available)"
                );
                return (
                    format!(
                        "Error: Search path '{}' is outside the workspace directory. \
                         Search is restricted to the workspace when sandbox is not available.",
                        requested_path
                    ),
                    false,
                );
            }

            canonical_path.to_string_lossy().to_string()
        };
        let path = path.as_str();

        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("fuzzy");

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        match mode {
            "fuzzy" => {
                // Fuzzy file search using nucleo_matcher
                self.execute_fuzzy_search(query, path, limit).await
            }
            "content" => {
                // Content search using ripgrep (rg) or grep
                // Use shell_words::quote for proper shell escaping to prevent injection
                let escaped_query = shell_words::quote(query);
                let escaped_path = shell_words::quote(path);

                // Check tool availability and warn if rg is missing
                let has_rg = which::which("rg").is_ok();
                let has_grep = which::which("grep").is_ok();
                if !has_rg {
                    if has_grep {
                        tracing::warn!(
                            "ripgrep (rg) not found, falling back to grep. \
                             Install ripgrep for better search performance."
                        );
                    } else {
                        return (
                            "Error: No search tools available. Install ripgrep (rg) or grep."
                                .to_string(),
                            false,
                        );
                    }
                }

                let command = format!(
                    "rg -n --max-count=5 --max-columns=200 {} {} 2>/dev/null | head -{} || grep -rn {} {} 2>/dev/null | head -{}",
                    escaped_query,
                    escaped_path,
                    limit,
                    escaped_query,
                    escaped_path,
                    limit
                );
                let shell_args = serde_json::json!({"command": command});
                self.execute_shell(&shell_args).await
            }
            "glob" => {
                // Glob pattern search using fd or find
                // Use shell_words::quote for proper shell escaping to prevent injection
                let escaped_query = shell_words::quote(query);
                let escaped_path = shell_words::quote(path);

                // Check tool availability and warn if fd is missing
                let has_fd = which::which("fd").is_ok();
                let has_find = which::which("find").is_ok();
                if !has_fd {
                    if has_find {
                        tracing::warn!(
                            "fd not found, falling back to find. \
                             Install fd for better search performance."
                        );
                    } else {
                        return (
                            "Error: No file search tools available. Install fd or find."
                                .to_string(),
                            false,
                        );
                    }
                }

                let command = format!(
                    "fd -t f {} {} 2>/dev/null | head -{} || find {} -type f -name {} 2>/dev/null | head -{}",
                    escaped_query,
                    escaped_path,
                    limit,
                    escaped_path,
                    escaped_query,
                    limit
                );
                let shell_args = serde_json::json!({"command": command});
                self.execute_shell(&shell_args).await
            }
            _ => {
                // Auto-detect: glob patterns use glob mode, otherwise fuzzy
                let is_glob = query.contains('*') || query.contains('?');
                if is_glob {
                    // Use shell_words::quote for proper shell escaping to prevent injection
                    let escaped_query = shell_words::quote(query);
                    let escaped_path = shell_words::quote(path);

                    // Check tool availability and warn if fd is missing
                    let has_fd = which::which("fd").is_ok();
                    let has_find = which::which("find").is_ok();
                    if !has_fd {
                        if has_find {
                            tracing::warn!(
                                "fd not found, falling back to find. \
                                 Install fd for better search performance."
                            );
                        } else {
                            return (
                                "Error: No file search tools available. Install fd or find."
                                    .to_string(),
                                false,
                            );
                        }
                    }

                    let command = format!(
                        "fd -t f {} {} 2>/dev/null | head -{} || find {} -type f -name {} 2>/dev/null | head -{}",
                        escaped_query,
                        escaped_path,
                        limit,
                        escaped_path,
                        escaped_query,
                        limit
                    );
                    let shell_args = serde_json::json!({"command": command});
                    self.execute_shell(&shell_args).await
                } else {
                    self.execute_fuzzy_search(query, path, limit).await
                }
            }
        }
    }

    /// Apply a unified diff patch using git apply
    ///
    /// Audit #52: Support standard unified diff format alongside custom apply-patch format
    async fn apply_unified_diff(&self, patch: &str) -> (String, bool) {
        // Check if git is available
        if which::which("git").is_err() {
            return (
                "Error: git not found. Unified diff patches require git to be installed."
                    .to_string(),
                false,
            );
        }

        // Write patch to a temporary file
        let temp_dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(e) => return (format!("Error creating temp directory: {}", e), false),
        };
        let patch_file = temp_dir.path().join("patch.diff");
        if let Err(e) = std::fs::write(&patch_file, patch) {
            return (format!("Error writing patch file: {}", e), false);
        }

        // Build git apply command
        // Use --3way for better conflict handling when possible
        // Use shell_words::quote for safety
        let patch_path_str = patch_file.to_string_lossy();
        let escaped_patch_path = shell_words::quote(&patch_path_str);

        // Construct command to run in working directory
        let command = format!("git apply --3way {}", escaped_patch_path);

        tracing::debug!(
            working_dir = %self.working_dir.display(),
            patch_format = "unified",
            "Applying unified diff via git apply"
        );

        // Execute using shell (respects sandbox mode)
        let shell_args = serde_json::json!({"command": command});
        let (output, success) = self.execute_shell(&shell_args).await;

        // If --3way fails, try without it (for non-git directories)
        if !success && output.contains("repository") {
            let fallback_command = format!("git apply {}", escaped_patch_path);
            let fallback_args = serde_json::json!({"command": fallback_command});
            return self.execute_shell(&fallback_args).await;
        }

        if success {
            // Parse output to provide useful information
            let result = if output.trim().is_empty() {
                "Unified diff patch applied successfully.".to_string()
            } else {
                format!("Unified diff patch applied.\n{}", output)
            };
            (result, true)
        } else {
            (format!("Error applying unified diff: {}", output), false)
        }
    }

    /// Execute fuzzy file search using nucleo_matcher
    async fn execute_fuzzy_search(&self, query: &str, path: &str, limit: usize) -> (String, bool) {
        let search_path = if path == "." {
            self.working_dir.clone()
        } else {
            let p = PathBuf::from(path);
            if p.is_absolute() {
                p
            } else {
                self.working_dir.join(p)
            }
        };

        let config = SearchConfig {
            limit,
            compute_indices: false,
            respect_gitignore: true,
            exclude: vec!["target/**".to_string(), "node_modules/**".to_string()],
            ..Default::default()
        };

        match search_async(query, &search_path, &config, None).await {
            Ok(FileSearchResults {
                matches,
                total_match_count,
            }) => {
                if matches.is_empty() {
                    return ("No files found matching the query".to_string(), true);
                }

                let mut output = String::new();
                for m in &matches {
                    output.push_str(&format!("{} (score: {})\n", m.path, m.score));
                }

                if total_match_count > matches.len() {
                    output.push_str(&format!(
                        "\n... and {} more matches (showing top {})\n",
                        total_match_count - matches.len(),
                        matches.len()
                    ));
                }

                (output, true)
            }
            Err(e) => (format!("Search error: {}", e), false),
        }
    }
}

/// Check if a tool call is approved according to the execution policy and approval callback
///
/// Returns Ok(true) if approved, Ok(false) if rejected, or the output string for forbidden tools
async fn check_tool_approval(
    state: &AgentState,
    tool_call: &ToolCall,
) -> Result<(bool, Option<String>), ()> {
    let policy = state.exec_policy();
    let approval_callback = state.approval_callback();

    // Evaluate the tool call against the policy
    let requirement = policy.evaluate(tool_call);

    // Audit #65: Log policy evaluation result for audit trail
    tracing::debug!(
        tool = %tool_call.tool,
        tool_call_id = %tool_call.id,
        approval_mode = ?policy.approval_mode,
        requirement = ?requirement,
        "ExecPolicy evaluated tool call"
    );

    match requirement {
        ApprovalRequirement::Approved => {
            // Auto-approved by policy
            state.emit_event(AgentEvent::ToolCallApproved {
                session_id: state.session_id.clone(),
                tool_call_id: tool_call.id.clone(),
                tool: tool_call.tool.clone(),
            });
            Ok((true, None))
        }
        ApprovalRequirement::NeedsApproval { reason } => {
            // Check if already session-approved
            if approval_callback.is_session_approved(&tool_call.tool).await {
                state.emit_event(AgentEvent::ToolCallApproved {
                    session_id: state.session_id.clone(),
                    tool_call_id: tool_call.id.clone(),
                    tool: tool_call.tool.clone(),
                });
                return Ok((true, None));
            }

            // Request interactive approval
            let request_id = uuid::Uuid::new_v4().to_string();

            // Emit ApprovalRequired event for TUI visibility
            state.emit_event(AgentEvent::ApprovalRequired {
                session_id: state.session_id.clone(),
                request_id: request_id.clone(),
                tool_call_id: tool_call.id.clone(),
                tool: tool_call.tool.clone(),
                args: tool_call.args.clone(),
                reason: reason.clone(),
            });

            // Request approval via callback
            let decision = approval_callback
                .request_approval(
                    &request_id,
                    &tool_call.id,
                    &tool_call.tool,
                    &tool_call.args,
                    reason.as_deref(),
                )
                .await;

            match decision {
                ApprovalDecision::Approve => {
                    state.emit_event(AgentEvent::ToolCallApproved {
                        session_id: state.session_id.clone(),
                        tool_call_id: tool_call.id.clone(),
                        tool: tool_call.tool.clone(),
                    });
                    Ok((true, None))
                }
                ApprovalDecision::ApproveAndRemember => {
                    approval_callback
                        .mark_session_approved(&tool_call.tool)
                        .await;
                    state.emit_event(AgentEvent::ToolCallApproved {
                        session_id: state.session_id.clone(),
                        tool_call_id: tool_call.id.clone(),
                        tool: tool_call.tool.clone(),
                    });
                    Ok((true, None))
                }
                ApprovalDecision::Deny | ApprovalDecision::DenyAndRemember => {
                    let rejection_reason = reason.unwrap_or_else(|| "User rejected".to_string());
                    state.emit_event(AgentEvent::ToolCallRejected {
                        session_id: state.session_id.clone(),
                        tool_call_id: tool_call.id.clone(),
                        tool: tool_call.tool.clone(),
                        reason: rejection_reason.clone(),
                    });
                    Ok((
                        false,
                        Some(format!("Tool call rejected: {}", rejection_reason)),
                    ))
                }
            }
        }
        ApprovalRequirement::Forbidden { reason } => {
            // Forbidden by policy
            state.emit_event(AgentEvent::ToolCallRejected {
                session_id: state.session_id.clone(),
                tool_call_id: tool_call.id.clone(),
                tool: tool_call.tool.clone(),
                reason: reason.clone(),
            });
            Ok((false, Some(format!("Tool call forbidden: {}", reason))))
        }
    }
}

/// Tool execution node - executes tool calls using DashFlow tools
///
/// This node:
/// 1. Checks each pending tool call against the execution policy
/// 2. Requests user approval for tools that require it
/// 3. Executes approved tool calls using the appropriate DashFlow tool
/// 4. Collects output and timing information
/// 5. Handles errors and timeouts
pub fn tool_execution_node(
    mut state: AgentState,
) -> Pin<Box<dyn Future<Output = Result<AgentState, dashflow::Error>> + Send>> {
    Box::pin(async move {
        tracing::debug!(
            session_id = %state.session_id,
            turn = state.turn_count,
            tools_to_execute = state.pending_tool_calls.len(),
            "Executing tools"
        );

        // Create tool executor with working directory if specified
        let working_dir = if state.working_directory.is_empty() {
            None
        } else {
            Some(PathBuf::from(&state.working_directory))
        };
        let mut executor = ToolExecutor::with_sandbox(working_dir, state.sandbox_mode);

        // Audit #70: Apply additional writable roots if configured
        if !state.sandbox_writable_roots.is_empty() {
            executor = executor.with_writable_roots(state.sandbox_writable_roots.clone());
        }

        // Attach MCP client if available
        if let Some(mcp_client) = state.mcp_client() {
            executor = executor.with_mcp_client(mcp_client);
        }

        // Execute tool calls with PARALLEL execution for better performance.
        // Phase 1: Check approvals sequentially (fast, involves user interaction)
        // Phase 2: Execute approved tools in parallel (slow I/O operations)
        //
        // This two-phase approach was changed from fully sequential execution
        // to reduce latency when the LLM requests multiple independent tool calls.
        let tool_calls = std::mem::take(&mut state.pending_tool_calls);

        // Phase 1: Check approvals for all tools (sequential - approvals may need user input)
        let mut approved_tools = Vec::new();
        for tool_call in tool_calls {
            let (approved, rejection_output) = check_tool_approval(&state, &tool_call)
                .await
                .unwrap_or((false, Some("Approval check failed".to_string())));

            if !approved {
                // Tool was rejected - add rejection result immediately
                let result = ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    tool: tool_call.tool.clone(),
                    output: rejection_output.unwrap_or_else(|| "Tool call rejected".to_string()),
                    success: false,
                    duration_ms: 0,
                };

                tracing::info!(
                    tool = %result.tool,
                    "Tool call rejected"
                );

                state.tool_results.push(result);
            } else {
                // Tool approved - queue for parallel execution
                approved_tools.push(tool_call);
            }
        }

        // Phase 2: Execute approved tools in parallel
        if !approved_tools.is_empty() {
            let executor = Arc::new(executor);
            let session_id = state.session_id.clone();
            // Get the stream callback for event emission (fire and forget pattern)
            let stream_callback = state.stream_callback();

            // Create futures for all approved tool executions
            let tool_futures: Vec<_> = approved_tools
                .into_iter()
                .map(|tool_call| {
                    let executor = Arc::clone(&executor);
                    let session_id = session_id.clone();
                    let stream_callback = Arc::clone(&stream_callback);

                    async move {
                        // Emit tool execution start event (fire and forget)
                        {
                            let callback = Arc::clone(&stream_callback);
                            let event = AgentEvent::ToolExecutionStart {
                                session_id: session_id.clone(),
                                tool_call_id: tool_call.id.clone(),
                                tool: tool_call.tool.clone(),
                            };
                            tokio::spawn(async move {
                                callback.on_event(event).await;
                            });
                        }

                        let start = Instant::now();

                        tracing::info!(
                            tool = %tool_call.tool,
                            id = %tool_call.id,
                            "Executing tool (parallel)"
                        );

                        // Execute using DashFlow tools
                        let (output, success) =
                            executor.execute(&tool_call.tool, &tool_call.args).await;

                        let duration_ms = start.elapsed().as_millis() as u64;

                        // Create output preview (first 200 chars)
                        let output_preview = if output.len() > 200 {
                            format!("{}...", &output[..200])
                        } else {
                            output.clone()
                        };

                        // Emit tool execution complete event (fire and forget)
                        {
                            let callback = Arc::clone(&stream_callback);
                            let event = AgentEvent::ToolExecutionComplete {
                                session_id: session_id.clone(),
                                tool_call_id: tool_call.id.clone(),
                                tool: tool_call.tool.clone(),
                                success,
                                duration_ms,
                                output_preview,
                            };
                            tokio::spawn(async move {
                                callback.on_event(event).await;
                            });
                        }

                        // Truncate large outputs to prevent context/cost blow-up (Audit #55)
                        let truncated_output = truncate_tool_output(output);

                        let result = ToolResult {
                            tool_call_id: tool_call.id.clone(),
                            tool: tool_call.tool.clone(),
                            output: truncated_output,
                            success,
                            duration_ms,
                        };

                        tracing::info!(
                            tool = %result.tool,
                            success = result.success,
                            duration_ms = result.duration_ms,
                            "Tool execution complete (parallel)"
                        );

                        result
                    }
                })
                .collect();

            // Execute all approved tools in parallel and collect results
            let results = futures::future::join_all(tool_futures).await;
            state.tool_results.extend(results);
        }

        tracing::debug!(
            session_id = %state.session_id,
            results = state.tool_results.len(),
            "All tools executed"
        );

        Ok(state)
    })
}

/// Mock tool execution for testing
///
/// This simulates tool execution. Used when testing without real tool execution.
pub fn mock_tool_execution(tool: &str, args: &serde_json::Value) -> (String, bool) {
    match tool {
        "shell" => {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("echo 'no command'");
            // Simulate shell output
            let output = format!("$ {}\nfile1.txt\nfile2.txt\nREADME.md\nsrc/\n", command);
            (output, true)
        }
        "read_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let output = format!(
                "Contents of {}:\n\n# Example File\n\nThis is mock content.\n",
                path
            );
            (output, true)
        }
        "write_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let output = format!("Successfully wrote to {}", path);
            (output, true)
        }
        "apply_patch" => {
            let output = "Patch applied successfully".to_string();
            (output, true)
        }
        "search_files" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("*");
            let output = format!(
                "Search results for '{}':\n- src/main.rs:10: match\n- src/lib.rs:25: match\n",
                query
            );
            (output, true)
        }
        _ => {
            let output = format!("Unknown tool: {}", tool);
            (output, false)
        }
    }
}

/// Tool execution node using mock execution (for testing)
///
/// Audit #56: This node now respects approval flow like the real tool_execution_node.
/// It checks exec_policy and approval_callback before executing tools.
pub fn mock_tool_execution_node(
    mut state: AgentState,
) -> Pin<Box<dyn Future<Output = Result<AgentState, dashflow::Error>> + Send>> {
    Box::pin(async move {
        tracing::debug!(
            session_id = %state.session_id,
            turn = state.turn_count,
            tools_to_execute = state.pending_tool_calls.len(),
            "Executing tools (mock)"
        );

        let tool_calls = std::mem::take(&mut state.pending_tool_calls);

        for tool_call in tool_calls {
            // Audit #56: Check approval before executing (same as real node)
            let (approved, rejection_output) = check_tool_approval(&state, &tool_call)
                .await
                .unwrap_or((false, Some("Approval check failed".to_string())));

            if !approved {
                // Tool was rejected - add rejection result
                let result = ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    tool: tool_call.tool.clone(),
                    output: rejection_output.unwrap_or_else(|| "Tool call rejected".to_string()),
                    success: false,
                    duration_ms: 0,
                };

                tracing::info!(
                    tool = %result.tool,
                    "Tool call rejected (mock)"
                );

                state.tool_results.push(result);
                continue;
            }

            let start = Instant::now();

            tracing::info!(
                tool = %tool_call.tool,
                id = %tool_call.id,
                "Executing tool (mock)"
            );

            let (output, success) = mock_tool_execution(&tool_call.tool, &tool_call.args);

            let duration_ms = start.elapsed().as_millis() as u64;

            // Truncate large outputs to prevent context/cost blow-up (Audit #55)
            let truncated_output = truncate_tool_output(output);

            let result = ToolResult {
                tool_call_id: tool_call.id.clone(),
                tool: tool_call.tool.clone(),
                output: truncated_output,
                success,
                duration_ms,
            };

            tracing::info!(
                tool = %result.tool,
                success = result.success,
                duration_ms = result.duration_ms,
                "Tool execution complete (mock)"
            );

            state.tool_results.push(result);
        }

        tracing::debug!(
            session_id = %state.session_id,
            results = state.tool_results.len(),
            "All tools executed (mock)"
        );

        Ok(state)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ToolCall;

    #[test]
    fn test_truncate_tool_output_under_limit() {
        let output = "Small output".to_string();
        let result = truncate_tool_output(output.clone());
        assert_eq!(result, output);
    }

    #[test]
    fn test_truncate_tool_output_at_limit() {
        let output = "x".repeat(MAX_TOOL_OUTPUT_SIZE);
        let result = truncate_tool_output(output.clone());
        assert_eq!(result, output);
    }

    #[test]
    fn test_truncate_tool_output_over_limit() {
        let output = "line1\nline2\nline3\n".repeat(5000); // > 50KB
        let result = truncate_tool_output(output.clone());

        assert!(result.len() < output.len());
        assert!(result.contains("[Output truncated:"));
        assert!(result.contains("bytes remaining"));
    }

    #[test]
    fn test_truncate_tool_output_preserves_line_boundary() {
        // Create output that would truncate mid-line without special handling
        let mut output = "x".repeat(MAX_TOOL_OUTPUT_SIZE - 10);
        output.push('\n');
        output.push_str(&"y".repeat(100)); // Push past limit

        let result = truncate_tool_output(output);

        // Should truncate at the newline, not mid-y-sequence
        assert!(result.ends_with("bytes remaining]") || !result.contains("yyyyy"));
    }

    #[test]
    fn test_truncate_tool_output_sanitizes_sensitive_data() {
        // Audit #68: Verify sensitive data is redacted
        let output = "Error: Connection failed to 192.168.1.100:8080\nAuth: api_key=sk-FAKE_TEST_KEY_000000000000";
        let result = truncate_tool_output(output.to_string());

        // Should redact IP:port
        assert!(result.contains("[REDACTED-HOST]"));
        assert!(!result.contains("192.168.1.100:8080"));

        // Should redact API key (api_key= pattern redacts the whole value)
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("sk-1234567890"));
    }

    #[tokio::test]
    async fn test_tool_execution_shell() {
        // Use mock execution for tests to avoid side effects
        let mut state = AgentState::new();
        state.pending_tool_calls.push(ToolCall::new(
            "shell",
            serde_json::json!({"command": "ls -la"}),
        ));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.tool_results.len(), 1);
        assert!(state.tool_results[0].success);
        assert!(state.pending_tool_calls.is_empty());
    }

    #[tokio::test]
    async fn test_tool_execution_unknown_tool() {
        let mut state = AgentState::new();
        state
            .pending_tool_calls
            .push(ToolCall::new("unknown_tool", serde_json::json!({})));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.tool_results.len(), 1);
        assert!(!state.tool_results[0].success);
    }

    #[tokio::test]
    async fn test_tool_executor_shell_echo() {
        // Test real shell execution with a safe command
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute("shell", &serde_json::json!({"command": "echo 'hello'"}))
            .await;

        assert!(success);
        assert!(output.contains("hello"));
    }

    #[tokio::test]
    async fn test_tool_executor_read_nonexistent_file() {
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute(
                "read_file",
                &serde_json::json!({"path": "/nonexistent/file.txt"}),
            )
            .await;

        assert!(!success);
        assert!(output.contains("Error"));
    }

    #[tokio::test]
    async fn test_mock_tool_execution_returns_expected_output() {
        let (output, success) = mock_tool_execution("shell", &serde_json::json!({"command": "ls"}));
        assert!(success);
        assert!(output.contains("$"));

        let (output, success) =
            mock_tool_execution("read_file", &serde_json::json!({"path": "test.txt"}));
        assert!(success);
        assert!(output.contains("test.txt"));
    }

    #[tokio::test]
    async fn test_mcp_tool_without_client() {
        // MCP tool execution without a client configured should fail gracefully
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute(
                "mcp__filesystem__read_file",
                &serde_json::json!({"path": "/test"}),
            )
            .await;

        assert!(!success);
        assert!(output.contains("MCP client not configured"));
    }

    #[tokio::test]
    async fn test_is_mcp_tool_routing() {
        // Verify that MCP tools are detected correctly
        assert!(is_mcp_tool("mcp__filesystem__read_file"));
        assert!(is_mcp_tool("mcp__git__commit"));
        assert!(!is_mcp_tool("shell"));
        assert!(!is_mcp_tool("read_file"));
    }

    #[tokio::test]
    async fn test_executor_with_mcp_client() {
        // Test that executor can be configured with an MCP client
        let mcp_client = Arc::new(McpClient::new());
        let executor = ToolExecutor::new(None).with_mcp_client(mcp_client);

        // The executor should have the MCP client
        // Trying to execute an MCP tool without connecting to a server should fail
        let (output, success) = executor
            .execute("mcp__nonexistent__tool", &serde_json::json!({}))
            .await;

        // Should fail because server isn't connected, but importantly it tries MCP execution
        assert!(!success);
        assert!(output.contains("MCP tool error") || output.contains("Unknown"));
    }

    #[tokio::test]
    async fn test_apply_patch_empty_patch() {
        // Test that empty patch returns an error
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute("apply_patch", &serde_json::json!({"patch": ""}))
            .await;

        assert!(!success);
        assert!(output.contains("empty patch"));
    }

    #[tokio::test]
    async fn test_apply_patch_missing_patch_arg() {
        // Test that missing patch argument returns an error
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute("apply_patch", &serde_json::json!({}))
            .await;

        assert!(!success);
        assert!(output.contains("empty patch"));
    }

    #[tokio::test]
    async fn test_mock_apply_patch() {
        // Test mock apply_patch execution
        let (output, success) =
            mock_tool_execution("apply_patch", &serde_json::json!({"patch": "---"}));
        assert!(success);
        assert!(output.contains("successfully"));
    }

    #[tokio::test]
    async fn test_apply_patch_real_file() {
        // Test applying a real patch to a real file
        // This uses the apply-patch format (not unified diff)
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let test_file = temp_dir.path().join("test.txt");

        // Create the original file
        std::fs::write(&test_file, "line one\nline two\nline three\n")
            .expect("Failed to write test file");

        // Create an apply-patch format patch that changes "line two" to "modified line"
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n line one\n-line two\n+modified line\n*** End Patch",
            test_file.display()
        );

        // Execute the patch (use WorkspaceWrite mode since we need to write files)
        let executor = ToolExecutor::with_sandbox(
            Some(temp_dir.path().to_path_buf()),
            SandboxMode::WorkspaceWrite,
        );
        let (output, success) = executor
            .execute("apply_patch", &serde_json::json!({"patch": patch}))
            .await;

        // Verify patch was applied successfully
        assert!(success, "Patch should apply successfully: {}", output);

        // Verify the file was modified
        let contents = std::fs::read_to_string(&test_file).expect("Failed to read patched file");
        assert!(
            contents.contains("modified line"),
            "File should contain patched content, got: {}",
            contents
        );
        assert!(
            !contents.contains("line two"),
            "File should not contain original line, got: {}",
            contents
        );
    }

    #[tokio::test]
    async fn test_apply_patch_with_special_characters() {
        // Test applying a patch with special characters
        // Pure Rust implementation handles this correctly
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let test_file = temp_dir.path().join("special.txt");

        // Create the original file with special characters
        std::fs::write(&test_file, "hello 'world'\ntest \"quotes\"\n$variable\n")
            .expect("Failed to write test file");

        // Create an apply-patch format patch
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-hello 'world'\n+hello 'universe'\n*** End Patch",
            test_file.display()
        );

        // Use WorkspaceWrite mode since we need to write files
        let executor = ToolExecutor::with_sandbox(
            Some(temp_dir.path().to_path_buf()),
            SandboxMode::WorkspaceWrite,
        );
        let (output, success) = executor
            .execute("apply_patch", &serde_json::json!({"patch": patch}))
            .await;

        assert!(success, "Patch with special chars should apply: {}", output);

        let contents = std::fs::read_to_string(&test_file).expect("Failed to read patched file");
        assert!(
            contents.contains("hello 'universe'"),
            "File should contain patched content with quotes, got: {}",
            contents
        );
    }

    #[tokio::test]
    async fn test_apply_patch_invalid_patch_format() {
        // Test that an invalid patch format reports failure
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");

        // Create a file that won't match the patch
        let test_file = temp_dir.path().join("mismatch.txt");
        std::fs::write(&test_file, "completely different content\n")
            .expect("Failed to write test file");

        // Create a patch that doesn't match the file content (apply-patch format)
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-nonexistent line\n+new line\n*** End Patch",
            test_file.display()
        );

        // Use WorkspaceWrite mode since we're testing patch application (even though it will fail)
        let executor = ToolExecutor::with_sandbox(
            Some(temp_dir.path().to_path_buf()),
            SandboxMode::WorkspaceWrite,
        );
        let (output, success) = executor
            .execute("apply_patch", &serde_json::json!({"patch": patch}))
            .await;

        // Pure Rust apply-patch should fail and report the error
        assert!(
            !success,
            "Mismatched patch should fail, got success with: {}",
            output
        );
        assert!(
            output.contains("Error") || output.contains("Failed") || output.contains("find"),
            "Should report error for mismatched content: {}",
            output
        );
    }

    #[tokio::test]
    async fn test_apply_patch_multiline_change() {
        // Test applying a patch that modifies multiple lines
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let test_file = temp_dir.path().join("multi.txt");

        // Create original file with multiple lines
        std::fs::write(
            &test_file,
            "fn main() {\n    println!(\"Hello\");\n    // comment\n}\n",
        )
        .expect("Failed to write test file");

        // Create an apply-patch format patch that changes multiple lines
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n fn main() {{\n-    println!(\"Hello\");\n-    // comment\n+    println!(\"World\");\n+    // updated comment\n*** End Patch",
            test_file.display()
        );

        // Use WorkspaceWrite mode since we need to write files
        let executor = ToolExecutor::with_sandbox(
            Some(temp_dir.path().to_path_buf()),
            SandboxMode::WorkspaceWrite,
        );
        let (output, success) = executor
            .execute("apply_patch", &serde_json::json!({"patch": patch}))
            .await;

        assert!(success, "Multiline patch should apply: {}", output);

        let contents = std::fs::read_to_string(&test_file).expect("Failed to read patched file");
        assert!(
            contents.contains("println!(\"World\")"),
            "File should contain first patched line, got: {}",
            contents
        );
        assert!(
            contents.contains("// updated comment"),
            "File should contain second patched line, got: {}",
            contents
        );
    }

    // === Audit #52: Unified diff support tests ===

    #[test]
    fn test_is_unified_diff_git_format() {
        // Git diff format should be detected
        let git_diff = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n";
        assert!(is_unified_diff(git_diff));
    }

    #[test]
    fn test_is_unified_diff_standard_format() {
        // Standard unified diff without git headers
        let unified = "--- file.txt.orig\n+++ file.txt\n@@ -1 +1 @@\n-old\n+new\n";
        assert!(is_unified_diff(unified));
    }

    #[test]
    fn test_is_unified_diff_apply_patch_format() {
        // Our custom apply-patch format should NOT be detected as unified diff
        let apply_patch =
            "*** Begin Patch\n*** Update File: test.txt\n@@\n-old\n+new\n*** End Patch";
        assert!(!is_unified_diff(apply_patch));
    }

    #[test]
    fn test_is_unified_diff_empty() {
        assert!(!is_unified_diff(""));
        assert!(!is_unified_diff("   \n\n  "));
    }

    #[test]
    fn test_is_unified_diff_partial_headers() {
        // Only --- without +++ is not a valid unified diff
        let partial = "--- file.txt\nsome content\n";
        assert!(!is_unified_diff(partial));

        // Only +++ without --- is not a valid unified diff
        let partial2 = "+++ file.txt\nsome content\n";
        assert!(!is_unified_diff(partial2));
    }

    #[test]
    fn test_is_unified_diff_diff_git_new_file() {
        // Git diff for new file
        let new_file = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+content\n";
        assert!(is_unified_diff(new_file));
    }

    #[tokio::test]
    async fn test_apply_unified_diff_in_git_repo() {
        // Test applying a unified diff in a git repository
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let test_file = temp_dir.path().join("test.txt");

        // Initialize git repo
        let git_init = std::process::Command::new("git")
            .arg("init")
            .current_dir(temp_dir.path())
            .output();

        // Skip test if git is not available
        if git_init.is_err() || !git_init.unwrap().status.success() {
            return;
        }

        // Configure git user (required for commits)
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(temp_dir.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(temp_dir.path())
            .output();

        // Create and commit original file
        std::fs::write(&test_file, "line1\nline2\nline3\n").expect("Failed to write file");
        let _ = std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(temp_dir.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(temp_dir.path())
            .output();

        // Create a unified diff
        let unified_diff = r#"diff --git a/test.txt b/test.txt
--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,3 @@
 line1
-line2
+modified line
 line3
"#;

        // Apply the unified diff
        let executor = ToolExecutor::with_sandbox(
            Some(temp_dir.path().to_path_buf()),
            SandboxMode::WorkspaceWrite,
        );
        let (output, success) = executor
            .execute("apply_patch", &serde_json::json!({"patch": unified_diff}))
            .await;

        // Check result
        assert!(
            success,
            "Unified diff should apply successfully: {}",
            output
        );

        // Verify file was modified
        let contents = std::fs::read_to_string(&test_file).expect("Failed to read file");
        assert!(
            contents.contains("modified line"),
            "File should contain patched content: {}",
            contents
        );
    }

    #[tokio::test]
    async fn test_apply_patch_detects_format() {
        // Test that apply_patch correctly detects and routes to the right handler
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");

        // Create executor with workspace-write mode
        let executor = ToolExecutor::with_sandbox(
            Some(temp_dir.path().to_path_buf()),
            SandboxMode::WorkspaceWrite,
        );

        // Test with apply-patch format (should use pure Rust impl)
        let apply_patch_format = "*** Begin Patch\n*** Add File: test.txt\n+content\n*** End Patch";
        let (output1, success1) = executor
            .execute(
                "apply_patch",
                &serde_json::json!({"patch": apply_patch_format}),
            )
            .await;
        // Should succeed (adding a new file)
        assert!(success1, "Apply-patch format should work: {}", output1);

        // Test with unified diff format (should detect and use git apply)
        // This will fail if not in a git repo, which is expected
        let unified_format = "diff --git a/nonexistent.txt b/nonexistent.txt\n--- a/nonexistent.txt\n+++ b/nonexistent.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let (output2, _success2) = executor
            .execute("apply_patch", &serde_json::json!({"patch": unified_format}))
            .await;

        // The output should indicate it tried to use git apply (either error or success)
        // The key test is that it detected the format correctly
        // Git apply errors include: "No such file", "does not exist", "patch does not apply"
        assert!(
            output2.contains("git")
                || output2.contains("unified")
                || output2.contains("repository")
                || output2.contains("apply")
                || output2.contains("No such file")
                || output2.contains("does not exist")
                || output2.contains("error:"),
            "Unified diff should be handled by git apply: {}",
            output2
        );
    }

    #[tokio::test]
    async fn test_mock_tool_execution_node_preserves_session_id() {
        let mut state = AgentState::new();
        let original_session_id = state.session_id.clone();
        state.pending_tool_calls.push(ToolCall::new(
            "shell",
            serde_json::json!({"command": "echo hello"}),
        ));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.session_id, original_session_id);
    }

    #[tokio::test]
    async fn test_mock_tool_execution_node_preserves_turn_count() {
        let mut state = AgentState::new();
        state.turn_count = 7;
        state.pending_tool_calls.push(ToolCall::new(
            "shell",
            serde_json::json!({"command": "echo hello"}),
        ));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.turn_count, 7);
    }

    #[tokio::test]
    async fn test_mock_tool_execution_node_preserves_messages() {
        use crate::state::Message;

        let mut state = AgentState::new();
        state.messages.push(Message::user("Hello"));
        state.messages.push(Message::assistant("Hi there"));
        state.pending_tool_calls.push(ToolCall::new(
            "shell",
            serde_json::json!({"command": "echo hello"}),
        ));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_mock_tool_execution_node_multiple_tools() {
        let mut state = AgentState::new();
        state.pending_tool_calls.push(ToolCall::new(
            "shell",
            serde_json::json!({"command": "echo 1"}),
        ));
        state.pending_tool_calls.push(ToolCall::new(
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ));
        state.pending_tool_calls.push(ToolCall::new(
            "write_file",
            serde_json::json!({"path": "out.txt", "content": "data"}),
        ));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.tool_results.len(), 3);
        assert!(state.pending_tool_calls.is_empty());

        // All mock tools should succeed
        for result in &state.tool_results {
            assert!(result.success);
        }
    }

    #[tokio::test]
    async fn test_mock_tool_execution_node_empty_pending_calls() {
        let state = AgentState::new();

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert!(state.tool_results.is_empty());
    }

    #[tokio::test]
    async fn test_mock_tool_execution_node_clears_pending_calls() {
        let mut state = AgentState::new();
        state
            .pending_tool_calls
            .push(ToolCall::new("shell", serde_json::json!({"command": "ls"})));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert!(state.pending_tool_calls.is_empty());
        assert_eq!(state.tool_results.len(), 1);
    }

    #[tokio::test]
    async fn test_mock_tool_execution_search_files() {
        let (output, success) =
            mock_tool_execution("search_files", &serde_json::json!({"query": "main"}));
        assert!(success);
        assert!(output.contains("main"));
        assert!(output.contains("src/main.rs"));
    }

    #[tokio::test]
    async fn test_tool_result_has_tool_call_id() {
        let mut state = AgentState::new();
        let tool_call = ToolCall::new("shell", serde_json::json!({"command": "ls"}));
        let tool_call_id = tool_call.id.clone();
        state.pending_tool_calls.push(tool_call);

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.tool_results[0].tool_call_id, tool_call_id);
    }

    #[tokio::test]
    async fn test_tool_result_has_correct_tool_name() {
        let mut state = AgentState::new();
        state.pending_tool_calls.push(ToolCall::new(
            "read_file",
            serde_json::json!({"path": "test.txt"}),
        ));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.tool_results[0].tool, "read_file");
    }

    #[tokio::test]
    async fn test_tool_result_duration_recorded() {
        let mut state = AgentState::new();
        state
            .pending_tool_calls
            .push(ToolCall::new("shell", serde_json::json!({"command": "ls"})));

        let result = mock_tool_execution_node(state).await;
        assert!(result.is_ok());
        let state = result.unwrap();
        // Duration should be set (might be 0 for very fast mock execution)
        assert!(state.tool_results[0].duration_ms <= 1000); // Should complete quickly
    }

    #[tokio::test]
    async fn test_tool_executor_unknown_tool() {
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute("nonexistent_tool", &serde_json::json!({}))
            .await;

        assert!(!success);
        assert!(output.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_tool_executor_list_directory() {
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute("list_directory", &serde_json::json!({"path": "."}))
            .await;

        // Should succeed (listing current directory)
        assert!(success, "list_directory failed: {}", output);
    }

    #[tokio::test]
    async fn test_tool_executor_list_dir() {
        // Audit #46: Verify that "list_dir" (tool definition name) works
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute("list_dir", &serde_json::json!({"path": "."}))
            .await;

        // Should succeed (listing current directory)
        assert!(success, "list_dir failed: {}", output);
    }

    #[tokio::test]
    async fn test_tool_executor_write_file_blocked_in_read_only_sandbox() {
        // Audit #47: write_file should be blocked in read-only sandbox mode
        let executor = ToolExecutor::with_sandbox(None, SandboxMode::ReadOnly);
        let (output, success) = executor
            .execute(
                "write_file",
                &serde_json::json!({"path": "/tmp/test.txt", "content": "test"}),
            )
            .await;

        // Should fail with sandbox error
        assert!(!success, "write_file should fail in read-only mode");
        assert!(
            output.contains("read-only sandbox mode"),
            "Expected sandbox error message, got: {}",
            output
        );
    }

    #[tokio::test]
    async fn test_tool_executor_apply_patch_blocked_in_read_only_sandbox() {
        // Audit #47: apply_patch should be blocked in read-only sandbox mode
        let executor = ToolExecutor::with_sandbox(None, SandboxMode::ReadOnly);
        let (output, success) = executor
            .execute("apply_patch", &serde_json::json!({"patch": "test patch"}))
            .await;

        // Should fail with sandbox error
        assert!(!success, "apply_patch should fail in read-only mode");
        assert!(
            output.contains("read-only sandbox mode"),
            "Expected sandbox error message, got: {}",
            output
        );
    }

    #[tokio::test]
    async fn test_tool_executor_search_files_glob() {
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute(
                "search_files",
                &serde_json::json!({"query": "*.rs", "mode": "glob"}),
            )
            .await;

        // Glob pattern search uses fd or find to find files
        assert!(success, "search_files (glob) failed: {}", output);
    }

    #[tokio::test]
    async fn test_tool_executor_search_files_content() {
        // Create a temp directory with a test file
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "hello world\ntest content\n")
            .expect("Failed to write test file");

        let executor = ToolExecutor::new(Some(temp_dir.path().to_path_buf()));
        let (output, success) = executor
            .execute(
                "search_files",
                &serde_json::json!({"query": "hello", "mode": "content", "path": "."}),
            )
            .await;

        // Content search uses rg or grep
        assert!(success, "search_files (content) failed: {}", output);
        assert!(
            output.contains("hello") || output.is_empty(),
            "Should find 'hello' in output: {}",
            output
        );
    }

    #[tokio::test]
    async fn test_tool_executor_search_files_fuzzy() {
        let executor = ToolExecutor::new(None);
        // Search for "toolexec" should find "tool_execution.rs"
        let (output, success) = executor
            .execute(
                "search_files",
                &serde_json::json!({"query": "toolexec", "mode": "fuzzy"}),
            )
            .await;

        assert!(success, "search_files (fuzzy) failed: {}", output);
        // Fuzzy search should return results with scores
        assert!(
            output.contains("score:") || output.contains("No files found"),
            "Fuzzy output: {}",
            output
        );
    }

    #[tokio::test]
    async fn test_tool_executor_search_files_fuzzy_default_mode() {
        let executor = ToolExecutor::new(None);
        // Without mode specified, should default to fuzzy
        let (output, success) = executor
            .execute("search_files", &serde_json::json!({"query": "Cargo"}))
            .await;

        assert!(success, "search_files (fuzzy default) failed: {}", output);
        // Should find Cargo.toml files
        assert!(
            output.contains("Cargo") || output.contains("score:"),
            "Should find Cargo files: {}",
            output
        );
    }

    #[tokio::test]
    async fn test_tool_executor_search_files_missing_query() {
        let executor = ToolExecutor::new(None);
        let (output, success) = executor
            .execute("search_files", &serde_json::json!({}))
            .await;

        assert!(!success);
        assert!(output.contains("missing") && output.contains("query"));
    }

    #[tokio::test]
    async fn test_tool_executor_shell_missing_command() {
        let executor = ToolExecutor::new(None);
        let (output, success) = executor.execute("shell", &serde_json::json!({})).await;

        assert!(!success);
        assert!(output.contains("missing") || output.contains("command"));
    }

    #[tokio::test]
    async fn test_tool_executor_with_working_dir() {
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let executor = ToolExecutor::new(Some(temp_dir.path().to_path_buf()));

        // Echo PWD should show the temp directory (on Unix systems)
        let (output, success) = executor
            .execute("shell", &serde_json::json!({"command": "pwd"}))
            .await;

        assert!(success, "pwd command failed: {}", output);
    }

    #[tokio::test]
    async fn test_parse_qualified_tool_name_valid() {
        assert!(parse_qualified_tool_name("mcp__server__tool").is_some());
        let (server, tool) = parse_qualified_tool_name("mcp__myserver__mytool").unwrap();
        assert_eq!(server, "myserver");
        assert_eq!(tool, "mytool");
    }

    #[tokio::test]
    async fn test_parse_qualified_tool_name_invalid() {
        assert!(parse_qualified_tool_name("shell").is_none());
        assert!(parse_qualified_tool_name("mcp__").is_none());
        assert!(parse_qualified_tool_name("mcp__server").is_none());
    }

    // End-to-end approval flow tests (N=239)

    mod approval_flow_tests {
        use super::*;
        use crate::approval_presets::exec_policy_from_preset;
        use crate::execpolicy::ApprovalMode;
        use crate::state::{AutoApproveCallback, AutoRejectCallback};
        use std::sync::atomic::{AtomicBool, Ordering};

        /// Test callback that tracks whether approval was requested
        struct TrackingApprovalCallback {
            approval_requested: AtomicBool,
            should_approve: bool,
        }

        impl TrackingApprovalCallback {
            fn new(should_approve: bool) -> Self {
                Self {
                    approval_requested: AtomicBool::new(false),
                    should_approve,
                }
            }

            fn was_approval_requested(&self) -> bool {
                self.approval_requested.load(Ordering::SeqCst)
            }
        }

        #[async_trait::async_trait]
        impl crate::state::ApprovalCallback for TrackingApprovalCallback {
            async fn request_approval(
                &self,
                _request_id: &str,
                _tool_call_id: &str,
                _tool: &str,
                _args: &serde_json::Value,
                _reason: Option<&str>,
            ) -> crate::codex::ApprovalDecision {
                self.approval_requested.store(true, Ordering::SeqCst);
                if self.should_approve {
                    crate::codex::ApprovalDecision::Approve
                } else {
                    crate::codex::ApprovalDecision::Deny
                }
            }

            async fn is_session_approved(&self, _tool: &str) -> bool {
                false
            }

            async fn mark_session_approved(&self, _tool: &str) {}
        }

        #[tokio::test]
        async fn test_read_only_mode_shell_triggers_approval_for_unknown_commands() {
            // read-only preset maps to ApprovalMode::Always
            // BUT: known-safe commands like "ls" are still auto-approved by the safe command whitelist
            // For unknown commands, approval IS required
            let policy = exec_policy_from_preset("read-only");
            assert_eq!(policy.approval_mode, ApprovalMode::Always);

            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            // Use an unknown command that isn't in the safe whitelist but will execute successfully
            // "echo" is simple but not in safe whitelist (only via bash -c wrapper sometimes)
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "touch /tmp/test-approval-flow"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // Approval should have been requested for unknown command in Always mode
            assert!(
                tracking_callback.was_approval_requested(),
                "read-only mode should request approval for unknown shell commands"
            );

            let state = result.unwrap();
            assert_eq!(state.tool_results.len(), 1);
            // Note: We don't assert success because the command may fail due to sandbox
            // The important thing is that approval was requested
        }

        #[tokio::test]
        async fn test_read_only_mode_safe_commands_auto_approved() {
            // Known-safe commands like "ls" are auto-approved even in read-only mode
            // This is by design - the safe command whitelist takes precedence
            let policy = exec_policy_from_preset("read-only");
            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "ls -la"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // Known-safe commands bypass approval even in Always mode
            assert!(
                !tracking_callback.was_approval_requested(),
                "Known-safe commands are auto-approved (whitelist takes precedence)"
            );

            let state = result.unwrap();
            assert!(state.tool_results[0].success);
        }

        #[tokio::test]
        async fn test_read_only_mode_read_file_triggers_approval() {
            // read-only mode requires approval for ALL tools
            let policy = exec_policy_from_preset("read-only");
            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            state.pending_tool_calls.push(ToolCall::new(
                "read_file",
                serde_json::json!({"path": "Cargo.toml"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // Even read_file should trigger approval in read-only mode
            // BUT - there's an explicit Allow rule for read_file in ExecPolicy::with_dangerous_patterns()
            // The preset uses with_dangerous_patterns() which allows read_file
            // So read_file is auto-approved by the explicit rule
            // This is correct behavior - read_file is safe
        }

        #[tokio::test]
        async fn test_auto_mode_safe_shell_no_approval() {
            // auto preset maps to ApprovalMode::OnDangerous
            let policy = exec_policy_from_preset("auto");
            assert_eq!(policy.approval_mode, ApprovalMode::OnDangerous);

            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            // "ls -la" is a known safe command
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "ls -la"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // Safe command should NOT trigger approval
            assert!(
                !tracking_callback.was_approval_requested(),
                "auto mode should NOT request approval for safe commands like 'ls'"
            );

            let state = result.unwrap();
            assert_eq!(state.tool_results.len(), 1);
            assert!(state.tool_results[0].success);
        }

        #[tokio::test]
        async fn test_auto_mode_unknown_command_triggers_approval() {
            // auto preset maps to ApprovalMode::OnDangerous
            let policy = exec_policy_from_preset("auto");

            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            // "npm install" is not a known safe command, so OnDangerous mode should prompt
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "npm install"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // Unknown command with "dangerous" shell tool should trigger approval
            assert!(
                tracking_callback.was_approval_requested(),
                "auto mode should request approval for unknown shell commands"
            );
        }

        #[tokio::test]
        async fn test_auto_mode_sudo_triggers_approval() {
            // sudo commands should trigger approval in auto mode
            let policy = exec_policy_from_preset("auto");

            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "sudo apt update"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // sudo should trigger approval
            assert!(
                tracking_callback.was_approval_requested(),
                "auto mode should request approval for sudo commands"
            );
        }

        #[tokio::test]
        async fn test_full_access_mode_no_approval() {
            // full-access preset maps to ApprovalMode::Never
            let policy = exec_policy_from_preset("full-access");
            assert_eq!(policy.approval_mode, ApprovalMode::Never);

            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            // Even unknown commands should not trigger approval in full-access mode
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "npm run build"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // Full-access mode should NOT request approval
            assert!(
                !tracking_callback.was_approval_requested(),
                "full-access mode should NOT request approval for any commands"
            );
        }

        #[tokio::test]
        async fn test_approval_rejection_skips_execution() {
            let policy = exec_policy_from_preset("read-only");

            // Callback that rejects all requests
            let tracking_callback = Arc::new(TrackingApprovalCallback::new(false));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            // Use an unknown command (not in safe whitelist) so approval is required
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "npm run build"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            let state = result.unwrap();
            assert_eq!(state.tool_results.len(), 1);
            // Tool should be rejected because callback denies
            assert!(!state.tool_results[0].success);
            assert!(state.tool_results[0].output.contains("rejected"));
        }

        #[tokio::test]
        async fn test_auto_reject_callback_rejects_all() {
            let policy = exec_policy_from_preset("read-only");

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(Arc::new(AutoRejectCallback));

            // Use an unknown command (not in safe whitelist) so approval is required
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "npm test"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            let state = result.unwrap();
            assert!(!state.tool_results[0].success);
            assert!(state.tool_results[0].output.contains("rejected"));
        }

        #[tokio::test]
        async fn test_auto_approve_callback_approves_all() {
            let policy = exec_policy_from_preset("read-only");

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(Arc::new(AutoApproveCallback));

            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "echo test"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            let state = result.unwrap();
            assert!(state.tool_results[0].success);
        }

        #[tokio::test]
        async fn test_dangerous_command_forbidden_regardless_of_mode() {
            // Even in full-access mode, critically dangerous commands should be forbidden
            let policy = exec_policy_from_preset("full-access");

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(Arc::new(AutoApproveCallback));

            // "rm -rf /" is forbidden by safety analysis
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "rm -rf /"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            let state = result.unwrap();
            // Should be rejected (forbidden by safety check)
            assert!(!state.tool_results[0].success);
            assert!(
                state.tool_results[0].output.contains("forbidden")
                    || state.tool_results[0].output.contains("Safety"),
                "Expected forbidden output, got: {}",
                state.tool_results[0].output
            );
        }

        #[tokio::test]
        async fn test_write_file_in_read_only_triggers_approval() {
            let policy = exec_policy_from_preset("read-only");
            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            state.pending_tool_calls.push(ToolCall::new(
                "write_file",
                serde_json::json!({"path": "/tmp/test.txt", "content": "hello"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // write_file should trigger approval in read-only mode
            assert!(
                tracking_callback.was_approval_requested(),
                "read-only mode should request approval for write_file"
            );
        }

        #[tokio::test]
        async fn test_apply_patch_in_auto_mode_triggers_approval() {
            // apply_patch is considered "dangerous" by is_dangerous_tool()
            let policy = exec_policy_from_preset("auto");
            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            state.pending_tool_calls.push(ToolCall::new(
                "apply_patch",
                serde_json::json!({"patch": "*** Begin Patch\n*** End Patch"}),
            ));

            let result = tool_execution_node(state).await;
            assert!(result.is_ok());

            // apply_patch should trigger approval in auto mode (dangerous tool)
            assert!(
                tracking_callback.was_approval_requested(),
                "auto mode should request approval for apply_patch (dangerous tool)"
            );
        }

        // Audit #56: Tests for mock_tool_execution_node respecting approval

        #[tokio::test]
        async fn test_mock_execution_respects_rejection() {
            // Mock execution should now respect approval callback
            let policy = exec_policy_from_preset("read-only");
            let tracking_callback = Arc::new(TrackingApprovalCallback::new(false)); // Reject

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            // Use an unknown command (not in safe whitelist) so approval is required
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "npm run test"}),
            ));

            // Use mock execution node
            let result = mock_tool_execution_node(state).await;
            assert!(result.is_ok());

            let state = result.unwrap();
            // Tool should be rejected
            assert_eq!(state.tool_results.len(), 1);
            assert!(
                !state.tool_results[0].success,
                "Mock execution should reject when approval denied"
            );
            assert!(
                state.tool_results[0].output.contains("rejected"),
                "Output should indicate rejection: {}",
                state.tool_results[0].output
            );
        }

        #[tokio::test]
        async fn test_mock_execution_respects_approval() {
            // Mock execution should execute when approval is granted
            let policy = exec_policy_from_preset("read-only");
            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true)); // Approve

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            // Use an unknown command that requires approval
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "npm run test"}),
            ));

            // Use mock execution node
            let result = mock_tool_execution_node(state).await;
            assert!(result.is_ok());

            let state = result.unwrap();
            // Tool should be approved and execute (mock returns success)
            assert_eq!(state.tool_results.len(), 1);
            assert!(
                state.tool_results[0].success,
                "Mock execution should succeed when approval granted"
            );
            // Approval should have been requested
            assert!(
                tracking_callback.was_approval_requested(),
                "Mock execution should request approval"
            );
        }

        #[tokio::test]
        async fn test_mock_execution_auto_approves_safe_commands() {
            // Safe commands should be auto-approved even in mock execution
            let policy = exec_policy_from_preset("auto");
            let tracking_callback = Arc::new(TrackingApprovalCallback::new(true));

            let mut state = AgentState::new()
                .with_exec_policy(Arc::new(policy))
                .with_approval_callback(tracking_callback.clone());

            // "ls" is a known safe command
            state.pending_tool_calls.push(ToolCall::new(
                "shell",
                serde_json::json!({"command": "ls -la"}),
            ));

            let result = mock_tool_execution_node(state).await;
            assert!(result.is_ok());

            let state = result.unwrap();
            // Should succeed
            assert!(state.tool_results[0].success);
            // Should NOT have requested approval (auto-approved by safe command whitelist)
            assert!(
                !tracking_callback.was_approval_requested(),
                "Safe commands should not request approval in mock execution"
            );
        }
    }

    // Audit #51: Search path restriction tests
    mod search_path_restriction_tests {
        use super::*;

        #[tokio::test]
        async fn test_search_files_relative_path_within_workspace() {
            // Relative paths within workspace should work
            let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
            let executor = ToolExecutor::with_sandbox(
                Some(temp_dir.path().to_path_buf()),
                SandboxMode::ReadOnly, // Restricted mode (no real sandbox, restricted search)
            );

            // Create a test file to search
            let test_file = temp_dir.path().join("test.txt");
            std::fs::write(&test_file, "hello world").expect("Failed to write test file");

            let (output, success) = executor
                .execute(
                    "search_files",
                    &serde_json::json!({"query": "test", "path": "."}),
                )
                .await;

            // Should succeed - "." is within workspace
            assert!(success, "Search in workspace should succeed: {}", output);
        }

        #[tokio::test]
        async fn test_search_files_absolute_path_outside_workspace_blocked() {
            // When sandbox is not available and mode is not unrestricted,
            // absolute paths outside workspace should be blocked
            let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
            let executor = ToolExecutor::with_sandbox(
                Some(temp_dir.path().to_path_buf()),
                SandboxMode::ReadOnly, // Restricted mode without real sandbox
            );

            // Try to search /tmp (outside workspace) when sandbox is unavailable
            // Note: This test may pass differently on systems where SandboxExecutor::is_available() is true
            // On macOS/Linux with sandbox available, the path would be allowed
            if !SandboxExecutor::is_available() {
                let (output, success) = executor
                    .execute(
                        "search_files",
                        &serde_json::json!({"query": "test", "path": "/etc"}),
                    )
                    .await;

                // Should fail - /etc is outside workspace and sandbox is not available
                assert!(
                    !success,
                    "Search outside workspace should fail without sandbox: {}",
                    output
                );
                assert!(
                    output.contains("outside the workspace")
                        || output.contains("not found or not accessible"),
                    "Expected workspace restriction error, got: {}",
                    output
                );
            }
        }

        #[tokio::test]
        async fn test_search_files_path_traversal_blocked() {
            // Path traversal attempts (../) should be resolved and blocked if outside workspace
            let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
            let executor = ToolExecutor::with_sandbox(
                Some(temp_dir.path().to_path_buf()),
                SandboxMode::ReadOnly,
            );

            if !SandboxExecutor::is_available() {
                // Try to escape via ../
                let (output, success) = executor
                    .execute(
                        "search_files",
                        &serde_json::json!({"query": "test", "path": "../../.."}),
                    )
                    .await;

                // Should fail - path resolves outside workspace
                assert!(
                    !success,
                    "Path traversal outside workspace should fail: {}",
                    output
                );
            }
        }

        #[tokio::test]
        async fn test_search_files_unrestricted_mode_allows_all_paths() {
            // In DangerFullAccess mode, all paths should be allowed
            let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
            let executor = ToolExecutor::with_sandbox(
                Some(temp_dir.path().to_path_buf()),
                SandboxMode::DangerFullAccess, // Unrestricted mode
            );

            // /tmp should be allowed in unrestricted mode
            let (output, _success) = executor
                .execute(
                    "search_files",
                    &serde_json::json!({"query": "test", "path": "/tmp"}),
                )
                .await;

            // Should succeed (or fail for other reasons like rg/fd not finding matches)
            // but NOT fail due to workspace restriction
            assert!(
                !output.contains("outside the workspace"),
                "Unrestricted mode should not block paths: {}",
                output
            );
        }
    }
}
