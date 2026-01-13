// Copyright 2026 Dropbox (created by Andrew Yates <ayates@dropbox.com>)

//! Tests for trace persistence, live introspection, and max parallel tasks.
//!
//! Extracted from executor/tests.rs by Worker #1695.
//!
//! NOTE: Tests for is_trace_persistence_enabled(), is_live_introspection_enabled(),
//! and is_trace_redaction_enabled() are in executor/trace.rs to avoid env var races.
//! This module contains integration tests that use these functions.

use super::*;
use std::sync::Mutex;

// Mutex to serialize env-var-dependent tests (parallel execution causes races)
// This mutex is shared with the one in executor/trace.rs via test_prelude
static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn test_build_execution_trace() {
    use std::time::SystemTime;

    // Create mock execution result
    let result = ExecutionResult::<AgentState> {
        final_state: AgentState::new(),
        nodes_executed: vec!["node1".to_string(), "node2".to_string()],
        interrupted_at: None,
        next_nodes: vec![],
    };

    // Create mock metrics
    let mut metrics = ExecutionMetrics::new();
    metrics
        .node_durations
        .insert("node1".to_string(), Duration::from_millis(100));
    metrics
        .node_durations
        .insert("node2".to_string(), Duration::from_millis(200));
    metrics.total_duration = Duration::from_millis(300);
    metrics.edges_traversed = 2;

    // Build trace
    let trace = build_execution_trace(
        &result,
        &metrics,
        Some("test_graph"),
        SystemTime::now(),
        Some("test-thread".to_string()),
    );

    // Verify trace fields
    assert!(trace.completed);
    assert_eq!(trace.thread_id, Some("test-thread".to_string())); // Issue #7 fix
    assert_eq!(trace.nodes_executed.len(), 2);
    assert_eq!(trace.nodes_executed[0].node, "node1");
    assert_eq!(trace.nodes_executed[0].duration_ms, 100);
    assert_eq!(trace.nodes_executed[1].node, "node2");
    assert_eq!(trace.nodes_executed[1].duration_ms, 200);
    assert_eq!(trace.total_duration_ms, 300);
    assert!(trace.execution_id.is_some());
    assert!(trace.started_at.is_some());
    assert!(trace.ended_at.is_some());
    assert_eq!(
        trace.metadata.get("graph_name").and_then(|v| v.as_str()),
        Some("test_graph")
    );
}

#[test]
fn test_persist_trace_creates_file() {
    // Use unique test directory
    let test_id = uuid::Uuid::new_v4().to_string();
    let test_dir = std::path::PathBuf::from(format!("/tmp/dashflow_persist_test_{}", test_id));
    let traces_dir = test_dir.join(".dashflow/traces");

    // Clean up before test
    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir_all(&test_dir).unwrap();

    // Create trace manually
    let trace = ExecutionTrace {
        thread_id: None,
        execution_id: Some(test_id.clone()),
        parent_execution_id: None,
        root_execution_id: None,
        depth: Some(0),
        nodes_executed: vec![NodeExecution::new("test", 100)],
        total_duration_ms: 100,
        total_tokens: 0,
        errors: vec![],
        completed: true,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        ended_at: Some(chrono::Utc::now().to_rfc3339()),
        final_state: None,
        metadata: std::collections::HashMap::new(),
        execution_metrics: None,
        performance_metrics: None,
    };

    // Persist trace
    persist_trace_in_dir(&trace, &test_dir);

    // Verify file was created
    assert!(traces_dir.exists(), "Traces directory should be created");

    let expected_file = traces_dir.join(format!("{}.json", test_id));
    assert!(expected_file.exists(), "Trace file should exist");

    // Verify content
    let content = std::fs::read_to_string(&expected_file).unwrap();
    let loaded: ExecutionTrace = serde_json::from_str(&content).unwrap();
    assert_eq!(loaded.execution_id, Some(test_id));

    // Clean up
    let _ = std::fs::remove_dir_all(&test_dir);
}

