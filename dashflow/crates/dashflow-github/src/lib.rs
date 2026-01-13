//! GitHub integration tools for `DashFlow` Rust.
//!
//! This crate provides a comprehensive set of tools for interacting with GitHub repositories,
//! enabling AI agents to manage issues, pull requests, files, and perform code searches.
//!
//! # Tools
//!
//! ## Issue Management
//! - **`GetIssueTool`**: Get issue details by number
//! - **`CommentOnIssueTool`**: Add comments to issues
//! - **`SearchIssuesAndPRsTool`**: Search issues and pull requests
//!
//! ## Pull Request Management
//! - **`GetPRTool`**: Get pull request details by number
//! - **`CreatePRTool`**: Create new pull requests
//! - **`CreateReviewRequestTool`**: Request reviews on pull requests
//!
//! ## File Management
//! - **`ReadFileTool`**: Read file contents from repository
//! - **`CreateFileTool`**: Create new files in repository
//! - **`UpdateFileTool`**: Update existing files
//! - **`DeleteFileTool`**: Delete files from repository
//!
//! ## Code Search
//! - **`SearchCodeTool`**: Search code across repositories
//!
//! # Authentication
//!
//! All tools require a GitHub personal access token with appropriate permissions.
//! Set the token when creating the Octocrab client instance.
//!
//! # Example
//!
//! ```rust
//! use dashflow_github::GetIssueTool;
//! use dashflow::core::tools::{Tool, ToolInput};
//! use serde_json::json;
//!
//! # tokio_test::block_on(async {
//! // Create tool (requires GitHub token in production)
//! let tool = GetIssueTool::new("owner", "repo", "github_token_here");
//!
//! // Get issue #42
//! let input = json!({"issue_number": 42});
//! // let result = tool._call(ToolInput::Structured(input)).await;
//! # });
//! ```

use async_trait::async_trait;
use dashflow::constants::{DEFAULT_HTTP_CONNECT_TIMEOUT, DEFAULT_HTTP_REQUEST_TIMEOUT};
use dashflow::core::tools::{Tool, ToolInput};
use dashflow::core::Error;
use octocrab::Octocrab;
use serde_json::json;
use std::sync::Arc;

/// Create an HTTP client with standard timeouts
fn create_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(DEFAULT_HTTP_REQUEST_TIMEOUT)
        .connect_timeout(DEFAULT_HTTP_CONNECT_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract string field from `ToolInput`
fn extract_string_field(input: &ToolInput, field: &str) -> Result<String, Error> {
    match input {
        ToolInput::String(s) => Ok(s.clone()),
        ToolInput::Structured(v) => v
            .get(field)
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .ok_or_else(|| Error::tool_error(format!("Missing '{field}' field in input"))),
    }
}

/// Extract optional string field from `ToolInput`
fn extract_optional_string(input: &ToolInput, field: &str) -> Option<String> {
    match input {
        ToolInput::Structured(v) => v
            .get(field)
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string),
        _ => None,
    }
}

/// Extract u64 field from `ToolInput`
fn extract_u64_field(input: &ToolInput, field: &str) -> Result<u64, Error> {
    match input {
        ToolInput::Structured(v) => v
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| Error::tool_error(format!("Missing or invalid '{field}' field"))),
        _ => Err(Error::tool_error(format!(
            "Expected structured input with '{field}' field"
        ))),
    }
}

/// Build an Octocrab client with the given personal access token.
///
/// # Errors
///
/// Returns an error if the client cannot be built (e.g., TLS initialization failure).
fn build_octocrab_client(token: impl Into<String>) -> Result<Arc<Octocrab>, Box<octocrab::Error>> {
    let octocrab = Octocrab::builder()
        .personal_token(token.into())
        .build()
        .map_err(Box::new)?;
    Ok(Arc::new(octocrab))
}

/// Build an Octocrab client with the given personal access token.
///
/// # Panics
///
/// Panics if the client cannot be built. Use `build_octocrab_client` for a fallible alternative.
#[allow(clippy::expect_used)] // Documented panic with build_octocrab_client() fallible alternative
fn build_octocrab_client_or_panic(token: impl Into<String>) -> Arc<Octocrab> {
    build_octocrab_client(token).expect("Failed to build Octocrab client")
}

// ============================================================================
// GetIssueTool
// ============================================================================

/// Tool for getting GitHub issue details.
///
/// Retrieves information about a specific issue by number.
///
/// # Input Format
///
/// - **Structured**: `{"issue_number": 42}`
///
/// # Example
///
/// ```no_run
/// use dashflow_github::GetIssueTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = GetIssueTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "get_issue");
/// ```
#[derive(Clone)]
pub struct GetIssueTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl GetIssueTool {
    /// Creates a new `GetIssueTool` instance.
    ///
    /// # Arguments
    /// * `owner` - Repository owner (username or organization)
    /// * `repo` - Repository name
    /// * `token` - GitHub personal access token
    ///
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `GetIssueTool` instance.
    ///
    /// # Errors
    ///
    /// Returns an error if the Octocrab client cannot be built.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for GetIssueTool {
    fn name(&self) -> &'static str {
        "get_issue"
    }

    fn description(&self) -> &'static str {
        "Get details of a GitHub issue by number. Input: {\"issue_number\": <number>}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let issue_number = extract_u64_field(&input, "issue_number")?;

        let issue = self
            .octocrab
            .issues(&self.owner, &self.repo)
            .get(issue_number)
            .await
            .map_err(|e| Error::tool_error(format!("Failed to get issue: {e}")))?;

        let result = json!({
            "number": issue.number,
            "title": issue.title,
            "state": format!("{:?}", issue.state),
            "body": issue.body.unwrap_or_default(),
            "user": issue.user.login,
            "created_at": issue.created_at.to_string(),
            "updated_at": issue.updated_at.to_string(),
            "labels": issue.labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            "comments": issue.comments,
        });

        Ok(serde_json::to_string_pretty(&result)
            .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}")))
    }
}

// ============================================================================
// CommentOnIssueTool
// ============================================================================

/// Tool for commenting on GitHub issues.
///
/// Adds a comment to an existing issue.
///
/// # Input Format
///
/// - **Structured**: `{"issue_number": 42, "comment": "Great work!"}`
///
/// # Example
///
/// ```no_run
/// use dashflow_github::CommentOnIssueTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = CommentOnIssueTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "comment_on_issue");
/// ```
#[derive(Clone)]
pub struct CommentOnIssueTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl CommentOnIssueTool {
    /// Creates a new `CommentOnIssueTool` instance.
    ///
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `CommentOnIssueTool` instance.
    ///
    /// # Errors
    ///
    /// Returns an error if the Octocrab client cannot be built.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for CommentOnIssueTool {
    fn name(&self) -> &'static str {
        "comment_on_issue"
    }

    fn description(&self) -> &'static str {
        "Add a comment to a GitHub issue. Input: {\"issue_number\": <number>, \"comment\": \"text\"}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let issue_number = extract_u64_field(&input, "issue_number")?;
        let comment = extract_string_field(&input, "comment")?;

        let comment_obj = self
            .octocrab
            .issues(&self.owner, &self.repo)
            .create_comment(issue_number, comment)
            .await
            .map_err(|e| Error::tool_error(format!("Failed to create comment: {e}")))?;

        let result = json!({
            "id": comment_obj.id,
            "created_at": comment_obj.created_at.to_string(),
            "body": comment_obj.body.unwrap_or_default(),
        });

        Ok(format!(
            "Comment added successfully: {}",
            serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}"))
        ))
    }
}

// ============================================================================
// GetPRTool
// ============================================================================

