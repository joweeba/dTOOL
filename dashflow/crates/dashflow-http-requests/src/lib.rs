//! # HTTP Requests Tools
//!
//! HTTP request tools for making API calls with GET, POST, PUT, PATCH, and DELETE methods.
//! These tools allow agents to interact with REST APIs and web services.
//!
//! ## Features
//!
//! - Support for all HTTP methods: GET, POST, PUT, PATCH, DELETE
//! - JSON request/response handling
//! - Custom headers support
//! - Flexible authentication (API keys, Bearer tokens, etc.)
//! - Timeout configuration
//! - `RequestsToolkit` for bundling all HTTP tools for agents
//!
//! ## Usage
//!
//! ### Using Individual Tools
//!
//! ```rust,no_run
//! use dashflow_http_requests::{HttpGetTool, HttpPostTool};
//! use dashflow::core::tools::Tool;
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() {
//!     // GET request
//!     let get_tool = HttpGetTool::new();
//!     let response = get_tool._call_str(
//!         json!({
//!             "url": "https://api.example.com/data",
//!             "headers": {
//!                 "Authorization": "Bearer token123"
//!             }
//!         }).to_string()
//!     ).await.unwrap();
//!
//!     // POST request
//!     let post_tool = HttpPostTool::new();
//!     let response = post_tool._call_str(
//!         json!({
//!             "url": "https://api.example.com/create",
//!             "data": {"name": "test", "value": 42},
//!             "headers": {
//!                 "Content-Type": "application/json"
//!             }
//!         }).to_string()
//!     ).await.unwrap();
//! }
//! ```
//!
//! ### Using `RequestsToolkit`
//!
//! ```rust
//! use dashflow_http_requests::RequestsToolkit;
//! use dashflow::core::tools::BaseToolkit;
//!
//! let toolkit = RequestsToolkit::new();
//! let tools = toolkit.get_tools();
//!
//! // Pass tools to agent for HTTP request capabilities
//! // tools contains: HttpGetTool, HttpPostTool, HttpPutTool, HttpPatchTool, HttpDeleteTool
//! ```
//!
//! ### Using `OpenAPIToolkit`
//!
//! ```rust
//! use dashflow_http_requests::OpenAPIToolkit;
//! use dashflow_json::JsonSpec;
//! use dashflow::core::tools::BaseToolkit;
//! use serde_json::json;
//!
//! # #[tokio::main]
//! # async fn main() {
//! // Load OpenAPI spec
//! let spec = json!({
//!     "openapi": "3.0.0",
//!     "info": {"title": "My API", "version": "1.0.0"},
//!     "paths": {
//!         "/users": {
//!             "get": {"summary": "Get users"}
//!         }
//!     }
//! });
//!
//! let json_spec = JsonSpec::new(spec);
//! let toolkit = OpenAPIToolkit::new(json_spec);
//! let tools = toolkit.get_tools();
//!
//! // tools contains: 5 HTTP tools + 2 JSON tools = 7 total
//! // Use with agent to interact with OpenAPI-compliant APIs
//! # }
//! ```

use async_trait::async_trait;
use dashflow::constants::DEFAULT_HTTP_REQUEST_TIMEOUT;
use dashflow::core::http_client;
use dashflow::core::tools::{Tool, ToolInput};
use dashflow::core::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

// Toolkit modules
mod openapi_toolkit;
mod toolkit;

// Re-exports
pub use openapi_toolkit::OpenAPIToolkit;
pub use toolkit::RequestsToolkit;

/// HTTP request configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    /// Target URL
    pub url: String,
    /// Optional headers
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Optional request body (for POST, PUT, PATCH)
    #[serde(default)]
    pub data: Option<Value>,
    /// Optional timeout in seconds
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// HTTP response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    /// HTTP status code
    pub status: u16,
    /// Response headers
    pub headers: HashMap<String, String>,
    /// Response body as text
    pub body: String,
}

/// Base HTTP tool with shared functionality
struct BaseHttpTool {
    client: Client,
    method: reqwest::Method,
    name: String,
    description: String,
}