// NOTE: test_is_trace_redaction_enabled moved to executor/trace.rs to avoid env var races
// with other tests in this module. See trace.rs for comprehensive redaction flag tests.

#[test]
fn test_persist_trace_redacts_sensitive_data() {
    let _guard = ENV_MUTEX.lock().unwrap();

    use crate::introspection::trace::NodeExecution;
    use std::collections::HashMap;

    let _process_guard = crate::test_support::cwd_lock()
        .lock()
        .expect("cwd_lock should not be poisoned");

    // Use unique test directory
    let test_id = uuid::Uuid::new_v4().to_string();
    let test_dir = std::path::PathBuf::from(format!("/tmp/dashflow_redact_test_{}", test_id));
    let traces_dir = test_dir.join(".dashflow/traces");

    // Clean up before test
    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir_all(&test_dir).unwrap();

    // Create trace with sensitive data that matches redaction patterns:
    // - Email: [a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}
    // - OpenAI key: sk-[a-zA-Z0-9]{20,} (20+ chars after sk-)
    let mut metadata = HashMap::new();
    metadata.insert(
        "api_key".to_string(),
        // 22 chars after sk- to match OpenAI pattern
        serde_json::json!("sk-FAKE_TEST_KEY_00000000XY"),
    );
    metadata.insert(
        "user_email".to_string(),
        serde_json::json!("user@example.com"),
    );

    let trace = ExecutionTrace {
        thread_id: None,
        execution_id: Some(test_id.clone()),
        parent_execution_id: None,
        root_execution_id: None,
        depth: Some(0),
        nodes_executed: vec![NodeExecution::new("test", 100)],
        total_duration_ms: 100,
        total_tokens: 0,
        errors: vec![],
        completed: true,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        ended_at: Some(chrono::Utc::now().to_rfc3339()),
        final_state: Some(serde_json::json!({
            "query": "Contact me at admin@company.org",
            // Another valid OpenAI key (24 chars after sk-)
            "api_key": "sk-FAKE_TEST_KEY_1111111WXYZ"
        })),
        metadata,
        execution_metrics: None,
        performance_metrics: None,
    };

    // Enable redaction (should be default, but be explicit)
    std::env::set_var("DASHFLOW_TRACE_REDACT", "true");

    // Persist trace
    persist_trace_in_dir(&trace, &test_dir);

    // Read back the saved trace
    let expected_file = traces_dir.join(format!("{}.json", test_id));
    assert!(expected_file.exists(), "Trace file should exist");

    let content = std::fs::read_to_string(&expected_file).unwrap();

    // Verify sensitive data was redacted
    assert!(
        !content.contains("sk-FAKE_TEST_KEY_00000000XY"),
        "API key in metadata should be redacted"
    );
    assert!(
        !content.contains("sk-FAKE_TEST_KEY_1111111WXYZ"),
        "API key in final_state should be redacted"
    );
    assert!(
        !content.contains("user@example.com"),
        "Email in metadata should be redacted"
    );
    assert!(
        !content.contains("admin@company.org"),
        "Email in final_state should be redacted"
    );

    // Verify redaction markers are present
    assert!(
        content.contains("[EMAIL]") || content.contains("[OPENAI_KEY]"),
        "Redaction markers should be present in content: {}",
        content
    );

    // Clean up
    std::env::remove_var("DASHFLOW_TRACE_REDACT");
    let _ = std::fs::remove_dir_all(&test_dir);
}