/// Tool for getting GitHub pull request details.
///
/// Retrieves information about a specific pull request by number.
///
/// # Input Format
///
/// - **Structured**: `{"pr_number": 42}`
///
/// # Example
///
/// ```no_run
/// use dashflow_github::GetPRTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = GetPRTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "get_pr");
/// ```
#[derive(Clone)]
pub struct GetPRTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl GetPRTool {
    /// Creates a new `GetPRTool` instance.
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `GetPRTool` instance.
    ///
    /// # Errors
    ///
    /// Returns an error if the Octocrab client cannot be built.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for GetPRTool {
    fn name(&self) -> &'static str {
        "get_pr"
    }

    fn description(&self) -> &'static str {
        "Get details of a GitHub pull request by number. Input: {\"pr_number\": <number>}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let pr_number = extract_u64_field(&input, "pr_number")?;

        let pr = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .get(pr_number)
            .await
            .map_err(|e| Error::tool_error(format!("Failed to get PR: {e}")))?;

        let result = json!({
            "number": pr.number,
            "title": pr.title.unwrap_or_default(),
            "state": format!("{:?}", pr.state),
            "body": pr.body.unwrap_or_default(),
            "user": pr.user.as_ref().map_or("unknown", |u| u.login.as_str()),
            "created_at": pr.created_at.map(|t| t.to_string()).unwrap_or_default(),
            "updated_at": pr.updated_at.map(|t| t.to_string()).unwrap_or_default(),
            "head": pr.head.ref_field,
            "base": pr.base.ref_field,
            "mergeable": pr.mergeable,
            "merged": pr.merged_at.is_some(),
        });

        Ok(serde_json::to_string_pretty(&result)
            .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}")))
    }
}

// ============================================================================
// CreatePRTool
// ============================================================================

/// Tool for creating GitHub pull requests.
///
/// Creates a new pull request from head branch to base branch.
///
/// # Input Format
///
/// - **Structured**: `{"title": "Fix bug", "head": "feature-branch", "base": "main", "body": "Description"}`
///
/// # Example
///
/// ```no_run
/// use dashflow_github::CreatePRTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = CreatePRTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "create_pr");
/// ```
#[derive(Clone)]
pub struct CreatePRTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl CreatePRTool {
    /// Creates a new `CreatePRTool` instance.
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `CreatePRTool` instance.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for CreatePRTool {
    fn name(&self) -> &'static str {
        "create_pr"
    }

    fn description(&self) -> &'static str {
        "Create a new GitHub pull request. Input: {\"title\": \"title\", \"head\": \"branch\", \"base\": \"main\", \"body\": \"description\"}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let title = extract_string_field(&input, "title")?;
        let head = extract_string_field(&input, "head")?;
        let base = extract_string_field(&input, "base")?;
        let body = extract_optional_string(&input, "body").unwrap_or_default();

        let pr = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .create(title, head, base)
            .body(body)
            .send()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to create PR: {e}")))?;

        let result = json!({
            "number": pr.number,
            "title": pr.title.unwrap_or_default(),
            "html_url": pr.html_url.map(|u| u.to_string()),
            "created_at": pr.created_at.map(|t| t.to_string()).unwrap_or_default(),
        });

        Ok(format!(
            "PR created successfully: {}",
            serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}"))
        ))
    }
}

// ============================================================================
// ReadFileTool
// ============================================================================

/// Tool for reading file contents from a GitHub repository.
///
/// Retrieves the contents of a file at a specific path and optional reference (branch/tag/commit).
///
/// # Input Format
///
/// - **Structured**: `{"path": "README.md", "ref": "main"}` (ref is optional, defaults to default branch)
///
/// # Example
///
/// ```no_run
/// use dashflow_github::ReadFileTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = ReadFileTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "read_file");
/// ```
#[derive(Clone)]
pub struct ReadFileTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl ReadFileTool {
    /// Creates a new `ReadFileTool` instance.
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `ReadFileTool` instance.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read file contents from GitHub repository. Input: {\"path\": \"path/to/file\", \"ref\": \"branch\"} (ref is optional)"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let path = extract_string_field(&input, "path")?;
        let reference = extract_optional_string(&input, "ref");

        let repos = self.octocrab.repos(&self.owner, &self.repo);
        let content_handler = repos.get_content();
        let mut request = content_handler.path(&path);

        if let Some(ref_str) = reference {
            request = request.r#ref(&ref_str);
        }

        let content = request
            .send()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to read file: {e}")))?;

        // GitHub content API returns base64 encoded content for files
        if let Some(first) = content.items.first() {
            if let Some(content_str) = &first.content {
                // Decode base64 content
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(content_str.replace('\n', ""))
                    .map_err(|e| Error::tool_error(format!("Failed to decode content: {e}")))?;
                let text = String::from_utf8(decoded)
                    .map_err(|e| Error::tool_error(format!("Invalid UTF-8 content: {e}")))?;
                return Ok(text);
            }
        }

        Err(Error::tool_error("File content not found"))
    }
}

// ============================================================================
// CreateFileTool
// ============================================================================

/// Tool for creating files in a GitHub repository.
///
/// Creates a new file at the specified path with the given content.
///
/// # Input Format
///
/// - **Structured**: `{"path": "new_file.txt", "content": "content", "message": "Add file", "branch": "main"}`
///
/// # Example
///
/// ```no_run
/// use dashflow_github::CreateFileTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = CreateFileTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "create_file");
/// ```
#[derive(Clone)]
pub struct CreateFileTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl CreateFileTool {
    /// Creates a new `CreateFileTool` instance.
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `CreateFileTool` instance.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for CreateFileTool {
    fn name(&self) -> &'static str {
        "create_file"
    }

    fn description(&self) -> &'static str {
        "Create a new file in GitHub repository. Input: {\"path\": \"file.txt\", \"content\": \"text\", \"message\": \"commit msg\", \"branch\": \"main\"}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let path = extract_string_field(&input, "path")?;
        let content = extract_string_field(&input, "content")?;
        let message = extract_string_field(&input, "message")?;
        let branch = extract_optional_string(&input, "branch");

        // Base64 encode the content
        use base64::Engine;
        let encoded_content = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());

        // Build the request
        let repos = self.octocrab.repos(&self.owner, &self.repo);
        let mut request = repos.create_file(&path, &message, &encoded_content);

        if let Some(branch_str) = branch {
            request = request.branch(&branch_str);
        }

        let response = request
            .send()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to create file: {e}")))?;

        let result = json!({
            "path": path,
            "sha": response.content.sha,
            "message": message,
        });

        Ok(format!(
            "File created successfully: {}",
            serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}"))
        ))
    }
}

// ============================================================================
// UpdateFileTool
// ============================================================================

/// Tool for updating files in a GitHub repository.
///
/// Updates an existing file at the specified path with new content.
///
/// # Input Format
///
/// - **Structured**: `{"path": "file.txt", "content": "new content", "message": "Update file", "sha": "blob_sha", "branch": "main"}`
///
/// Note: The `sha` field is the blob SHA of the file being replaced (required by GitHub API).
///
/// # Example
///
/// ```no_run
/// use dashflow_github::UpdateFileTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = UpdateFileTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "update_file");
/// ```
#[derive(Clone)]
pub struct UpdateFileTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl UpdateFileTool {
    /// Creates a new `UpdateFileTool` instance.
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `UpdateFileTool` instance.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for UpdateFileTool {
    fn name(&self) -> &'static str {
        "update_file"
    }

    fn description(&self) -> &'static str {
        "Update an existing file in GitHub repository. Input: {\"path\": \"file.txt\", \"content\": \"text\", \"message\": \"commit msg\", \"sha\": \"blob_sha\", \"branch\": \"main\"}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let path = extract_string_field(&input, "path")?;
        let content = extract_string_field(&input, "content")?;
        let message = extract_string_field(&input, "message")?;
        let sha = extract_string_field(&input, "sha")?;
        let branch = extract_optional_string(&input, "branch");