impl BaseHttpTool {
    fn new(method: reqwest::Method, name: String, description: String) -> Self {
        // Use optimized HTTP client with connection pooling for API-heavy workloads
        let client = http_client::HttpClientBuilder::new()
            .with_llm_defaults()
            .request_timeout(DEFAULT_HTTP_REQUEST_TIMEOUT) // Override for HTTP tools (shorter than LLM default)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            method,
            name,
            description,
        }
    }

    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
        let timeout = request
            .timeout
            .map_or(DEFAULT_HTTP_REQUEST_TIMEOUT, Duration::from_secs);

        let mut req_builder = self
            .client
            .request(self.method.clone(), &request.url)
            .timeout(timeout);

        // Add headers
        for (key, value) in request.headers {
            req_builder = req_builder.header(key, value);
        }

        // Add body for POST, PUT, PATCH
        if matches!(
            self.method,
            reqwest::Method::POST | reqwest::Method::PUT | reqwest::Method::PATCH
        ) {
            if let Some(data) = request.data {
                req_builder = req_builder.json(&data);
            }
        }

        let response = req_builder.send().await?;

        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // Use size-limited read to prevent memory exhaustion from large responses
        let body =
            http_client::read_text_with_limit(response, http_client::DEFAULT_RESPONSE_SIZE_LIMIT)
                .await?;

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// HTTP GET request tool
///
/// Makes HTTP GET requests to retrieve data from APIs.
///
/// # Input Format
///
/// JSON string with the following fields:
/// - `url` (required): Target URL
/// - `headers` (optional): HTTP headers as key-value pairs
/// - `timeout` (optional): Request timeout in seconds (default: 30)
///
/// # Example
///
/// ```json
/// {
///   "url": "https://api.example.com/data",
///   "headers": {
///     "Authorization": "Bearer token123"
///   },
///   "timeout": 10
/// }
/// ```
pub struct HttpGetTool {
    base: BaseHttpTool,
}

impl HttpGetTool {
    /// Create a new HTTP GET tool
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: BaseHttpTool::new(
                reqwest::Method::GET,
                "http_get".to_string(),
                "Make HTTP GET requests to retrieve data from URLs. \
                 Input should be a JSON string with 'url' (required), \
                 'headers' (optional), and 'timeout' (optional) fields."
                    .to_string(),
            ),
        }
    }
}

impl Default for HttpGetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpGetTool {
    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    async fn _call(&self, input: ToolInput) -> Result<String> {
        let json_str = match input {
            ToolInput::String(s) => s,
            ToolInput::Structured(v) => serde_json::to_string(&v)?,
        };
        let request: HttpRequest = serde_json::from_str(&json_str)?;
        let response = self.base.execute(request).await?;
        Ok(serde_json::to_string_pretty(&response)?)
    }
}

/// HTTP POST request tool
///
/// Makes HTTP POST requests to send data to APIs.
///
/// # Input Format
///
/// JSON string with the following fields:
/// - `url` (required): Target URL
/// - `data` (optional): JSON data to send in request body
/// - `headers` (optional): HTTP headers as key-value pairs
/// - `timeout` (optional): Request timeout in seconds (default: 30)
///
/// # Example
///
/// ```json
/// {
///   "url": "https://api.example.com/create",
///   "data": {"name": "test", "value": 42},
///   "headers": {
///     "Content-Type": "application/json",
///     "Authorization": "Bearer token123"
///   }
/// }
/// ```
pub struct HttpPostTool {
    base: BaseHttpTool,
}

impl HttpPostTool {
    /// Create a new HTTP POST tool
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: BaseHttpTool::new(
                reqwest::Method::POST,
                "http_post".to_string(),
                "Make HTTP POST requests to send data to URLs. \
                 Input should be a JSON string with 'url' (required), \
                 'data' (optional JSON object), 'headers' (optional), \
                 and 'timeout' (optional) fields."
                    .to_string(),
            ),
        }
    }
}

impl Default for HttpPostTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpPostTool {
    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    async fn _call(&self, input: ToolInput) -> Result<String> {
        let json_str = match input {
            ToolInput::String(s) => s,
            ToolInput::Structured(v) => serde_json::to_string(&v)?,
        };
        let request: HttpRequest = serde_json::from_str(&json_str)?;
        let response = self.base.execute(request).await?;
        Ok(serde_json::to_string_pretty(&response)?)
    }
}

/// HTTP PUT request tool
///
/// Makes HTTP PUT requests to update resources at APIs.
///
/// # Input Format
///
/// JSON string with the following fields:
/// - `url` (required): Target URL
/// - `data` (optional): JSON data to send in request body
/// - `headers` (optional): HTTP headers as key-value pairs
/// - `timeout` (optional): Request timeout in seconds (default: 30)
pub struct HttpPutTool {
    base: BaseHttpTool,
}