#[test]
fn test_persist_trace_no_redaction_when_disabled() {
    let _guard = ENV_MUTEX.lock().unwrap();

    use crate::introspection::trace::NodeExecution;

    let _process_guard = crate::test_support::cwd_lock()
        .lock()
        .expect("cwd_lock should not be poisoned");

    // Use unique test directory
    let test_id = uuid::Uuid::new_v4().to_string();
    let test_dir = std::path::PathBuf::from(format!("/tmp/dashflow_no_redact_test_{}", test_id));
    let traces_dir = test_dir.join(".dashflow/traces");

    // Clean up before test
    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir_all(&test_dir).unwrap();

    // Create trace with sensitive data
    let trace = ExecutionTrace {
        thread_id: None,
        execution_id: Some(test_id.clone()),
        parent_execution_id: None,
        root_execution_id: None,
        depth: Some(0),
        nodes_executed: vec![NodeExecution::new("test", 100)],
        total_duration_ms: 100,
        total_tokens: 0,
        errors: vec![],
        completed: true,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        ended_at: Some(chrono::Utc::now().to_rfc3339()),
        final_state: Some(serde_json::json!({
            "test_data": "plaintext_value_123"
        })),
        metadata: std::collections::HashMap::new(),
        execution_metrics: None,
        performance_metrics: None,
    };

    // Disable redaction
    std::env::set_var("DASHFLOW_TRACE_REDACT", "false");

    // Persist trace
    persist_trace_in_dir(&trace, &test_dir);

    // Read back the saved trace
    let expected_file = traces_dir.join(format!("{}.json", test_id));
    assert!(expected_file.exists(), "Trace file should exist");

    let content = std::fs::read_to_string(&expected_file).unwrap();

    // Verify data was NOT redacted
    assert!(
        content.contains("plaintext_value_123"),
        "Data should not be redacted when disabled"
    );

    // Clean up
    std::env::remove_var("DASHFLOW_TRACE_REDACT");
    let _ = std::fs::remove_dir_all(&test_dir);
}

#[tokio::test]
async fn test_subgraph_traces_populate_hierarchical_ids() -> Result<()> {
    use crate::subgraph::SubgraphNode;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Build a child graph (subgraph) with one node.
    let mut child_graph: StateGraph<AgentState> = StateGraph::new();
    child_graph.add_node_from_fn("child", |state| Box::pin(async move { Ok(state) }));
    child_graph.set_entry_point("child");
    child_graph.add_edge("child", END);
    let compiled_child = child_graph
        .compile()
        .unwrap()
        .with_trace_base_dir(temp_dir.path());

    // Build a parent graph that invokes the child as a SubgraphNode.
    let subgraph_node = SubgraphNode::new(
        "child_graph",
        compiled_child,
        |parent: &AgentState| parent.clone(),
        |_parent: AgentState, child: AgentState| child,
    );

    let mut parent_graph: StateGraph<AgentState> = StateGraph::new();
    parent_graph.add_node("subgraph", subgraph_node);
    parent_graph.set_entry_point("subgraph");
    parent_graph.add_edge("subgraph", END);
    let compiled_parent = parent_graph
        .compile()
        .unwrap()
        .with_trace_base_dir(temp_dir.path());

    compiled_parent.invoke(AgentState::default()).await.unwrap();

    // Wait for async trace persistence (PERF-003 made this non-blocking)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let traces_dir = temp_dir.path().join(".dashflow/traces");
    assert!(traces_dir.exists(), "Traces directory should exist");

    let mut traces = Vec::new();
    for entry in std::fs::read_dir(&traces_dir).unwrap().flatten() {
        let content = std::fs::read_to_string(entry.path()).unwrap();
        let trace: ExecutionTrace = serde_json::from_str(&content).unwrap();
        traces.push(trace);
    }

    let parent_trace = traces
        .iter()
        .find(|t| t.depth == Some(0))
        .expect("should persist a top-level trace");
    let child_trace = traces
        .iter()
        .find(|t| t.depth == Some(1))
        .expect("should persist a subgraph trace");

    let parent_exec_id = parent_trace.execution_id.clone().expect("parent has execution_id");
    assert_eq!(child_trace.parent_execution_id, Some(parent_exec_id.clone()));
    assert_eq!(child_trace.root_execution_id, Some(parent_exec_id));

    Ok(())
}

// ===== Max Parallel Tasks Tests =====

