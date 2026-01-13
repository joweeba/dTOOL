//! Sensitive Data Redaction
//!
//! This module provides automatic PII and secret redaction for execution traces
//! and self-improvement data. It prevents sensitive information from being
//! persisted to disk or transmitted to external systems.
//!
//! ## Built-in Patterns
//!
//! The following patterns are detected and redacted by default:
//! - Email addresses
//! - Phone numbers (US format)
//! - Social Security Numbers
//! - Credit card numbers
//! - API keys (common formats)
//! - Bearer tokens
//! - AWS access keys
//! - Private keys (PEM format)
//! - Passwords in URLs
//!
//! ## Custom Patterns
//!
//! Users can add custom redaction patterns via `RedactionConfig`:
//!
//! ```rust,ignore
//! use dashflow::self_improvement::{RedactionConfig, SensitiveDataRedactor};
//!
//! let config = RedactionConfig::default()
//!     .with_custom_pattern("internal_id", r"INT-\d{8}", "[INTERNAL_ID]")
//!     .with_field_redaction("user.ssn")
//!     .with_disabled_pattern("email"); // Don't redact emails
//!
//! let redactor = SensitiveDataRedactor::new(config);
//! let clean_text = redactor.redact_string("Contact: user@example.com");
//! ```

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Built-in redaction patterns with their replacements
static BUILTIN_PATTERNS: LazyLock<Vec<BuiltinPattern>> = LazyLock::new(|| {
    vec![
        // Email addresses
        BuiltinPattern {
            name: "email",
            pattern: r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}",
            replacement: "[EMAIL]",
            description: "Email addresses",
        },
        // US Phone numbers (various formats)
        BuiltinPattern {
            name: "phone_us",
            pattern: r"\b(?:\+?1[-.\s]?)?(?:\(?\d{3}\)?[-.\s]?)?\d{3}[-.\s]?\d{4}\b",
            replacement: "[PHONE]",
            description: "US phone numbers",
        },
        // Social Security Numbers
        BuiltinPattern {
            name: "ssn",
            pattern: r"\b\d{3}[-\s]?\d{2}[-\s]?\d{4}\b",
            replacement: "[SSN]",
            description: "Social Security Numbers",
        },
        // Credit card numbers (basic patterns)
        BuiltinPattern {
            name: "credit_card",
            pattern: r"\b(?:4[0-9]{12}(?:[0-9]{3})?|5[1-5][0-9]{14}|3[47][0-9]{13}|6(?:011|5[0-9]{2})[0-9]{12})\b",
            replacement: "[CREDIT_CARD]",
            description: "Credit card numbers",
        },
        // Credit card with separators
        BuiltinPattern {
            name: "credit_card_sep",
            pattern: r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b",
            replacement: "[CREDIT_CARD]",
            description: "Credit card numbers with separators",
        },
        // API keys (generic patterns)
        BuiltinPattern {
            name: "api_key",
            pattern: r"(?i)\b(?:api[_-]?key|apikey)[=:\s]+['\x22]?[a-zA-Z0-9_-]{20,}['\x22]?",
            replacement: "[API_KEY]",
            description: "API keys",
        },
        // Bearer tokens
        BuiltinPattern {
            name: "bearer_token",
            pattern: r"[Bb]earer\s+[a-zA-Z0-9_.-]+",
            replacement: "Bearer [TOKEN]",
            description: "Bearer authentication tokens",
        },
        // AWS Access Key IDs
        BuiltinPattern {
            name: "aws_access_key",
            pattern: r"\b(?:AKIA|ABIA|ACCA|ASIA)[A-Z0-9]{16}\b",
            replacement: "[AWS_ACCESS_KEY]",
            description: "AWS Access Key IDs",
        },
        // AWS Secret Keys (40 char base64)
        BuiltinPattern {
            name: "aws_secret_key",
            pattern: r"\b[A-Za-z0-9/+=]{40}\b",
            replacement: "[AWS_SECRET]",
            description: "AWS Secret Access Keys",
        },
        // Private keys (PEM format start)
        BuiltinPattern {
            name: "private_key",
            pattern: r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----",
            replacement: "[PRIVATE_KEY_REDACTED]",
            description: "Private key headers",
        },
        // Passwords in URLs
        BuiltinPattern {
            name: "url_password",
            pattern: r"://[^:]+:([^@]+)@",
            replacement: "://[CREDENTIALS]@",
            description: "Passwords in URLs",
        },
        // Generic secret patterns
        BuiltinPattern {
            name: "generic_secret",
            pattern: r#"(?i)(?:password|passwd|pwd|secret|token)[=:\s]+['"]?([^\s'"]{8,})['"]?"#,
            replacement: "[REDACTED]",
            description: "Generic password/secret patterns",
        },
        // OpenAI API keys
        BuiltinPattern {
            name: "openai_key",
            pattern: r"\bsk-[a-zA-Z0-9]{20,}\b",
            replacement: "[OPENAI_KEY]",
            description: "OpenAI API keys",
        },
        // Anthropic API keys
        BuiltinPattern {
            name: "anthropic_key",
            pattern: r"\bsk-ant-[a-zA-Z0-9_-]{20,}\b",
            replacement: "[ANTHROPIC_KEY]",
            description: "Anthropic API keys",
        },
        // GitHub tokens
        BuiltinPattern {
            name: "github_token",
            pattern: r"\b(?:ghp|gho|ghu|ghs|ghr)_[a-zA-Z0-9]{36,}\b",
            replacement: "[GITHUB_TOKEN]",
            description: "GitHub tokens",
        },
        // Slack tokens
        BuiltinPattern {
            name: "slack_token",
            pattern: r"\bxox[baprs]-[a-zA-Z0-9-]+",
            replacement: "[SLACK_TOKEN]",
            description: "Slack tokens",
        },
        // IP addresses (optional - often not sensitive)
        BuiltinPattern {
            name: "ip_address",
            pattern: r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
            replacement: "[IP_ADDRESS]",
            description: "IPv4 addresses",
        },
    ]
});