impl HttpPutTool {
    /// Create a new HTTP PUT tool
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: BaseHttpTool::new(
                reqwest::Method::PUT,
                "http_put".to_string(),
                "Make HTTP PUT requests to update resources at URLs. \
                 Input should be a JSON string with 'url' (required), \
                 'data' (optional JSON object), 'headers' (optional), \
                 and 'timeout' (optional) fields."
                    .to_string(),
            ),
        }
    }
}

impl Default for HttpPutTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpPutTool {
    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    async fn _call(&self, input: ToolInput) -> Result<String> {
        let json_str = match input {
            ToolInput::String(s) => s,
            ToolInput::Structured(v) => serde_json::to_string(&v)?,
        };
        let request: HttpRequest = serde_json::from_str(&json_str)?;
        let response = self.base.execute(request).await?;
        Ok(serde_json::to_string_pretty(&response)?)
    }
}

/// HTTP PATCH request tool
///
/// Makes HTTP PATCH requests to partially update resources at APIs.
///
/// # Input Format
///
/// JSON string with the following fields:
/// - `url` (required): Target URL
/// - `data` (optional): JSON data to send in request body
/// - `headers` (optional): HTTP headers as key-value pairs
/// - `timeout` (optional): Request timeout in seconds (default: 30)
pub struct HttpPatchTool {
    base: BaseHttpTool,
}

impl HttpPatchTool {
    /// Create a new HTTP PATCH tool
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: BaseHttpTool::new(
                reqwest::Method::PATCH,
                "http_patch".to_string(),
                "Make HTTP PATCH requests to partially update resources at URLs. \
                 Input should be a JSON string with 'url' (required), \
                 'data' (optional JSON object), 'headers' (optional), \
                 and 'timeout' (optional) fields."
                    .to_string(),
            ),
        }
    }
}

impl Default for HttpPatchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpPatchTool {
    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    async fn _call(&self, input: ToolInput) -> Result<String> {
        let json_str = match input {
            ToolInput::String(s) => s,
            ToolInput::Structured(v) => serde_json::to_string(&v)?,
        };
        let request: HttpRequest = serde_json::from_str(&json_str)?;
        let response = self.base.execute(request).await?;
        Ok(serde_json::to_string_pretty(&response)?)
    }
}

/// HTTP DELETE request tool
///
/// Makes HTTP DELETE requests to remove resources at APIs.
///
/// # Input Format
///
/// JSON string with the following fields:
/// - `url` (required): Target URL
/// - `headers` (optional): HTTP headers as key-value pairs
/// - `timeout` (optional): Request timeout in seconds (default: 30)
pub struct HttpDeleteTool {
    base: BaseHttpTool,
}

impl HttpDeleteTool {
    /// Create a new HTTP DELETE tool
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: BaseHttpTool::new(
                reqwest::Method::DELETE,
                "http_delete".to_string(),
                "Make HTTP DELETE requests to remove resources at URLs. \
                 Input should be a JSON string with 'url' (required), \
                 'headers' (optional), and 'timeout' (optional) fields."
                    .to_string(),
            ),
        }
    }
}

impl Default for HttpDeleteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpDeleteTool {
    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    async fn _call(&self, input: ToolInput) -> Result<String> {
        let json_str = match input {
            ToolInput::String(s) => s,
            ToolInput::Structured(v) => serde_json::to_string(&v)?,
        };
        let request: HttpRequest = serde_json::from_str(&json_str)?;
        let response = self.base.execute(request).await?;
        Ok(serde_json::to_string_pretty(&response)?)
    }
}

#[cfg(test)]
mod tests {
    // `cargo verify` runs clippy with `-D warnings` for all targets, including unit tests.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    // =============================================================================
    // HTTP Tool Construction Tests
    // =============================================================================

    #[tokio::test]
    async fn test_http_get_tool_construction() {
        let tool = HttpGetTool::new();
        assert_eq!(tool.name(), "http_get");
        assert!(tool.description().contains("GET"));
    }

    #[tokio::test]
    async fn test_http_post_tool_construction() {
        let tool = HttpPostTool::new();
        assert_eq!(tool.name(), "http_post");
        assert!(tool.description().contains("POST"));
    }

    #[tokio::test]
    async fn test_http_put_tool_construction() {
        let tool = HttpPutTool::new();
        assert_eq!(tool.name(), "http_put");
        assert!(tool.description().contains("PUT"));
    }

