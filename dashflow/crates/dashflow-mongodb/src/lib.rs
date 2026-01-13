//! `MongoDB` Atlas Vector Search integration for `DashFlow` Rust.
//!
//! This crate provides a vector store implementation using `MongoDB` Atlas Vector Search,
//! enabling efficient similarity search over large collections of embeddings.
//!
//! # Features
//!
//! - Full `VectorStore` trait implementation
//! - Vector similarity search using `MongoDB` Atlas Vector Search
//! - Multiple distance metrics (cosine, euclidean, dot product)
//! - JSONB metadata storage and filtering
//! - CRUD operations (create, read, update, delete)
//! - Automatic index management
//!
//! # Requirements
//!
//! - `MongoDB` Atlas cluster (M10 or higher) with Vector Search enabled
//! - `MongoDB` Atlas Search index configured for vector search
//!
//! # Example
//!
//! ```rust,no_run
//! use dashflow_mongodb::MongoDBVectorStore;
//! use dashflow::core::embeddings::Embeddings;
//! use dashflow::core::vector_stores::VectorStore;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create embeddings model (example using a mock)
//! # struct MockEmbeddings;
//! # #[async_trait::async_trait]
//! # impl Embeddings for MockEmbeddings {
//! #     async fn _embed_documents(&self, texts: &[String]) -> dashflow::core::Result<Vec<Vec<f32>>> {
//! #         Ok(vec![vec![0.0; 1536]; texts.len()])
//! #     }
//! #     async fn _embed_query(&self, text: &str) -> dashflow::core::Result<Vec<f32>> {
//! #         Ok(vec![0.0; 1536])
//! #     }
//! # }
//! let embeddings: Arc<dyn Embeddings> = Arc::new(MockEmbeddings);
//!
//! // Create vector store
//! let mut store = MongoDBVectorStore::new(
//!     "mongodb+srv://FAKE_USER:FAKE_PASS@cluster.example.net",
//!     "my_database",
//!     "my_collection",
//!     "vector_index", // Atlas Search index name
//!     embeddings,
//! ).await?;
//!
//! // Add documents
//! let ids = store.add_texts(
//!     &["document 1", "document 2"],
//!     None,
//!     None,
//! ).await?;
//!
//! // Search for similar documents
//! let results = store._similarity_search(
//!     "query text",
//!     5,
//!     None,
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Atlas Search Index Configuration
//!
//! Create an Atlas Search index with the following JSON definition:
//!
//! ```json
//! {
//!   "fields": [
//!     {
//!       "type": "vector",
//!       "path": "embedding",
//!       "numDimensions": 1536,
//!       "similarity": "cosine"
//!     },
//!     {
//!       "type": "filter",
//!       "path": "metadata"
//!     }
//!   ]
//! }
//! ```
//!
//! # See Also
//!
//! - [`VectorStore`](dashflow::core::vector_stores::VectorStore) - The trait this implements
//! - [`Embeddings`](dashflow::core::embeddings::Embeddings) - Required for generating vectors
//! - [`dashflow-supabase`](https://docs.rs/dashflow-supabase) - Alternative: Supabase/PostgreSQL with pgvector
//! - [`dashflow-pgvector`](https://docs.rs/dashflow-pgvector) - Alternative: PostgreSQL native vector search
//! - [MongoDB Atlas Vector Search](https://www.mongodb.com/docs/atlas/atlas-vector-search/vector-search-overview/) - Official docs

mod mongodb_store;

pub use mongodb_store::MongoDBVectorStore;