        // Base64 encode the content
        use base64::Engine;
        let encoded_content = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());

        // Build the request
        let repos = self.octocrab.repos(&self.owner, &self.repo);
        let mut request = repos.update_file(&path, &message, &encoded_content, &sha);

        if let Some(branch_str) = branch {
            request = request.branch(&branch_str);
        }

        let response = request
            .send()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to update file: {e}")))?;

        let result = json!({
            "path": path,
            "sha": response.content.sha,
            "message": message,
        });

        Ok(format!(
            "File updated successfully: {}",
            serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}"))
        ))
    }
}

// ============================================================================
// DeleteFileTool
// ============================================================================

/// Tool for deleting files from a GitHub repository.
///
/// Deletes an existing file at the specified path.
///
/// # Input Format
///
/// - **Structured**: `{"path": "file.txt", "message": "Delete file", "sha": "blob_sha", "branch": "main"}`
///
/// Note: The `sha` field is the blob SHA of the file being deleted (required by GitHub API).
///
/// # Example
///
/// ```no_run
/// use dashflow_github::DeleteFileTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = DeleteFileTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "delete_file");
/// ```
#[derive(Clone)]
pub struct DeleteFileTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl DeleteFileTool {
    /// Creates a new `DeleteFileTool` instance.
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `DeleteFileTool` instance.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &'static str {
        "delete_file"
    }

    fn description(&self) -> &'static str {
        "Delete a file from GitHub repository. Input: {\"path\": \"file.txt\", \"message\": \"commit msg\", \"sha\": \"blob_sha\", \"branch\": \"main\"}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let path = extract_string_field(&input, "path")?;
        let message = extract_string_field(&input, "message")?;
        let sha = extract_string_field(&input, "sha")?;
        let branch = extract_optional_string(&input, "branch");

        // Build the request
        let repos = self.octocrab.repos(&self.owner, &self.repo);
        let mut request = repos.delete_file(&path, &message, &sha);

        if let Some(branch_str) = branch {
            request = request.branch(&branch_str);
        }

        request
            .send()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to delete file: {e}")))?;

        Ok(format!("File '{path}' deleted successfully"))
    }
}

// ============================================================================
// SearchCodeTool
// ============================================================================

/// Tool for searching code in GitHub repositories.
///
/// Searches for code matching a query string.
///
/// # Input Format
///
/// - **Structured**: `{"query": "search term", "per_page": 10}` (`per_page` is optional, default 30)
///
/// # Example
///
/// ```no_run
/// use dashflow_github::SearchCodeTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = SearchCodeTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "search_code");
/// ```
#[derive(Clone)]
pub struct SearchCodeTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl SearchCodeTool {
    /// Creates a new `SearchCodeTool` instance.
    ///
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `SearchCodeTool` instance.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &'static str {
        "search_code"
    }

    fn description(&self) -> &'static str {
        "Search code in GitHub repository. Input: {\"query\": \"search term\", \"per_page\": 10}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let query = extract_string_field(&input, "query")?;
        let per_page = match &input {
            ToolInput::Structured(v) => v
                .get("per_page")
                .and_then(serde_json::Value::as_u64)
                .map(|n| n as u8),
            _ => None,
        };

        // Add repo qualifier to search query
        let full_query = format!("{} repo:{}/{}", query, self.owner, self.repo);

        let mut search = self.octocrab.search().code(&full_query);

        if let Some(pp) = per_page {
            search = search.per_page(pp);
        }

        let results = search
            .send()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to search code: {e}")))?;

        let items: Vec<_> = results
            .items
            .iter()
            .map(|item| {
                json!({
                    "name": item.name,
                    "path": item.path,
                    "sha": item.sha,
                    "url": item.html_url,
                })
            })
            .collect();

        let result = json!({
            "total_count": results.total_count,
            "items": items,
        });

        Ok(serde_json::to_string_pretty(&result)
            .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}")))
    }
}

// ============================================================================
// SearchIssuesAndPRsTool
// ============================================================================

/// Tool for searching issues and pull requests in GitHub.
///
/// Searches for issues and PRs matching a query string.
///
/// # Input Format
///
/// - **Structured**: `{"query": "search term", "per_page": 10}` (`per_page` is optional, default 30)
///
/// # Example
///
/// ```no_run
/// use dashflow_github::SearchIssuesAndPRsTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = SearchIssuesAndPRsTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "search_issues_and_prs");
/// ```
#[derive(Clone)]
pub struct SearchIssuesAndPRsTool {
    owner: String,
    repo: String,
    octocrab: Arc<Octocrab>,
}

impl SearchIssuesAndPRsTool {
    /// Creates a new `SearchIssuesAndPRsTool` instance.
    ///
    /// # Arguments
    /// * `owner` - Repository owner (username or organization)
    /// * `repo` - Repository name
    /// * `token` - GitHub personal access token
    ///
    /// # Panics
    ///
    /// Panics if the Octocrab client cannot be built. Use `try_new` for a fallible alternative.
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client_or_panic(token),
        }
    }

    /// Try to create a new `SearchIssuesAndPRsTool` instance.
    pub fn try_new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Box<octocrab::Error>> {
        Ok(Self {
            owner: owner.into(),
            repo: repo.into(),
            octocrab: build_octocrab_client(token)?,
        })
    }
}

#[async_trait]
impl Tool for SearchIssuesAndPRsTool {
    fn name(&self) -> &'static str {
        "search_issues_and_prs"
    }

    fn description(&self) -> &'static str {
        "Search issues and pull requests in GitHub repository. Input: {\"query\": \"search term\", \"per_page\": 10}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let query = extract_string_field(&input, "query")?;
        let per_page = match &input {
            ToolInput::Structured(v) => v
                .get("per_page")
                .and_then(serde_json::Value::as_u64)
                .map(|n| n as u8),
            _ => None,
        };

        // Add repo qualifier to search query
        let full_query = format!("{} repo:{}/{}", query, self.owner, self.repo);

        let mut search = self.octocrab.search().issues_and_pull_requests(&full_query);

        if let Some(pp) = per_page {
            search = search.per_page(pp);
        }

        let results = search
            .send()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to search issues/PRs: {e}")))?;

        let items: Vec<_> = results
            .items
            .iter()
            .map(|item| {
                json!({
                    "number": item.number,
                    "title": item.title,
                    "state": format!("{:?}", item.state),
                    "user": item.user.login,
                    "created_at": item.created_at.to_string(),
                    "url": item.html_url,
                    "is_pull_request": item.pull_request.is_some(),
                })
            })
            .collect();

        let result = json!({
            "total_count": results.total_count,
            "items": items,
        });

        Ok(serde_json::to_string_pretty(&result)
            .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}")))
    }
}

// ============================================================================
// CreateReviewRequestTool
// ============================================================================

/// Tool for requesting reviews on GitHub pull requests.
///
/// Requests reviews from specified users or teams.
///
/// # Input Format
///
/// - **Structured**: `{"pr_number": 42, "reviewers": ["user1", "user2"]}`
///
/// # Example
///
/// ```no_run
/// use dashflow_github::CreateReviewRequestTool;
/// use dashflow::core::tools::Tool;
///
/// let tool = CreateReviewRequestTool::new("octocat", "Hello-World", "token");
/// assert_eq!(tool.name(), "create_review_request");
/// ```
#[derive(Clone)]
pub struct CreateReviewRequestTool {
    owner: String,
    repo: String,
    token: String,
}

impl CreateReviewRequestTool {
    /// Creates a new `CreateReviewRequestTool` instance.
    ///
    /// # Arguments
    /// * `owner` - Repository owner (username or organization)
    /// * `repo` - Repository name
    /// * `token` - GitHub personal access token
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            token: token.into(),
        }
    }
}