    #[tokio::test]
    async fn test_http_patch_tool_construction() {
        let tool = HttpPatchTool::new();
        assert_eq!(tool.name(), "http_patch");
        assert!(tool.description().contains("PATCH"));
    }

    #[tokio::test]
    async fn test_http_delete_tool_construction() {
        let tool = HttpDeleteTool::new();
        assert_eq!(tool.name(), "http_delete");
        assert!(tool.description().contains("DELETE"));
    }

    // =============================================================================
    // Default Trait Tests
    // =============================================================================

    #[test]
    fn test_http_get_tool_default() {
        let tool = HttpGetTool::default();
        assert_eq!(tool.name(), "http_get");
    }

    #[test]
    fn test_http_post_tool_default() {
        let tool = HttpPostTool::default();
        assert_eq!(tool.name(), "http_post");
    }

    #[test]
    fn test_http_put_tool_default() {
        let tool = HttpPutTool::default();
        assert_eq!(tool.name(), "http_put");
    }

    #[test]
    fn test_http_patch_tool_default() {
        let tool = HttpPatchTool::default();
        assert_eq!(tool.name(), "http_patch");
    }

    #[test]
    fn test_http_delete_tool_default() {
        let tool = HttpDeleteTool::default();
        assert_eq!(tool.name(), "http_delete");
    }

    // =============================================================================
    // HttpRequest Deserialization Tests
    // =============================================================================

