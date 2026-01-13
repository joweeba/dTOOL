//! Basic example demonstrating MongoDB Atlas Vector Search integration.
//!
//! This example shows how to:
//! - Create a MongoDB vector store
//! - Add documents with embeddings
//! - Perform similarity search
//! - Filter by metadata
//! - Delete documents
//!
//! # Requirements
//!
//! 1. MongoDB Atlas cluster (M10 or higher) with Vector Search enabled
//! 2. Atlas Search index configured (see README.md for configuration)
//! 3. Set environment variable: MONGODB_CONNECTION_STRING
//!
//! # Running
//!
//! ```bash
//! export MONGODB_CONNECTION_STRING="mongodb+srv://FAKE_USER:FAKE_PASS@cluster.example.net"
//! cargo run --example mongodb_basic
//! ```

use dashflow::core::embeddings::Embeddings;
use dashflow::core::vector_stores::VectorStore;
use dashflow_mongodb::MongoDBVectorStore;
use std::collections::HashMap;
use std::sync::Arc;

/// Mock embeddings for demonstration (replace with real embeddings in production)
struct MockEmbeddings;

#[async_trait::async_trait]
impl Embeddings for MockEmbeddings {
    async fn _embed_documents(&self, texts: &[String]) -> dashflow::core::Result<Vec<Vec<f32>>> {
        // Generate deterministic fake embeddings for demonstration
        // In production, use real embeddings (OpenAI, Cohere, etc.)
        Ok(texts
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let base = (text.len() as f32 / 10.0).sin();
                (0..1536)
                    .map(|j| base + (i as f32 * 0.1) + (j as f32 * 0.001))
                    .collect()
            })
            .collect())
    }

    async fn _embed_query(&self, text: &str) -> dashflow::core::Result<Vec<f32>> {
        // Generate deterministic fake query embedding
        let base = (text.len() as f32 / 10.0).sin();
        Ok((0..1536).map(|j| base + (j as f32 * 0.001)).collect())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("MongoDB Atlas Vector Search Example\n");

    // Get connection string from environment
    let connection_string = std::env::var("MONGODB_CONNECTION_STRING")
        .unwrap_or_else(|_| "mongodb://localhost:27017".to_string());

    println!("Connecting to MongoDB...");
    let embeddings: Arc<dyn Embeddings> = Arc::new(MockEmbeddings);

    // Create vector store
    let mut store = MongoDBVectorStore::new(
        &connection_string,
        "dashflow_test",       // database name
        "vector_documents",    // collection name
        "vector_search_index", // Atlas Search index name (must be created manually)
        embeddings,
    )
    .await?;

    println!("✓ Connected to MongoDB\n");

    // Example 1: Add documents without metadata
    println!("1. Adding documents without metadata...");
    let texts = vec![
        "The quick brown fox jumps over the lazy dog",
        "Machine learning is a subset of artificial intelligence",
        "Rust is a systems programming language focused on safety",
        "Vector databases enable semantic search over embeddings",
    ];

    let ids = store.add_texts(&texts, None, None).await?;
    println!("   Added {} documents with IDs: {:?}\n", ids.len(), ids);

    // Example 2: Add documents with metadata
    println!("2. Adding documents with metadata...");
    let texts_with_meta = vec![
        "Python is widely used for data science and machine learning",
        "MongoDB is a NoSQL database with flexible schema design",
    ];

    let metadatas = vec![
        {
            let mut meta = HashMap::new();
            meta.insert("category".to_string(), serde_json::json!("programming"));
            meta.insert("language".to_string(), serde_json::json!("python"));
            meta.insert("year".to_string(), serde_json::json!(2024));
            meta
        },
        {
            let mut meta = HashMap::new();
            meta.insert("category".to_string(), serde_json::json!("database"));
            meta.insert("type".to_string(), serde_json::json!("nosql"));
            meta.insert("year".to_string(), serde_json::json!(2024));
            meta
        },
    ];

    let ids_with_meta = store
        .add_texts(&texts_with_meta, Some(&metadatas), None)
        .await?;
    println!("   Added {} documents with metadata\n", ids_with_meta.len());

    // Example 3: Similarity search
    println!("3. Similarity search (no filter)...");
    let query = "What is machine learning?";
    let results = store._similarity_search(query, 3, None).await?;

    println!("   Query: '{}'\n   Results:", query);
    for (i, doc) in results.iter().enumerate() {
        println!(
            "   {}. {} (ID: {})",
            i + 1,
            &doc.page_content[..60.min(doc.page_content.len())],
            doc.id.as_deref().unwrap_or("N/A")
        );
    }
    println!();

    // Example 4: Similarity search with scores
    println!("4. Similarity search with relevance scores...");
    let results_with_scores = store.similarity_search_with_score(query, 3, None).await?;

    println!("   Query: '{}'\n   Results:", query);
    for (i, (doc, score)) in results_with_scores.iter().enumerate() {
        println!(
            "   {}. [Score: {:.4}] {}",
            i + 1,
            score,
            &doc.page_content[..60.min(doc.page_content.len())]
        );
    }
    println!();

    // Example 5: Filtered similarity search
    println!("5. Filtered similarity search (category = 'programming')...");
    let mut filter = HashMap::new();
    filter.insert("category".to_string(), serde_json::json!("programming"));

    let filtered_results = store
        ._similarity_search("programming languages", 5, Some(&filter))
        .await?;

    println!("   Results with category filter:");
    for (i, doc) in filtered_results.iter().enumerate() {
        println!(
            "   {}. {} (category: {})",
            i + 1,
            &doc.page_content[..60.min(doc.page_content.len())],
            doc.metadata
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("N/A")
        );
    }
    println!();

    // Example 6: Get documents by ID
    println!("6. Retrieving documents by ID...");
    let retrieved_docs = store.get_by_ids(&ids[0..2]).await?;
    println!("   Retrieved {} documents:", retrieved_docs.len());
    for doc in &retrieved_docs {
        println!(
            "   - {} (ID: {})",
            &doc.page_content[..40.min(doc.page_content.len())],
            doc.id.as_deref().unwrap_or("N/A")
        );
    }
    println!();

    // Example 7: Delete documents
    println!("7. Deleting documents...");
    let deleted = store.delete(Some(&ids[0..1])).await?;
    println!("   Deleted documents: {}\n", deleted);

    // Verify deletion
    let remaining_docs = store.get_by_ids(&ids[0..1]).await?;
    println!(
        "   Verification: {} documents remain with that ID",
        remaining_docs.len()
    );

    println!("\n✓ Example completed successfully!");
    println!("\nNote: MongoDB Atlas Vector Search requires:");
    println!("  1. Atlas cluster (M10+) with Vector Search enabled");
    println!("  2. Atlas Search index configured for vector search");
    println!("  3. Index definition matching embedding dimensions (1536 for this example)");
    println!("\nSee README.md for complete setup instructions.");

    Ok(())
}