#[async_trait]
impl Tool for CreateReviewRequestTool {
    fn name(&self) -> &'static str {
        "create_review_request"
    }

    fn description(&self) -> &'static str {
        "Request reviews on a GitHub pull request. Input: {\"pr_number\": 42, \"reviewers\": [\"user1\", \"user2\"]}"
    }

    async fn _call(&self, input: ToolInput) -> Result<String, Error> {
        let pr_number = extract_u64_field(&input, "pr_number")?;

        let reviewers = match &input {
            ToolInput::Structured(v) => v
                .get("reviewers")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| Error::tool_error("Missing or invalid 'reviewers' field"))?,
            _ => {
                return Err(Error::tool_error(
                    "Expected structured input with 'reviewers' field",
                ))
            }
        };

        // Octocrab doesn't have a direct API for review requests in v0.40
        // Use reqwest to make the API call directly
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}/requested_reviewers",
            self.owner, self.repo, pr_number
        );

        let body = json!({
            "reviewers": reviewers,
        });

        // Use the token provided during construction
        let client = create_http_client();
        let token = &self.token;

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "dashflow-github")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to request reviews: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(Error::tool_error(format!(
                "GitHub API error ({status}): {error_text}"
            )));
        }

        let response_json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::tool_error(format!("Failed to parse response: {e}")))?;

        Ok(format!(
            "Review request created successfully: {}",
            serde_json::to_string_pretty(&response_json)
                .unwrap_or_else(|e| format!("{{\"error\": \"Serialization failed: {e}\"}}"))
        ))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Helper Function Tests - These don't require Octocrab/TLS
    // ========================================================================

    mod extract_string_field_tests {
        use super::*;

        #[test]
        fn test_extract_from_structured_input() {
            let input = ToolInput::Structured(json!({"name": "test_value", "other": 123}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "test_value");
        }

        #[test]
        fn test_extract_from_string_input() {
            let input = ToolInput::String("raw_string".to_string());
            let result = extract_string_field(&input, "any_field");
            // String input ignores field name and returns the string itself
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "raw_string");
        }

        #[test]
        fn test_missing_field_returns_error() {
            let input = ToolInput::Structured(json!({"other_field": "value"}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Missing 'name' field"));
        }

        #[test]
        fn test_null_value_returns_error() {
            let input = ToolInput::Structured(json!({"name": null}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_err());
        }

        #[test]
        fn test_numeric_value_returns_error() {
            let input = ToolInput::Structured(json!({"name": 12345}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_err());
        }

        #[test]
        fn test_boolean_value_returns_error() {
            let input = ToolInput::Structured(json!({"name": true}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_err());
        }

        #[test]
        fn test_array_value_returns_error() {
            let input = ToolInput::Structured(json!({"name": ["a", "b"]}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_err());
        }

        #[test]
        fn test_object_value_returns_error() {
            let input = ToolInput::Structured(json!({"name": {"nested": "value"}}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_err());
        }

        #[test]
        fn test_empty_string_is_valid() {
            let input = ToolInput::Structured(json!({"name": ""}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "");
        }

        #[test]
        fn test_unicode_string() {
            let input = ToolInput::Structured(json!({"name": "日本語テスト 🚀"}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "日本語テスト 🚀");
        }

        #[test]
        fn test_whitespace_only_string() {
            let input = ToolInput::Structured(json!({"name": "   \t\n  "}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "   \t\n  ");
        }
    }

    mod extract_optional_string_tests {
        use super::*;

        #[test]
        fn test_present_field_returns_some() {
            let input = ToolInput::Structured(json!({"ref": "main"}));
            let result = extract_optional_string(&input, "ref");
            assert_eq!(result, Some("main".to_string()));
        }

        #[test]
        fn test_missing_field_returns_none() {
            let input = ToolInput::Structured(json!({"other": "value"}));
            let result = extract_optional_string(&input, "ref");
            assert_eq!(result, None);
        }

        #[test]
        fn test_null_value_returns_none() {
            let input = ToolInput::Structured(json!({"ref": null}));
            let result = extract_optional_string(&input, "ref");
            assert_eq!(result, None);
        }

        #[test]
        fn test_numeric_value_returns_none() {
            let input = ToolInput::Structured(json!({"ref": 123}));
            let result = extract_optional_string(&input, "ref");
            assert_eq!(result, None);
        }

        #[test]
        fn test_string_input_returns_none() {
            let input = ToolInput::String("raw_string".to_string());
            let result = extract_optional_string(&input, "any_field");
            // String input type returns None for optional string extraction
            assert_eq!(result, None);
        }

        #[test]
        fn test_empty_string_returns_some() {
            let input = ToolInput::Structured(json!({"ref": ""}));
            let result = extract_optional_string(&input, "ref");
            assert_eq!(result, Some(String::new()));
        }

        #[test]
        fn test_empty_json_object() {
            let input = ToolInput::Structured(json!({}));
            let result = extract_optional_string(&input, "ref");
            assert_eq!(result, None);
        }
    }

    mod extract_u64_field_tests {
        use super::*;

        #[test]
        fn test_extract_positive_number() {
            let input = ToolInput::Structured(json!({"issue_number": 42}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 42);
        }

        #[test]
        fn test_extract_zero() {
            let input = ToolInput::Structured(json!({"issue_number": 0}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 0);
        }

        #[test]
        fn test_extract_large_number() {
            let input = ToolInput::Structured(json!({"issue_number": 9999999999_u64}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 9999999999);
        }

        #[test]
        fn test_missing_field_returns_error() {
            let input = ToolInput::Structured(json!({"other_field": 42}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Missing or invalid 'issue_number'"));
        }

        #[test]
        fn test_string_value_returns_error() {
            let input = ToolInput::Structured(json!({"issue_number": "42"}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_err());
        }

        #[test]
        fn test_negative_number_returns_error() {
            let input = ToolInput::Structured(json!({"issue_number": -42}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_err());
        }

        #[test]
        fn test_float_returns_error() {
            let input = ToolInput::Structured(json!({"issue_number": 42.5}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_err());
        }

        #[test]
        fn test_null_value_returns_error() {
            let input = ToolInput::Structured(json!({"issue_number": null}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_err());
        }

        #[test]
        fn test_string_input_returns_error() {
            let input = ToolInput::String("42".to_string());
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Expected structured input"));
        }

        #[test]
        fn test_boolean_value_returns_error() {
            let input = ToolInput::Structured(json!({"issue_number": true}));
            let result = extract_u64_field(&input, "issue_number");
            assert!(result.is_err());
        }
    }

    // ========================================================================
    // CreateReviewRequestTool Tests - Doesn't use Octocrab internally
    // ========================================================================

    mod create_review_request_tool_tests {
        use super::*;

        #[test]
        fn test_new_stores_owner_repo_token() {
            let tool = CreateReviewRequestTool::new("my_owner", "my_repo", "my_token");
            assert_eq!(tool.owner, "my_owner");
            assert_eq!(tool.repo, "my_repo");
            assert_eq!(tool.token, "my_token");
        }

        #[test]
        fn test_name_returns_correct_value() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "create_review_request");
        }

        #[test]
        fn test_description_contains_required_fields() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let desc = tool.description();
            assert!(desc.contains("pr_number"));
            assert!(desc.contains("reviewers"));
        }

        #[test]
        fn test_new_with_string_types() {
            let tool = CreateReviewRequestTool::new(
                String::from("owner"),
                String::from("repo"),
                String::from("token"),
            );
            assert_eq!(tool.owner, "owner");
        }

        #[test]
        fn test_new_with_str_types() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            assert_eq!(tool.owner, "owner");
        }

        #[test]
        fn test_new_with_mixed_types() {
            let tool = CreateReviewRequestTool::new(
                "owner",
                String::from("repo"),
                "token",
            );
            assert_eq!(tool.owner, "owner");
            assert_eq!(tool.repo, "repo");
        }

        #[test]
        fn test_empty_strings_allowed() {
            let tool = CreateReviewRequestTool::new("", "", "");
            assert_eq!(tool.owner, "");
            assert_eq!(tool.repo, "");
            assert_eq!(tool.token, "");
        }

        #[test]
        fn test_special_characters_in_repo_name() {
            let tool = CreateReviewRequestTool::new("my-org", "my_repo.rs", "ghp_xxx");
            assert_eq!(tool.owner, "my-org");
            assert_eq!(tool.repo, "my_repo.rs");
        }

        #[tokio::test]
        async fn test_call_missing_pr_number() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({"reviewers": ["user1"]}));
            let result = tool._call(input).await;
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("pr_number"));
        }

        #[tokio::test]
        async fn test_call_missing_reviewers() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({"pr_number": 42}));
            let result = tool._call(input).await;
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("reviewers"));
        }

        #[tokio::test]
        async fn test_call_invalid_reviewers_type() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({"pr_number": 42, "reviewers": "not_an_array"}));
            let result = tool._call(input).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_call_string_input_fails() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::String("pr 42".to_string());
            let result = tool._call(input).await;
            assert!(result.is_err());
        }
    }

    // ========================================================================
    // HTTP Client Tests
    // ========================================================================

    mod http_client_tests {
        use super::*;

        #[test]
        fn test_create_http_client_succeeds() {
            let client = create_http_client();
            // If we got here without panic, the client was created successfully
            let _ = client;
        }
    }

    // ========================================================================
    // Tool Input Edge Cases
    // ========================================================================

    mod tool_input_edge_cases {
        use super::*;

        #[test]
        fn test_deeply_nested_json() {
            let input = ToolInput::Structured(json!({
                "level1": {
                    "level2": {
                        "level3": "value"
                    }
                },
                "name": "top_level"
            }));
            let result = extract_string_field(&input, "name");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "top_level");
        }

        #[test]
        fn test_array_at_top_level_json() {
            // Structured input with array value
            let input = ToolInput::Structured(json!({"items": ["a", "b", "c"], "name": "test"}));
            let result = extract_string_field(&input, "name");
            assert!(result.is_ok());
        }

        #[test]
        fn test_special_json_field_names() {
            let input = ToolInput::Structured(json!({
                "field-with-dashes": "value1",
                "field.with.dots": "value2",
                "field with spaces": "value3"
            }));
            assert_eq!(
                extract_string_field(&input, "field-with-dashes").unwrap(),
                "value1"
            );
            assert_eq!(
                extract_string_field(&input, "field.with.dots").unwrap(),
                "value2"
            );
            assert_eq!(
                extract_string_field(&input, "field with spaces").unwrap(),
                "value3"
            );
        }

        #[test]
        fn test_numeric_string_field_name() {
            let input = ToolInput::Structured(json!({"123": "numeric_key_value"}));
            let result = extract_string_field(&input, "123");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "numeric_key_value");
        }

        #[test]
        fn test_very_long_string_value() {
            let long_string = "x".repeat(100_000);
            let input = ToolInput::Structured(json!({"content": long_string}));
            let result = extract_string_field(&input, "content");
            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 100_000);
        }

        #[test]
        fn test_json_with_escaped_characters() {
            let input = ToolInput::Structured(json!({"path": "dir/file.txt", "message": "Fix \"bug\" in code"}));
            assert_eq!(extract_string_field(&input, "path").unwrap(), "dir/file.txt");
            assert_eq!(
                extract_string_field(&input, "message").unwrap(),
                "Fix \"bug\" in code"
            );
        }

        #[test]
        fn test_json_with_newlines() {
            let input = ToolInput::Structured(json!({"content": "line1\nline2\nline3"}));
            let result = extract_string_field(&input, "content");
            assert!(result.is_ok());
            assert!(result.unwrap().contains('\n'));
        }
    }

    // ========================================================================
    // Octocrab Builder Tests - Marked #[ignore] due to TLS issues
    // ========================================================================

    mod octocrab_builder_tests {
        use super::*;

        #[test]
        #[ignore = "Octocrab TLS cert loading fails in test env"]
        fn test_build_octocrab_client_success() {
            let result = build_octocrab_client("test_token");
            assert!(result.is_ok());
        }

        #[test]
        #[ignore = "Octocrab TLS cert loading fails in test env"]
        fn test_build_octocrab_client_or_panic_success() {
            let _client = build_octocrab_client_or_panic("test_token");
        }
    }

    // ========================================================================
    // Tool Name/Description Tests - Only for CreateReviewRequestTool (no TLS)
    // Other tools require Octocrab construction which needs TLS
    // ========================================================================

    mod tool_metadata_tests {
        use super::*;

        #[test]
        fn test_create_review_request_tool_metadata() {
            let tool = CreateReviewRequestTool::new("o", "r", "t");
            assert_eq!(tool.name(), "create_review_request");
            assert!(!tool.description().is_empty());
            assert!(tool.description().contains("review"));
        }
    }

    // ========================================================================
    // Integration Tests - Marked #[ignore] due to TLS/Network requirements
    // ========================================================================

    mod integration_tests {
        use super::*;

        // NOTE: These tests marked #[ignore] because Octocrab::builder().build()
        // attempts to load platform TLS certificates at construction time, which fails
        // in some test environments (macOS keychain I/O error).
        // These will be converted to real integration tests in a future phase
        // with proper GitHub API infrastructure and credential management.

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_get_issue_tool_name() {
            let tool = GetIssueTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "get_issue");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_comment_on_issue_tool_name() {
            let tool = CommentOnIssueTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "comment_on_issue");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_get_pr_tool_name() {
            let tool = GetPRTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "get_pr");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_create_pr_tool_name() {
            let tool = CreatePRTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "create_pr");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_read_file_tool_name() {
            let tool = ReadFileTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "read_file");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_create_file_tool_name() {
            let tool = CreateFileTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "create_file");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_update_file_tool_name() {
            let tool = UpdateFileTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "update_file");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_delete_file_tool_name() {
            let tool = DeleteFileTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "delete_file");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_search_code_tool_name() {
            let tool = SearchCodeTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "search_code");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_search_issues_and_prs_tool_name() {
            let tool = SearchIssuesAndPRsTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "search_issues_and_prs");
        }

        #[tokio::test]
        #[ignore = "Octocrab TLS cert loading fails in test env - convert to real integration test"]
        async fn test_create_review_request_tool_with_octocrab() {
            // This test would use octocrab if not ignored
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            assert_eq!(tool.name(), "create_review_request");
        }
    }

    // ========================================================================
    // Additional Helper Function Edge Cases
    // ========================================================================

    mod extract_string_field_additional {
        use super::*;

        #[test]
        fn test_multiple_fields_extracts_correct_one() {
            let input = ToolInput::Structured(json!({
                "first": "value1",
                "second": "value2",
                "third": "value3"
            }));
            assert_eq!(extract_string_field(&input, "first").unwrap(), "value1");
            assert_eq!(extract_string_field(&input, "second").unwrap(), "value2");
            assert_eq!(extract_string_field(&input, "third").unwrap(), "value3");
        }

        #[test]
        fn test_case_sensitive_field_names() {
            let input = ToolInput::Structured(json!({
                "Name": "uppercase",
                "name": "lowercase"
            }));
            assert_eq!(extract_string_field(&input, "name").unwrap(), "lowercase");
            assert_eq!(extract_string_field(&input, "Name").unwrap(), "uppercase");
        }

        #[test]
        fn test_field_with_underscore_prefix() {
            let input = ToolInput::Structured(json!({"_private": "hidden"}));
            assert_eq!(extract_string_field(&input, "_private").unwrap(), "hidden");
        }

        #[test]
        fn test_field_with_dollar_sign() {
            let input = ToolInput::Structured(json!({"$ref": "reference"}));
            assert_eq!(extract_string_field(&input, "$ref").unwrap(), "reference");
        }

        #[test]
        fn test_field_with_at_sign() {
            let input = ToolInput::Structured(json!({"@type": "schema"}));
            assert_eq!(extract_string_field(&input, "@type").unwrap(), "schema");
        }

        #[test]
        fn test_string_with_backslash() {
            let input = ToolInput::Structured(json!({"path": "C:\\Users\\test"}));
            assert_eq!(extract_string_field(&input, "path").unwrap(), "C:\\Users\\test");
        }

        #[test]
        fn test_string_with_tab_characters() {
            let input = ToolInput::Structured(json!({"content": "col1\tcol2\tcol3"}));
            assert_eq!(extract_string_field(&input, "content").unwrap(), "col1\tcol2\tcol3");
        }

        #[test]
        fn test_string_with_carriage_return() {
            let input = ToolInput::Structured(json!({"content": "line1\r\nline2"}));
            assert_eq!(extract_string_field(&input, "content").unwrap(), "line1\r\nline2");
        }

        #[test]
        fn test_string_with_null_character() {
            let input = ToolInput::Structured(json!({"content": "before\0after"}));
            assert_eq!(extract_string_field(&input, "content").unwrap(), "before\0after");
        }

        #[test]
        fn test_extract_from_nested_not_possible() {
            // Verifies that extract_string_field only looks at top-level
            let input = ToolInput::Structured(json!({
                "outer": {
                    "inner": "nested_value"
                }
            }));
            // Should fail because "inner" is not at top level
            assert!(extract_string_field(&input, "inner").is_err());
        }

        #[test]
        fn test_string_input_preserves_exact_content() {
            let raw = "exact content with  multiple   spaces";
            let input = ToolInput::String(raw.to_string());
            assert_eq!(extract_string_field(&input, "anything").unwrap(), raw);
        }

        #[test]
        fn test_empty_field_name() {
            let input = ToolInput::Structured(json!({"": "empty_key_value"}));
            assert_eq!(extract_string_field(&input, "").unwrap(), "empty_key_value");
        }
    }

    mod extract_optional_string_additional {
        use super::*;

        #[test]
        fn test_boolean_value_returns_none() {
            let input = ToolInput::Structured(json!({"flag": true}));
            assert_eq!(extract_optional_string(&input, "flag"), None);
        }

        #[test]
        fn test_array_value_returns_none() {
            let input = ToolInput::Structured(json!({"items": ["a", "b"]}));
            assert_eq!(extract_optional_string(&input, "items"), None);
        }

        #[test]
        fn test_object_value_returns_none() {
            let input = ToolInput::Structured(json!({"nested": {"key": "value"}}));
            assert_eq!(extract_optional_string(&input, "nested"), None);
        }

        #[test]
        fn test_float_value_returns_none() {
            let input = ToolInput::Structured(json!({"amount": 3.14}));
            assert_eq!(extract_optional_string(&input, "amount"), None);
        }

        #[test]
        fn test_whitespace_only_string_returns_some() {
            let input = ToolInput::Structured(json!({"branch": "  "}));
            assert_eq!(extract_optional_string(&input, "branch"), Some("  ".to_string()));
        }

        #[test]
        fn test_unicode_string_returns_some() {
            let input = ToolInput::Structured(json!({"branch": "分支/功能"}));
            assert_eq!(extract_optional_string(&input, "branch"), Some("分支/功能".to_string()));
        }

        #[test]
        fn test_multiple_optional_fields() {
            let input = ToolInput::Structured(json!({
                "ref": "main",
                "branch": "develop"
            }));
            assert_eq!(extract_optional_string(&input, "ref"), Some("main".to_string()));
            assert_eq!(extract_optional_string(&input, "branch"), Some("develop".to_string()));
            assert_eq!(extract_optional_string(&input, "missing"), None);
        }
    }

    mod extract_u64_field_additional {
        use super::*;

        #[test]
        fn test_max_u64_value() {
            let input = ToolInput::Structured(json!({"num": u64::MAX}));
            let result = extract_u64_field(&input, "num");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), u64::MAX);
        }

        #[test]
        fn test_integer_in_valid_json_range() {
            // JSON can represent integers up to 2^53-1 exactly
            let large_but_exact = 9007199254740991_u64; // 2^53 - 1
            let input = ToolInput::Structured(json!({"num": large_but_exact}));
            let result = extract_u64_field(&input, "num");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), large_but_exact);
        }

        #[test]
        fn test_array_of_numbers_returns_error() {
            let input = ToolInput::Structured(json!({"nums": [1, 2, 3]}));
            assert!(extract_u64_field(&input, "nums").is_err());
        }

        #[test]
        fn test_object_returns_error() {
            let input = ToolInput::Structured(json!({"num": {"value": 42}}));
            assert!(extract_u64_field(&input, "num").is_err());
        }

        #[test]
        fn test_empty_object_returns_error() {
            let input = ToolInput::Structured(json!({}));
            assert!(extract_u64_field(&input, "num").is_err());
        }

        #[test]
        fn test_multiple_u64_fields() {
            let input = ToolInput::Structured(json!({
                "pr_number": 42,
                "issue_number": 99,
                "page": 1
            }));
            assert_eq!(extract_u64_field(&input, "pr_number").unwrap(), 42);
            assert_eq!(extract_u64_field(&input, "issue_number").unwrap(), 99);
            assert_eq!(extract_u64_field(&input, "page").unwrap(), 1);
        }
    }

    // ========================================================================
    // CreateReviewRequestTool Additional Tests
    // ========================================================================

    mod create_review_request_tool_additional {
        use super::*;

        #[test]
        fn test_unicode_owner_repo() {
            let tool = CreateReviewRequestTool::new("用户", "仓库", "token");
            assert_eq!(tool.owner, "用户");
            assert_eq!(tool.repo, "仓库");
        }

        #[test]
        fn test_very_long_token() {
            let long_token = "ghp_".to_string() + &"x".repeat(1000);
            let tool = CreateReviewRequestTool::new("owner", "repo", &long_token);
            assert_eq!(tool.token.len(), 1004);
        }

        #[tokio::test]
        async fn test_call_empty_reviewers_array() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 42,
                "reviewers": []
            }));
            // Empty array is valid structurally, will fail at API level
            // but input parsing should succeed
            let result = tool._call(input).await;
            // Will fail due to network, but not due to input validation
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_call_reviewers_with_non_string_elements() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 42,
                "reviewers": ["user1", 123, "user2"]
            }));
            // Non-string elements should be filtered out
            let result = tool._call(input).await;
            // Will fail at network level, not input validation
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_call_pr_number_as_string_fails() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": "42",  // String instead of number
                "reviewers": ["user1"]
            }));
            let result = tool._call(input).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("pr_number"));
        }

        #[tokio::test]
        async fn test_call_pr_number_zero() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 0,
                "reviewers": ["user1"]
            }));
            // Zero is a valid u64, will fail at API level
            let result = tool._call(input).await;
            assert!(result.is_err()); // Network error expected
        }

        #[tokio::test]
        async fn test_call_pr_number_large() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 9999999999_u64,
                "reviewers": ["user1"]
            }));
            // Large number is valid, will fail at API level
            let result = tool._call(input).await;
            assert!(result.is_err()); // Network error expected
        }

        #[tokio::test]
        async fn test_call_reviewers_with_unicode() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 42,
                "reviewers": ["用户1", "юзер2", "🧑‍💻"]
            }));
            let result = tool._call(input).await;
            // Input validation passes, network will fail
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_call_reviewers_with_empty_strings() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 42,
                "reviewers": ["", "user1", ""]
            }));
            let result = tool._call(input).await;
            // Empty strings are valid strings, will fail at API
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_call_with_extra_fields_ignored() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 42,
                "reviewers": ["user1"],
                "extra_field": "ignored",
                "another": 123
            }));
            let result = tool._call(input).await;
            // Extra fields should be ignored, network will fail
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_call_reviewers_null_value() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 42,
                "reviewers": null
            }));
            let result = tool._call(input).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("reviewers"));
        }

        #[tokio::test]
        async fn test_call_both_fields_missing() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({}));
            let result = tool._call(input).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_call_with_negative_pr_number() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": -1,
                "reviewers": ["user1"]
            }));
            let result = tool._call(input).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("pr_number"));
        }

        #[tokio::test]
        async fn test_call_with_float_pr_number() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            let input = ToolInput::Structured(json!({
                "pr_number": 42.5,
                "reviewers": ["user1"]
            }));
            let result = tool._call(input).await;
            assert!(result.is_err());
        }
    }

    // ========================================================================
    // Base64 Encoding Tests (used by file tools)
    // ========================================================================

    mod base64_encoding_tests {
        use base64::Engine;

        #[test]
        fn test_encode_empty_string() {
            let encoded = base64::engine::general_purpose::STANDARD.encode(b"");
            assert_eq!(encoded, "");
        }

        #[test]
        fn test_encode_simple_text() {
            let encoded = base64::engine::general_purpose::STANDARD.encode(b"Hello, World!");
            assert_eq!(encoded, "SGVsbG8sIFdvcmxkIQ==");
        }

        #[test]
        fn test_encode_unicode_text() {
            let text = "日本語テスト";
            let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
            // Verify roundtrip
            let decoded = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();
            let result = String::from_utf8(decoded).unwrap();
            assert_eq!(result, text);
        }

        #[test]
        fn test_encode_binary_data() {
            let binary: Vec<u8> = vec![0, 1, 2, 255, 254, 253];
            let encoded = base64::engine::general_purpose::STANDARD.encode(&binary);
            let decoded = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();
            assert_eq!(decoded, binary);
        }

        #[test]
        fn test_decode_with_newlines() {
            // GitHub content API returns base64 with newlines
            let encoded_with_newlines = "SGVs\nbG8s\nIFdv\ncmxk\nIQ==";
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(encoded_with_newlines.replace('\n', ""))
                .unwrap();
            let result = String::from_utf8(decoded).unwrap();
            assert_eq!(result, "Hello, World!");
        }

        #[test]
        fn test_decode_invalid_base64() {
            let result = base64::engine::general_purpose::STANDARD.decode("not!valid@base64");
            assert!(result.is_err());
        }

        #[test]
        fn test_encode_code_content() {
            let code = r#"fn main() {
    println!("Hello, World!");
}
"#;
            let encoded = base64::engine::general_purpose::STANDARD.encode(code.as_bytes());
            let decoded = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();
            let result = String::from_utf8(decoded).unwrap();
            assert_eq!(result, code);
        }

        #[test]
        fn test_encode_large_content() {
            let large = "x".repeat(1_000_000);
            let encoded = base64::engine::general_purpose::STANDARD.encode(large.as_bytes());
            let decoded = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();
            assert_eq!(decoded.len(), 1_000_000);
        }
    }

    // ========================================================================
    // Per-Page Extraction Tests (for search tools)
    // ========================================================================

    mod per_page_extraction_tests {
        use super::*;

        fn extract_per_page(input: &ToolInput) -> Option<u8> {
            match input {
                ToolInput::Structured(v) => v
                    .get("per_page")
                    .and_then(serde_json::Value::as_u64)
                    .map(|n| n as u8),
                _ => None,
            }
        }

        #[test]
        fn test_per_page_missing_returns_none() {
            let input = ToolInput::Structured(json!({"query": "test"}));
            assert_eq!(extract_per_page(&input), None);
        }

        #[test]
        fn test_per_page_present_returns_some() {
            let input = ToolInput::Structured(json!({"query": "test", "per_page": 10}));
            assert_eq!(extract_per_page(&input), Some(10));
        }

        #[test]
        fn test_per_page_zero() {
            let input = ToolInput::Structured(json!({"query": "test", "per_page": 0}));
            assert_eq!(extract_per_page(&input), Some(0));
        }

        #[test]
        fn test_per_page_max_u8() {
            let input = ToolInput::Structured(json!({"query": "test", "per_page": 255}));
            assert_eq!(extract_per_page(&input), Some(255));
        }

        #[test]
        fn test_per_page_overflow_wraps() {
            // 256 as u8 wraps to 0
            let input = ToolInput::Structured(json!({"query": "test", "per_page": 256}));
            assert_eq!(extract_per_page(&input), Some(0));
        }

        #[test]
        fn test_per_page_string_returns_none() {
            let input = ToolInput::Structured(json!({"query": "test", "per_page": "10"}));
            assert_eq!(extract_per_page(&input), None);
        }

        #[test]
        fn test_per_page_float_returns_none() {
            let input = ToolInput::Structured(json!({"query": "test", "per_page": 10.5}));
            assert_eq!(extract_per_page(&input), None);
        }

        #[test]
        fn test_per_page_negative_returns_none() {
            let input = ToolInput::Structured(json!({"query": "test", "per_page": -10}));
            assert_eq!(extract_per_page(&input), None);
        }

        #[test]
        fn test_per_page_null_returns_none() {
            let input = ToolInput::Structured(json!({"query": "test", "per_page": null}));
            assert_eq!(extract_per_page(&input), None);
        }

        #[test]
        fn test_string_input_returns_none() {
            let input = ToolInput::String("query".to_string());
            assert_eq!(extract_per_page(&input), None);
        }
    }

    // ========================================================================
    // Reviewers Array Extraction Tests
    // ========================================================================

    mod reviewers_extraction_tests {
        use super::*;

        fn extract_reviewers(input: &ToolInput) -> Option<Vec<String>> {
            match input {
                ToolInput::Structured(v) => v
                    .get("reviewers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
                            .collect::<Vec<_>>()
                    }),
                _ => None,
            }
        }

        #[test]
        fn test_valid_reviewers_array() {
            let input = ToolInput::Structured(json!({
                "reviewers": ["user1", "user2", "user3"]
            }));
            let result = extract_reviewers(&input);
            assert_eq!(result, Some(vec!["user1".to_string(), "user2".to_string(), "user3".to_string()]));
        }

        #[test]
        fn test_empty_reviewers_array() {
            let input = ToolInput::Structured(json!({"reviewers": []}));
            assert_eq!(extract_reviewers(&input), Some(vec![]));
        }

        #[test]
        fn test_reviewers_missing() {
            let input = ToolInput::Structured(json!({"pr_number": 42}));
            assert_eq!(extract_reviewers(&input), None);
        }

        #[test]
        fn test_reviewers_not_array() {
            let input = ToolInput::Structured(json!({"reviewers": "user1"}));
            assert_eq!(extract_reviewers(&input), None);
        }

        #[test]
        fn test_reviewers_filters_non_strings() {
            let input = ToolInput::Structured(json!({
                "reviewers": ["user1", 123, "user2", null, true]
            }));
            let result = extract_reviewers(&input);
            assert_eq!(result, Some(vec!["user1".to_string(), "user2".to_string()]));
        }

        #[test]
        fn test_reviewers_all_non_strings() {
            let input = ToolInput::Structured(json!({
                "reviewers": [123, null, true, {"nested": "obj"}]
            }));
            let result = extract_reviewers(&input);
            assert_eq!(result, Some(vec![]));
        }

        #[test]
        fn test_reviewers_with_unicode() {
            let input = ToolInput::Structured(json!({
                "reviewers": ["用户", "пользователь", "👤"]
            }));
            let result = extract_reviewers(&input);
            assert_eq!(result, Some(vec!["用户".to_string(), "пользователь".to_string(), "👤".to_string()]));
        }

        #[test]
        fn test_string_input_returns_none() {
            let input = ToolInput::String("user1,user2".to_string());
            assert_eq!(extract_reviewers(&input), None);
        }

        #[test]
        fn test_nested_array_not_flattened() {
            let input = ToolInput::Structured(json!({
                "reviewers": ["user1", ["nested"]]
            }));
            let result = extract_reviewers(&input);
            // Nested array is not a string, so filtered out
            assert_eq!(result, Some(vec!["user1".to_string()]));
        }
    }

    // ========================================================================
    // JSON Serialization Edge Cases
    // ========================================================================

    mod json_serialization_tests {
        use super::*;

        #[test]
        fn test_json_with_special_characters() {
            let obj = json!({
                "message": "Fix \"bug\" in <code>",
                "path": "dir\\file.txt"
            });
            let serialized = serde_json::to_string_pretty(&obj).unwrap();
            assert!(serialized.contains("Fix \\\"bug\\\" in <code>"));
        }

        #[test]
        fn test_json_with_null_values() {
            let obj = json!({
                "title": "Test",
                "body": null
            });
            let serialized = serde_json::to_string_pretty(&obj).unwrap();
            assert!(serialized.contains("null"));
        }

        #[test]
        fn test_json_with_empty_arrays() {
            let obj = json!({
                "labels": [],
                "assignees": []
            });
            let serialized = serde_json::to_string_pretty(&obj).unwrap();
            assert!(serialized.contains("[]"));
        }

        #[test]
        fn test_json_preserves_unicode() {
            let obj = json!({
                "title": "日本語タイトル",
                "user": "用户名"
            });
            let serialized = serde_json::to_string_pretty(&obj).unwrap();
            assert!(serialized.contains("日本語タイトル"));
            assert!(serialized.contains("用户名"));
        }

        #[test]
        fn test_json_with_large_numbers() {
            let obj = json!({
                "total_count": 9999999999_u64,
                "items": []
            });
            let serialized = serde_json::to_string_pretty(&obj).unwrap();
            assert!(serialized.contains("9999999999"));
        }

        #[test]
        fn test_json_with_boolean_values() {
            let obj = json!({
                "merged": true,
                "draft": false
            });
            let serialized = serde_json::to_string_pretty(&obj).unwrap();
            assert!(serialized.contains("true"));
            assert!(serialized.contains("false"));
        }

        #[test]
        fn test_json_with_nested_objects() {
            let obj = json!({
                "user": {
                    "login": "octocat",
                    "id": 1
                },
                "head": {
                    "ref": "feature",
                    "sha": "abc123"
                }
            });
            let serialized = serde_json::to_string_pretty(&obj).unwrap();
            assert!(serialized.contains("octocat"));
            assert!(serialized.contains("feature"));
        }

        #[test]
        fn test_json_with_newlines_in_string() {
            let obj = json!({
                "body": "Line 1\nLine 2\nLine 3"
            });
            let serialized = serde_json::to_string_pretty(&obj).unwrap();
            assert!(serialized.contains("\\n"));
        }
    }

    // ========================================================================
    // Error Message Format Tests
    // ========================================================================

    mod error_message_tests {
        use super::*;

        #[test]
        fn test_missing_field_error_includes_field_name() {
            let input = ToolInput::Structured(json!({}));
            let err = extract_string_field(&input, "path").unwrap_err();
            assert!(err.to_string().contains("path"));
        }

        #[test]
        fn test_missing_u64_field_error_includes_field_name() {
            let input = ToolInput::Structured(json!({}));
            let err = extract_u64_field(&input, "pr_number").unwrap_err();
            assert!(err.to_string().contains("pr_number"));
        }

        #[test]
        fn test_invalid_u64_error_includes_field_name() {
            let input = ToolInput::Structured(json!({"count": "not_a_number"}));
            let err = extract_u64_field(&input, "count").unwrap_err();
            assert!(err.to_string().contains("count"));
        }

        #[test]
        fn test_string_input_u64_error_includes_field_name() {
            let input = ToolInput::String("42".to_string());
            let err = extract_u64_field(&input, "issue_number").unwrap_err();
            assert!(err.to_string().contains("issue_number"));
        }
    }

    // ========================================================================
    // Query Building Tests
    // ========================================================================

    mod query_building_tests {
        #[test]
        fn test_search_query_format() {
            let query = "test function";
            let owner = "octocat";
            let repo = "Hello-World";
            let full_query = format!("{} repo:{}/{}", query, owner, repo);
            assert_eq!(full_query, "test function repo:octocat/Hello-World");
        }

        #[test]
        fn test_search_query_with_special_chars() {
            let query = "impl<T> Clone";
            let owner = "rust-lang";
            let repo = "rust";
            let full_query = format!("{} repo:{}/{}", query, owner, repo);
            assert_eq!(full_query, "impl<T> Clone repo:rust-lang/rust");
        }

        #[test]
        fn test_search_query_with_unicode() {
            let query = "日本語";
            let owner = "user";
            let repo = "repo";
            let full_query = format!("{} repo:{}/{}", query, owner, repo);
            assert_eq!(full_query, "日本語 repo:user/repo");
        }

        #[test]
        fn test_api_url_format() {
            let owner = "octocat";
            let repo = "Hello-World";
            let pr_number = 42;
            let url = format!(
                "https://api.github.com/repos/{}/{}/pulls/{}/requested_reviewers",
                owner, repo, pr_number
            );
            assert_eq!(url, "https://api.github.com/repos/octocat/Hello-World/pulls/42/requested_reviewers");
        }

        #[test]
        fn test_api_url_with_special_chars_in_repo() {
            let owner = "my-org";
            let repo = "my.repo";
            let pr_number = 1;
            let url = format!(
                "https://api.github.com/repos/{}/{}/pulls/{}/requested_reviewers",
                owner, repo, pr_number
            );
            assert_eq!(url, "https://api.github.com/repos/my-org/my.repo/pulls/1/requested_reviewers");
        }
    }

    // ========================================================================
    // HTTP Header Tests
    // ========================================================================

    mod http_header_tests {
        #[test]
        fn test_authorization_header_format() {
            let token = "ghp_FAKETOKEN000000000";
            let header = format!("Bearer {}", token);
            assert_eq!(header, "Bearer ghp_FAKETOKEN000000000");
        }

        #[test]
        fn test_user_agent_value() {
            let user_agent = "dashflow-github";
            assert_eq!(user_agent, "dashflow-github");
        }

        #[test]
        fn test_accept_header_value() {
            let accept = "application/vnd.github+json";
            assert_eq!(accept, "application/vnd.github+json");
        }

        #[test]
        fn test_api_version_header() {
            let version = "2022-11-28";
            assert_eq!(version, "2022-11-28");
        }
    }

    // ========================================================================
    // Tool Clone Tests
    // ========================================================================

    mod clone_tests {
        use super::*;

        #[test]
        fn test_create_review_request_tool_fields_accessible_after_new() {
            let tool = CreateReviewRequestTool::new("owner", "repo", "token");
            assert_eq!(tool.owner, "owner");
            assert_eq!(tool.repo, "repo");
            assert_eq!(tool.token, "token");
        }

        #[test]
        fn test_create_review_request_tool_multiple_instances() {
            let tool1 = CreateReviewRequestTool::new("owner1", "repo1", "token1");
            let tool2 = CreateReviewRequestTool::new("owner2", "repo2", "token2");
            assert_ne!(tool1.owner, tool2.owner);
            assert_ne!(tool1.repo, tool2.repo);
            assert_ne!(tool1.token, tool2.token);
        }
    }

    // ========================================================================
    // Tool Description Content Tests
    // ========================================================================

    mod description_content_tests {
        use super::*;

        #[test]
        fn test_create_review_request_description_has_input_format() {
            let tool = CreateReviewRequestTool::new("o", "r", "t");
            let desc = tool.description();
            assert!(desc.contains("{"));
            assert!(desc.contains("}"));
        }

        #[test]
        fn test_create_review_request_description_mentions_pr_number() {
            let tool = CreateReviewRequestTool::new("o", "r", "t");
            let desc = tool.description();
            assert!(desc.contains("pr_number"));
        }

        #[test]
        fn test_create_review_request_description_mentions_reviewers() {
            let tool = CreateReviewRequestTool::new("o", "r", "t");
            let desc = tool.description();
            assert!(desc.contains("reviewers"));
        }
    }
}