/// A built-in redaction pattern
#[derive(Debug, Clone)]
struct BuiltinPattern {
    name: &'static str,
    pattern: &'static str,
    replacement: &'static str,
    description: &'static str,
}

/// Configuration for sensitive data redaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionConfig {
    /// Enable/disable redaction globally
    pub enabled: bool,

    /// Custom patterns to add (name -> (regex, replacement))
    #[serde(default)]
    pub custom_patterns: HashMap<String, CustomPattern>,

    /// Built-in patterns to disable
    #[serde(default)]
    pub disabled_patterns: HashSet<String>,

    /// Specific JSON field paths to always redact (e.g., "user.password", "config.api_key")
    #[serde(default)]
    pub redact_fields: HashSet<String>,

    /// Whether to redact IP addresses (disabled by default as often not sensitive)
    #[serde(default)]
    pub redact_ip_addresses: bool,

    /// Maximum length of redacted values to show (0 = hide completely)
    #[serde(default)]
    pub show_partial_length: usize,

    /// Placeholder for partial redaction (e.g., "***")
    #[serde(default = "default_partial_placeholder")]
    pub partial_placeholder: String,
}

fn default_partial_placeholder() -> String {
    "***".to_string()
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            custom_patterns: HashMap::new(),
            disabled_patterns: {
                let mut set = HashSet::new();
                // IP addresses disabled by default (often not PII)
                set.insert("ip_address".to_string());
                set
            },
            redact_fields: HashSet::new(),
            redact_ip_addresses: false,
            show_partial_length: 0,
            partial_placeholder: default_partial_placeholder(),
        }
    }
}

impl RedactionConfig {
    /// Create a new config with default settings
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Disable all redaction
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Enable strict mode (all patterns enabled, no partial values)
    #[must_use]
    pub fn strict() -> Self {
        Self {
            enabled: true,
            custom_patterns: HashMap::new(),
            disabled_patterns: HashSet::new(), // Enable all, including IP
            redact_fields: HashSet::new(),
            redact_ip_addresses: true,
            show_partial_length: 0,
            partial_placeholder: default_partial_placeholder(),
        }
    }

    /// Add a custom redaction pattern
    #[must_use]
    pub fn with_custom_pattern(
        mut self,
        name: impl Into<String>,
        pattern: impl Into<String>,
        replacement: impl Into<String>,
    ) -> Self {
        self.custom_patterns.insert(
            name.into(),
            CustomPattern {
                pattern: pattern.into(),
                replacement: replacement.into(),
            },
        );
        self
    }

    /// Add a field path to always redact
    #[must_use]
    pub fn with_field_redaction(mut self, field_path: impl Into<String>) -> Self {
        self.redact_fields.insert(field_path.into());
        self
    }