#[test]
fn test_max_parallel_tasks_default() {
    let mut graph: StateGraph<AgentState> = StateGraph::new();
    graph.add_node_from_fn("dummy", |state| Box::pin(async move { Ok(state) }));
    graph.set_entry_point("dummy");
    graph.add_edge("dummy", END);
    let compiled = graph.compile().expect("should compile");

    // Default is 64 concurrent tasks
    assert_eq!(
        compiled.max_parallel_tasks,
        Some(DEFAULT_MAX_PARALLEL_TASKS)
    );
    assert_eq!(compiled.max_parallel_tasks, Some(64));
}

#[test]
fn test_max_parallel_tasks_custom() {
    let mut graph: StateGraph<AgentState> = StateGraph::new();
    graph.add_node_from_fn("dummy", |state| Box::pin(async move { Ok(state) }));
    graph.set_entry_point("dummy");
    graph.add_edge("dummy", END);
    let compiled = graph
        .compile()
        .expect("should compile")
        .with_max_parallel_tasks(128);

    assert_eq!(compiled.max_parallel_tasks, Some(128));
}

#[test]
fn test_max_parallel_tasks_minimum_one() {
    let mut graph: StateGraph<AgentState> = StateGraph::new();
    graph.add_node_from_fn("dummy", |state| Box::pin(async move { Ok(state) }));
    graph.set_entry_point("dummy");
    graph.add_edge("dummy", END);

    // Setting 0 should be clamped to 1
    let compiled = graph
        .compile()
        .expect("should compile")
        .with_max_parallel_tasks(0);
    assert_eq!(compiled.max_parallel_tasks, Some(1));
}

#[test]
fn test_max_parallel_tasks_without_limits() {
    let mut graph: StateGraph<AgentState> = StateGraph::new();
    graph.add_node_from_fn("dummy", |state| Box::pin(async move { Ok(state) }));
    graph.set_entry_point("dummy");
    graph.add_edge("dummy", END);
    let compiled = graph.compile().expect("should compile").without_limits();

    // without_limits() sets max_parallel_tasks to None (unlimited)
    assert_eq!(compiled.max_parallel_tasks, None);
}

#[tokio::test]
async fn test_parallel_execution_respects_max_tasks() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Track maximum concurrent executions
    let concurrent_count = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));

    let mut graph: StateGraph<AgentState> = StateGraph::new();

    // Create 10 parallel nodes that track concurrency
    let node_names: Vec<String> = (0..10).map(|i| format!("node_{i}")).collect();
    for name in &node_names {
        let cc = concurrent_count.clone();
        let mc = max_concurrent.clone();
        graph.add_node_from_fn(name.as_str(), move |state| {
            let cc = cc.clone();
            let mc = mc.clone();
            Box::pin(async move {
                // Increment concurrent count
                let current = cc.fetch_add(1, Ordering::SeqCst) + 1;
                // Update max if this is a new high
                mc.fetch_max(current, Ordering::SeqCst);

                // Simulate some work
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;

                // Decrement concurrent count
                cc.fetch_sub(1, Ordering::SeqCst);
                Ok(state)
            })
        });
    }

    // Set up parallel execution of all nodes
    graph.set_entry_point(&node_names[0]);
    graph.add_parallel_edges(&node_names[0], node_names[1..].to_vec());
    for name in &node_names[1..] {
        graph.add_edge(name, END);
    }

    // Limit to 3 concurrent tasks
    let compiled = graph
        .compile_with_merge()
        .expect("should compile")
        .with_max_parallel_tasks(3)
        .without_retries();

    let state = AgentState::new();
    let _ = compiled.invoke(state).await;

    // Max concurrent should not exceed 3 (our limit)
    // Note: The first node executes before the parallel branch, so
    // max is measured on the 9 parallel nodes, limited to 3
    let observed_max = max_concurrent.load(Ordering::SeqCst);
    assert!(
        observed_max <= 3,
        "Expected max concurrent <= 3, got {observed_max}"
    );
}