    #[tokio::test]
    async fn test_request_deserialization() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "headers": {
                "Authorization": "Bearer token123"
            },
            "timeout": 10
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request.url, "https://api.example.com/test");
        assert_eq!(
            request.headers.get("Authorization").unwrap(),
            "Bearer token123"
        );
        assert_eq!(request.timeout, Some(10));
    }

    #[tokio::test]
    async fn test_request_deserialization_minimal() {
        let json_str = json!({
            "url": "https://api.example.com/test"
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request.url, "https://api.example.com/test");
        assert!(request.headers.is_empty());
        assert_eq!(request.timeout, None);
        assert!(request.data.is_none());
    }

    #[tokio::test]
    async fn test_post_request_with_data() {
        let json_str = json!({
            "url": "https://api.example.com/create",
            "data": {
                "name": "test",
                "value": 42
            },
            "headers": {
                "Content-Type": "application/json"
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request.url, "https://api.example.com/create");
        assert!(request.data.is_some());
        let data = request.data.unwrap();
        assert_eq!(data["name"], "test");
        assert_eq!(data["value"], 42);
    }

    #[test]
    fn test_request_deserialization_multiple_headers() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "headers": {
                "Authorization": "Bearer token123",
                "Content-Type": "application/json",
                "Accept": "application/json",
                "X-Custom-Header": "custom-value"
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request.headers.len(), 4);
        assert_eq!(
            request.headers.get("Authorization").unwrap(),
            "Bearer token123"
        );
        assert_eq!(
            request.headers.get("Content-Type").unwrap(),
            "application/json"
        );
        assert_eq!(request.headers.get("Accept").unwrap(), "application/json");
        assert_eq!(
            request.headers.get("X-Custom-Header").unwrap(),
            "custom-value"
        );
    }

    #[test]
    fn test_request_deserialization_with_complex_data() {
        let json_str = json!({
            "url": "https://api.example.com/complex",
            "data": {
                "string_field": "hello",
                "number_field": 42,
                "float_field": 3.14,
                "bool_field": true,
                "null_field": null,
                "array_field": [1, 2, 3],
                "nested": {
                    "inner": "value"
                }
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        let data = request.data.unwrap();
        assert_eq!(data["string_field"], "hello");
        assert_eq!(data["number_field"], 42);
        assert!((data["float_field"].as_f64().unwrap() - 3.14).abs() < 0.001);
        assert_eq!(data["bool_field"], true);
        assert!(data["null_field"].is_null());
        assert_eq!(data["array_field"].as_array().unwrap().len(), 3);
        assert_eq!(data["nested"]["inner"], "value");
    }

    #[test]
    fn test_request_deserialization_with_empty_data() {
        let json_str = json!({
            "url": "https://api.example.com/empty",
            "data": {}
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert!(request.data.is_some());
        let data = request.data.unwrap();
        assert!(data.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_request_deserialization_with_array_data() {
        let json_str = json!({
            "url": "https://api.example.com/array",
            "data": [1, 2, 3, "four", {"five": 5}]
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        let data = request.data.unwrap();
        assert!(data.is_array());
        assert_eq!(data.as_array().unwrap().len(), 5);
    }

    #[test]
    fn test_request_deserialization_large_timeout() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "timeout": 3600
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request.timeout, Some(3600));
    }

    #[test]
    fn test_request_deserialization_zero_timeout() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "timeout": 0
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request.timeout, Some(0));
    }

    #[test]
    fn test_request_deserialization_empty_headers() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "headers": {}
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert!(request.headers.is_empty());
    }

    // =============================================================================
    // HttpRequest Serialization Tests
    // =============================================================================

    #[test]
    fn test_request_serialization() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token".to_string());

        let request = HttpRequest {
            url: "https://api.example.com/test".to_string(),
            headers,
            data: Some(json!({"key": "value"})),
            timeout: Some(30),
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let parsed: HttpRequest = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.url, request.url);
        assert_eq!(parsed.timeout, request.timeout);
        assert_eq!(
            parsed.headers.get("Authorization").unwrap(),
            "Bearer token"
        );
    }

    #[test]
    fn test_request_serialization_minimal() {
        let request = HttpRequest {
            url: "https://api.example.com/test".to_string(),
            headers: HashMap::new(),
            data: None,
            timeout: None,
        };

        let json_str = serde_json::to_string(&request).unwrap();
        assert!(json_str.contains("\"url\":\"https://api.example.com/test\""));
    }

    #[test]
    fn test_request_clone() {
        let mut headers = HashMap::new();
        headers.insert("X-Test".to_string(), "value".to_string());

        let request = HttpRequest {
            url: "https://api.example.com/test".to_string(),
            headers,
            data: Some(json!({"key": "value"})),
            timeout: Some(30),
        };

        let cloned = request.clone();
        assert_eq!(cloned.url, request.url);
        assert_eq!(cloned.headers, request.headers);
        assert_eq!(cloned.data, request.data);
        assert_eq!(cloned.timeout, request.timeout);
    }

    #[test]
    fn test_request_debug() {
        let request = HttpRequest {
            url: "https://api.example.com/test".to_string(),
            headers: HashMap::new(),
            data: None,
            timeout: None,
        };

        let debug_str = format!("{:?}", request);
        assert!(debug_str.contains("HttpRequest"));
        assert!(debug_str.contains("api.example.com"));
    }

    // =============================================================================
    // HttpResponse Serialization/Deserialization Tests
    // =============================================================================

    #[test]
    fn test_response_serialization() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let response = HttpResponse {
            status: 200,
            headers,
            body: r#"{"result": "success"}"#.to_string(),
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let parsed: HttpResponse = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.status, 200);
        assert_eq!(
            parsed.headers.get("Content-Type").unwrap(),
            "application/json"
        );
        assert_eq!(parsed.body, r#"{"result": "success"}"#);
    }

    #[test]
    fn test_response_deserialization() {
        let json_str = json!({
            "status": 404,
            "headers": {
                "Content-Type": "text/plain"
            },
            "body": "Not Found"
        })
        .to_string();

        let response: HttpResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response.status, 404);
        assert_eq!(response.body, "Not Found");
    }

    #[test]
    fn test_response_various_status_codes() {
        let status_codes = [100, 200, 201, 204, 301, 302, 400, 401, 403, 404, 500, 502, 503];

        for code in status_codes {
            let response = HttpResponse {
                status: code,
                headers: HashMap::new(),
                body: String::new(),
            };
            assert_eq!(response.status, code);
        }
    }

    #[test]
    fn test_response_empty_body() {
        let response = HttpResponse {
            status: 204,
            headers: HashMap::new(),
            body: String::new(),
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let parsed: HttpResponse = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.status, 204);
        assert!(parsed.body.is_empty());
    }

    #[test]
    fn test_response_multiple_headers() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("X-Request-Id".to_string(), "abc123".to_string());
        headers.insert(
            "Cache-Control".to_string(),
            "max-age=3600, public".to_string(),
        );

        let response = HttpResponse {
            status: 200,
            headers: headers.clone(),
            body: "{}".to_string(),
        };

        assert_eq!(response.headers.len(), 3);
        assert_eq!(
            response.headers.get("X-Request-Id").unwrap(),
            "abc123"
        );
    }

    #[test]
    fn test_response_clone() {
        let mut headers = HashMap::new();
        headers.insert("X-Test".to_string(), "value".to_string());

        let response = HttpResponse {
            status: 200,
            headers,
            body: "test body".to_string(),
        };

        let cloned = response.clone();
        assert_eq!(cloned.status, response.status);
        assert_eq!(cloned.headers, response.headers);
        assert_eq!(cloned.body, response.body);
    }

    #[test]
    fn test_response_debug() {
        let response = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: "test".to_string(),
        };

        let debug_str = format!("{:?}", response);
        assert!(debug_str.contains("HttpResponse"));
        assert!(debug_str.contains("200"));
    }

    // =============================================================================
    // Tool Description Content Tests
    // =============================================================================

    #[test]
    fn test_http_get_tool_description_content() {
        let tool = HttpGetTool::new();
        let desc = tool.description();
        assert!(desc.contains("GET"));
        assert!(desc.contains("url"));
        assert!(desc.contains("headers"));
    }

    #[test]
    fn test_http_post_tool_description_content() {
        let tool = HttpPostTool::new();
        let desc = tool.description();
        assert!(desc.contains("POST"));
        assert!(desc.contains("url"));
        assert!(desc.contains("data"));
    }

    #[test]
    fn test_http_put_tool_description_content() {
        let tool = HttpPutTool::new();
        let desc = tool.description();
        assert!(desc.contains("PUT"));
        assert!(desc.contains("url"));
        assert!(desc.contains("data"));
    }

    #[test]
    fn test_http_patch_tool_description_content() {
        let tool = HttpPatchTool::new();
        let desc = tool.description();
        assert!(desc.contains("PATCH"));
        assert!(desc.contains("url"));
        assert!(desc.contains("data"));
    }

    #[test]
    fn test_http_delete_tool_description_content() {
        let tool = HttpDeleteTool::new();
        let desc = tool.description();
        assert!(desc.contains("DELETE"));
        assert!(desc.contains("url"));
    }

    // =============================================================================
    // Error Handling Tests
    // =============================================================================

    #[test]
    fn test_request_deserialization_missing_url() {
        let json_str = json!({
            "headers": {}
        })
        .to_string();

        let result: std::result::Result<HttpRequest, _> = serde_json::from_str(&json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_request_deserialization_invalid_timeout_type() {
        let json_str = r#"{"url": "https://example.com", "timeout": "invalid"}"#;

        let result: std::result::Result<HttpRequest, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_request_deserialization_invalid_headers_type() {
        let json_str = r#"{"url": "https://example.com", "headers": "invalid"}"#;

        let result: std::result::Result<HttpRequest, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_request_deserialization_invalid_json() {
        let json_str = "not valid json {{{";

        let result: std::result::Result<HttpRequest, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tool_call_invalid_json() {
        let tool = HttpGetTool::new();
        let result = tool._call(ToolInput::String("not json".to_string())).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tool_call_missing_url() {
        let tool = HttpGetTool::new();
        let result = tool
            ._call(ToolInput::String(json!({"headers": {}}).to_string()))
            .await;
        assert!(result.is_err());
    }

    // =============================================================================
    // ToolInput Tests
    // =============================================================================

    #[tokio::test]
    async fn test_tool_input_structured_get() {
        let tool = HttpGetTool::new();
        let input = ToolInput::Structured(json!({
            "url": "http://invalid.test.local/api"
        }));

        // This should fail with connection error, not parsing error
        let result = tool._call(input).await;
        // Connection will fail but input parsing should succeed
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tool_input_structured_post() {
        let tool = HttpPostTool::new();
        let input = ToolInput::Structured(json!({
            "url": "http://invalid.test.local/api",
            "data": {"key": "value"}
        }));

        let result = tool._call(input).await;
        assert!(result.is_err()); // Connection error expected
    }

    #[tokio::test]
    async fn test_tool_input_structured_put() {
        let tool = HttpPutTool::new();
        let input = ToolInput::Structured(json!({
            "url": "http://invalid.test.local/api",
            "data": {"key": "value"}
        }));

        let result = tool._call(input).await;
        assert!(result.is_err()); // Connection error expected
    }

    #[tokio::test]
    async fn test_tool_input_structured_patch() {
        let tool = HttpPatchTool::new();
        let input = ToolInput::Structured(json!({
            "url": "http://invalid.test.local/api",
            "data": {"key": "value"}
        }));

        let result = tool._call(input).await;
        assert!(result.is_err()); // Connection error expected
    }

    #[tokio::test]
    async fn test_tool_input_structured_delete() {
        let tool = HttpDeleteTool::new();
        let input = ToolInput::Structured(json!({
            "url": "http://invalid.test.local/api"
        }));

        let result = tool._call(input).await;
        assert!(result.is_err()); // Connection error expected
    }

    // =============================================================================
    // URL Handling Tests
    // =============================================================================

    #[test]
    fn test_request_with_query_params() {
        let json_str = json!({
            "url": "https://api.example.com/search?q=rust&page=1&limit=10"
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("page=1"));
        assert!(request.url.contains("limit=10"));
    }

    #[test]
    fn test_request_with_encoded_url() {
        let json_str = json!({
            "url": "https://api.example.com/search?q=hello%20world"
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert!(request.url.contains("hello%20world"));
    }

    #[test]
    fn test_request_with_port() {
        let json_str = json!({
            "url": "http://localhost:8080/api/v1/users"
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert!(request.url.contains(":8080"));
    }

    #[test]
    fn test_request_with_fragment() {
        let json_str = json!({
            "url": "https://api.example.com/docs#section-1"
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert!(request.url.contains("#section-1"));
    }

    // =============================================================================
    // Special Header Values Tests
    // =============================================================================

    #[test]
    fn test_request_with_bearer_token() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "headers": {
                "Authorization": "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert!(request.headers.get("Authorization").unwrap().starts_with("Bearer "));
    }

    #[test]
    fn test_request_with_basic_auth() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "headers": {
                "Authorization": "Basic dXNlcm5hbWU6cGFzc3dvcmQ="
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert!(request.headers.get("Authorization").unwrap().starts_with("Basic "));
    }

    #[test]
    fn test_request_with_api_key_header() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "headers": {
                "X-API-Key": "sk-FAKE_TEST_KEY_0000"
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            request.headers.get("X-API-Key").unwrap(),
            "sk-FAKE_TEST_KEY_0000"
        );
    }

    #[test]
    fn test_request_with_content_type_variations() {
        let content_types = [
            "application/json",
            "application/xml",
            "text/plain",
            "text/html",
            "multipart/form-data",
            "application/x-www-form-urlencoded",
        ];

        for ct in content_types {
            let json_str = json!({
                "url": "https://api.example.com/test",
                "headers": {
                    "Content-Type": ct
                }
            })
            .to_string();

            let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
            assert_eq!(request.headers.get("Content-Type").unwrap(), ct);
        }
    }

    // =============================================================================
    // Special Data Value Tests
    // =============================================================================

    #[test]
    fn test_request_with_unicode_data() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "data": {
                "message": "Hello, 世界! 🎉",
                "name": "Müller"
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        let data = request.data.unwrap();
        assert_eq!(data["message"], "Hello, 世界! 🎉");
        assert_eq!(data["name"], "Müller");
    }

    #[test]
    fn test_request_with_escaped_strings() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "data": {
                "path": "C:\\Users\\test",
                "quote": "He said \"hello\"",
                "newline": "line1\nline2"
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        let data = request.data.unwrap();
        assert!(data["path"].as_str().unwrap().contains("\\"));
        assert!(data["quote"].as_str().unwrap().contains("\"hello\""));
        assert!(data["newline"].as_str().unwrap().contains('\n'));
    }

    #[test]
    fn test_request_with_large_numbers() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "data": {
                "big_int": 9007199254740991_i64,
                "big_float": 1.7976931348623157e308_f64,
                "small_float": 2.2250738585072014e-308_f64
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        let data = request.data.unwrap();
        assert!(data["big_int"].as_i64().is_some());
        assert!(data["big_float"].as_f64().is_some());
    }

    #[test]
    fn test_request_with_deeply_nested_data() {
        let json_str = json!({
            "url": "https://api.example.com/test",
            "data": {
                "level1": {
                    "level2": {
                        "level3": {
                            "level4": {
                                "value": "deep"
                            }
                        }
                    }
                }
            }
        })
        .to_string();

        let request: HttpRequest = serde_json::from_str(&json_str).unwrap();
        let data = request.data.unwrap();
        assert_eq!(
            data["level1"]["level2"]["level3"]["level4"]["value"],
            "deep"
        );
    }
}