    /// Disable a built-in pattern
    #[must_use]
    pub fn with_disabled_pattern(mut self, pattern_name: impl Into<String>) -> Self {
        self.disabled_patterns.insert(pattern_name.into());
        self
    }

    /// Enable IP address redaction
    #[must_use]
    pub fn with_ip_redaction(mut self) -> Self {
        self.redact_ip_addresses = true;
        self.disabled_patterns.remove("ip_address");
        self
    }

    /// Show partial values (first N characters)
    #[must_use]
    pub fn with_partial_values(mut self, length: usize) -> Self {
        self.show_partial_length = length;
        self
    }
}

/// A custom redaction pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPattern {
    /// Regular expression pattern to match.
    pub pattern: String,
    /// Text to replace matches with.
    pub replacement: String,
}

/// Compiled redaction patterns for efficient matching
struct CompiledPatterns {
    patterns: Vec<(String, Regex, String)>, // (name, regex, replacement)
}

impl CompiledPatterns {
    fn new(config: &RedactionConfig) -> Self {
        let mut patterns = Vec::new();

        // Add built-in patterns (unless disabled)
        for builtin in BUILTIN_PATTERNS.iter() {
            if !config.disabled_patterns.contains(builtin.name) {
                match Regex::new(builtin.pattern) {
                    Ok(regex) => {
                        patterns.push((
                            builtin.name.to_string(),
                            regex,
                            builtin.replacement.to_string(),
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(
                            pattern_name = builtin.name,
                            pattern = builtin.pattern,
                            error = %e,
                            "Failed to compile builtin redaction pattern; pattern will be skipped"
                        );
                    }
                }
            }
        }

        // Add custom patterns
        for (name, custom) in &config.custom_patterns {
            match Regex::new(&custom.pattern) {
                Ok(regex) => {
                    patterns.push((name.clone(), regex, custom.replacement.clone()));
                }
                Err(e) => {
                    tracing::warn!(
                        pattern_name = %name,
                        pattern = %custom.pattern,
                        error = %e,
                        "Failed to compile custom redaction pattern; pattern will be skipped"
                    );
                }
            }
        }

        Self { patterns }
    }

    fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (_name, regex, replacement) in &self.patterns {
            result = regex.replace_all(&result, replacement.as_str()).to_string();
        }
        result
    }
}

/// Sensitive data redactor for execution traces and self-improvement data
pub struct SensitiveDataRedactor {
    config: RedactionConfig,
    compiled: CompiledPatterns,
}

impl Default for SensitiveDataRedactor {
    fn default() -> Self {
        Self::new(RedactionConfig::default())
    }
}

impl SensitiveDataRedactor {
    /// Create a new redactor with the given configuration
    #[must_use]
    pub fn new(config: RedactionConfig) -> Self {
        let compiled = CompiledPatterns::new(&config);
        Self { config, compiled }
    }

    /// Check if redaction is enabled
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Redact sensitive data from a string
    #[must_use]
    pub fn redact_string(&self, text: &str) -> String {
        if !self.config.enabled {
            return text.to_string();
        }
        self.compiled.redact(text)
    }

    /// Redact sensitive data from a JSON value
    #[must_use]
    pub fn redact_json(&self, value: &serde_json::Value) -> serde_json::Value {
        if !self.config.enabled {
            return value.clone();
        }
        self.redact_json_internal(value, &[])
    }

    fn redact_json_internal(
        &self,
        value: &serde_json::Value,
        path: &[String],
    ) -> serde_json::Value {
        // Check if this field path should be fully redacted
        let path_str = path.join(".");
        if self.config.redact_fields.contains(&path_str) {
            return serde_json::Value::String("[REDACTED]".to_string());
        }

        match value {
            serde_json::Value::String(s) => serde_json::Value::String(self.redact_string(s)),
            serde_json::Value::Object(obj) => {
                let mut new_obj = serde_json::Map::new();
                for (key, val) in obj {
                    let mut new_path = path.to_vec();
                    new_path.push(key.clone());
                    new_obj.insert(key.clone(), self.redact_json_internal(val, &new_path));
                }
                serde_json::Value::Object(new_obj)
            }
            serde_json::Value::Array(arr) => serde_json::Value::Array(
                arr.iter()
                    .map(|v| self.redact_json_internal(v, path))
                    .collect(),
            ),
            // Numbers, bools, nulls pass through unchanged
            other => other.clone(),
        }
    }

    /// Redact an ExecutionTrace in place
    pub fn redact_execution_trace(&self, trace: &mut crate::introspection::ExecutionTrace) {
        if !self.config.enabled {
            return;
        }

        // Redact final_state
        if let Some(ref state) = trace.final_state {
            trace.final_state = Some(self.redact_json(state));
        }

        // Redact metadata
        let redacted_metadata: std::collections::HashMap<String, serde_json::Value> = trace
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), self.redact_json(v)))
            .collect();
        trace.metadata = redacted_metadata;

        // Redact node executions
        for node in &mut trace.nodes_executed {
            self.redact_node_execution(node);
        }

        // Redact errors
        for error in &mut trace.errors {
            self.redact_error_trace(error);
        }
    }

    /// Redact a NodeExecution in place
    pub fn redact_node_execution(&self, node: &mut crate::introspection::trace::NodeExecution) {
        if !self.config.enabled {
            return;
        }

        // Redact state_before
        if let Some(ref state) = node.state_before {
            node.state_before = Some(self.redact_json(state));
        }

        // Redact state_after
        if let Some(ref state) = node.state_after {
            node.state_after = Some(self.redact_json(state));
        }

        // Redact error_message
        if let Some(ref msg) = node.error_message {
            node.error_message = Some(self.redact_string(msg));
        }

        // Redact metadata
        let redacted_metadata: std::collections::HashMap<String, serde_json::Value> = node
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), self.redact_json(v)))
            .collect();
        node.metadata = redacted_metadata;
    }

    /// Redact an ErrorTrace in place
    pub fn redact_error_trace(&self, error: &mut crate::introspection::trace::ErrorTrace) {
        if !self.config.enabled {
            return;
        }

        // Redact message
        error.message = self.redact_string(&error.message);

        // Redact context (stack trace)
        if let Some(ref ctx) = error.context {
            error.context = Some(self.redact_string(ctx));
        }

        // Redact state_at_error
        if let Some(ref state) = error.state_at_error {
            error.state_at_error = Some(self.redact_json(state));
        }

        // Redact metadata
        let redacted_metadata: std::collections::HashMap<String, serde_json::Value> = error
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), self.redact_json(v)))
            .collect();
        error.metadata = redacted_metadata;
    }

    /// Get a list of all available pattern names (built-in + custom)
    #[must_use]
    pub fn available_patterns(&self) -> Vec<&str> {
        let mut patterns: Vec<&str> = BUILTIN_PATTERNS.iter().map(|p| p.name).collect();
        for name in self.config.custom_patterns.keys() {
            patterns.push(name.as_str());
        }
        patterns
    }

    /// Get descriptions of built-in patterns
    #[must_use]
    pub fn pattern_descriptions() -> Vec<(&'static str, &'static str)> {
        BUILTIN_PATTERNS
            .iter()
            .map(|p| (p.name, p.description))
            .collect()
    }
}

/// Redaction statistics for monitoring
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedactionStats {
    /// Number of strings processed
    pub strings_processed: u64,
    /// Number of redactions performed
    pub redactions_performed: u64,
    /// Breakdown by pattern name
    pub by_pattern: HashMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_redaction() {
        let redactor = SensitiveDataRedactor::default();
        let input = "Contact us at user@example.com or admin@test.org";
        let output = redactor.redact_string(input);
        assert!(output.contains("[EMAIL]"));
        assert!(!output.contains("@example.com"));
        assert!(!output.contains("@test.org"));
    }

    #[test]
    fn test_phone_redaction() {
        let redactor = SensitiveDataRedactor::default();

        // Various phone formats
        let inputs = [
            "Call 555-123-4567",
            "Phone: (555) 123-4567",
            "Tel: +1-555-123-4567",
            "Mobile: 5551234567",
        ];

        for input in inputs {
            let output = redactor.redact_string(input);
            assert!(output.contains("[PHONE]"), "Failed for: {}", input);
        }
    }

    #[test]
    fn test_ssn_redaction() {
        let redactor = SensitiveDataRedactor::default();

        let inputs = ["SSN: 123-45-6789", "Social: 123 45 6789", "ID: 123456789"];

        for input in inputs {
            let output = redactor.redact_string(input);
            assert!(output.contains("[SSN]"), "Failed for: {}", input);
        }
    }

    #[test]
    fn test_credit_card_redaction() {
        let redactor = SensitiveDataRedactor::default();

        // Test Visa (16 digits starting with 4)
        let visa_input = "Card: 4111111111111111";
        let visa_output = redactor.redact_string(visa_input);
        assert!(
            visa_output.contains("[CREDIT_CARD]"),
            "Failed for Visa: {}",
            visa_input
        );

        // Test Mastercard (16 digits starting with 51-55)
        let mc_input = "CC: 5500000000000004";
        let mc_output = redactor.redact_string(mc_input);
        assert!(
            mc_output.contains("[CREDIT_CARD]"),
            "Failed for MC: {}",
            mc_input
        );

        // Test Amex (15 digits starting with 34 or 37)
        let amex_input = "AMEX: 371449635398431";
        let amex_output = redactor.redact_string(amex_input);
        assert!(
            amex_output.contains("[CREDIT_CARD]"),
            "Failed for AMEX: {}",
            amex_input
        );
    }

    #[test]
    fn test_api_key_redaction() {
        let redactor = SensitiveDataRedactor::default();

        let inputs = [
            "api_key=abc123def456ghi789jkl012mno345",
            "apiKey: 'abc123def456ghi789jkl012mno345'",
            "API-KEY=abc123def456ghi789jkl012mno345",
        ];

        for input in inputs {
            let output = redactor.redact_string(input);
            assert!(output.contains("[API_KEY]"), "Failed for: {}", input);
        }
    }

    #[test]
    fn test_bearer_token_redaction() {
        let redactor = SensitiveDataRedactor::default();
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let output = redactor.redact_string(input);
        assert!(output.contains("Bearer [TOKEN]"));
        assert!(!output.contains("eyJhbGciOiJIUzI1NiI"));
    }

    #[test]
    fn test_aws_key_redaction() {
        let redactor = SensitiveDataRedactor::default();

        let input = "AWS_ACCESS_KEY_ID=AKIAFAKETEST00000000";
        let output = redactor.redact_string(input);
        assert!(output.contains("[AWS_ACCESS_KEY]"));
    }

    #[test]
    fn test_openai_key_redaction() {
        let redactor = SensitiveDataRedactor::default();
        // Test pattern designed to trigger redaction without matching GitHub's secret scanner
        let input = "OPENAI_API_KEY=sk-proj-FAKE_TEST_KEY_aaaaaaaaaaaaaaaaaaaaaaaaa";
        let output = redactor.redact_string(input);
        assert!(output.contains("[OPENAI_KEY]"));
    }

    #[test]
    fn test_anthropic_key_redaction() {
        let redactor = SensitiveDataRedactor::default();
        // Test pattern designed to trigger redaction without matching GitHub's secret scanner
        let input = "ANTHROPIC_API_KEY=sk-ant-api03-FAKE_TEST_aaaaaaaaaaaaaaaaaaa";
        let output = redactor.redact_string(input);
        assert!(output.contains("[ANTHROPIC_KEY]"));
    }

    #[test]
    fn test_github_token_redaction() {
        let redactor = SensitiveDataRedactor::default();
        // GitHub tokens have ghp/gho/ghu/ghs/ghr prefix followed by 36+ alphanumeric chars
        // Use "GH_ACCESS" to avoid triggering generic_secret pattern on "TOKEN"
        // Test pattern: ghp_ + 40 chars triggers GitHub token detection
        let input = "GH_ACCESS=ghp_FAKE0TEST0TOKEN0FOR0UNIT0TESTING000000";
        let output = redactor.redact_string(input);
        assert!(output.contains("[GITHUB_TOKEN]"), "Output was: {}", output);
    }

    #[test]
    fn test_url_password_redaction() {
        let redactor = SensitiveDataRedactor::default();
        let input = "postgresql://user:secretpassword@localhost:5432/db";
        let output = redactor.redact_string(input);
        assert!(output.contains("[CREDENTIALS]@"));
        assert!(!output.contains("secretpassword"));
    }

    #[test]
    fn test_json_redaction() {
        let redactor = SensitiveDataRedactor::default();
        let json = serde_json::json!({
            "user": {
                "name": "John",
                "email": "john@example.com",
                "phone": "555-123-4567"
            },
            "api_key": "sk-FAKE_TEST_KEY_abcdefghi0000000000"
        });

        let redacted = redactor.redact_json(&json);
        let redacted_str = serde_json::to_string(&redacted).unwrap();

        assert!(redacted_str.contains("[EMAIL]"));
        assert!(redacted_str.contains("[PHONE]"));
        assert!(redacted_str.contains("[OPENAI_KEY]"));
        assert!(!redacted_str.contains("john@example.com"));
    }

    #[test]
    fn test_field_redaction() {
        let config = RedactionConfig::default()
            .with_field_redaction("user.password")
            .with_field_redaction("config.secret");

        let redactor = SensitiveDataRedactor::new(config);

        let json = serde_json::json!({
            "user": {
                "name": "John",
                "password": "not_a_secret_pattern"
            },
            "config": {
                "secret": "also_not_matching_patterns",
                "other": "value"
            }
        });

        let redacted = redactor.redact_json(&json);

        assert_eq!(redacted["user"]["password"], "[REDACTED]");
        assert_eq!(redacted["config"]["secret"], "[REDACTED]");
        assert_eq!(redacted["config"]["other"], "value");
    }

    #[test]
    fn test_custom_pattern() {
        // Use a pattern that won't conflict with built-in patterns
        let config = RedactionConfig::default().with_custom_pattern(
            "project_id",
            r"PROJ-[A-Z]{4}",
            "[PROJECT]",
        );

        let redactor = SensitiveDataRedactor::new(config);
        let input = "Record: PROJ-ABCD created";
        let output = redactor.redact_string(input);

        assert!(output.contains("[PROJECT]"), "Output was: {}", output);
        assert!(
            !output.contains("PROJ-ABCD"),
            "PROJ-ABCD still present in: {}",
            output
        );
    }

    #[test]
    fn test_disabled_pattern() {
        let config = RedactionConfig::default().with_disabled_pattern("email");

        let redactor = SensitiveDataRedactor::new(config);
        let input = "Contact: user@example.com";
        let output = redactor.redact_string(input);

        // Email should NOT be redacted
        assert!(output.contains("user@example.com"));
    }

    #[test]
    fn test_disabled_redaction() {
        let config = RedactionConfig::disabled();
        let redactor = SensitiveDataRedactor::new(config);

        let input = "user@example.com 555-123-4567 sk-abc123def456";
        let output = redactor.redact_string(input);

        // Nothing should be redacted
        assert_eq!(input, output);
    }

    #[test]
    fn test_strict_mode() {
        let config = RedactionConfig::strict();
        let redactor = SensitiveDataRedactor::new(config);

        // IP addresses should be redacted in strict mode
        let input = "Server at 192.168.1.100";
        let output = redactor.redact_string(input);
        assert!(output.contains("[IP_ADDRESS]"));
    }

    #[test]
    fn test_available_patterns() {
        let redactor = SensitiveDataRedactor::default();
        let patterns = redactor.available_patterns();

        assert!(patterns.contains(&"email"));
        assert!(patterns.contains(&"phone_us"));
        assert!(patterns.contains(&"ssn"));
        assert!(patterns.contains(&"credit_card"));
    }

    #[test]
    fn test_pattern_descriptions() {
        let descriptions = SensitiveDataRedactor::pattern_descriptions();
        assert!(!descriptions.is_empty());

        let email_desc = descriptions.iter().find(|(name, _)| *name == "email");
        assert!(email_desc.is_some());
    }

    #[test]
    fn test_multiple_patterns_in_one_string() {
        let redactor = SensitiveDataRedactor::default();
        let input = "User user@example.com called 555-123-4567 with card 4111111111111111";
        let output = redactor.redact_string(input);

        assert!(output.contains("[EMAIL]"));
        assert!(output.contains("[PHONE]"));
        assert!(output.contains("[CREDIT_CARD]"));
    }

    #[test]
    fn test_nested_json_array() {
        let redactor = SensitiveDataRedactor::default();
        let json = serde_json::json!({
            "contacts": [
                {"email": "a@example.com"},
                {"email": "b@example.com"}
            ]
        });

        let redacted = redactor.redact_json(&json);
        let arr = redacted["contacts"].as_array().unwrap();

        assert_eq!(arr[0]["email"], "[EMAIL]");
        assert_eq!(arr[1]["email"], "[EMAIL]");
    }
}
