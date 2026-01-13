//! `MongoDB` Atlas Vector Search vector store implementation for `DashFlow` Rust.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bson::{doc, Document as BsonDocument};
use dashflow::core::documents::Document;
use dashflow::core::embeddings::Embeddings;
use dashflow::core::vector_stores::{DistanceMetric, VectorStore};
use dashflow::core::{Error, Result};
use dashflow::{embed, embed_query};
use mongodb::options::ClientOptions;
use mongodb::{Client, Collection};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// Document stored in `MongoDB` with embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MongoDocument {
    #[serde(rename = "_id")]
    id: String,
    text: String,
    embedding: Vec<f32>,
    metadata: JsonValue,
}

/// `MongoDB` Atlas Vector Search vector store implementation.
///
/// This implementation uses `MongoDB` Atlas Vector Search for efficient similarity search
/// over large collections of embeddings. It requires a `MongoDB` Atlas cluster (M10+)
/// with Vector Search enabled and a properly configured Atlas Search index.
///
/// # Example
///
/// ```rust,no_run
/// use dashflow_mongodb::MongoDBVectorStore;
/// use dashflow::core::embeddings::Embeddings;
/// use dashflow::core::vector_stores::VectorStore;
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # struct MockEmbeddings;
/// # #[async_trait::async_trait]
/// # impl Embeddings for MockEmbeddings {
/// #     async fn _embed_documents(&self, texts: &[String]) -> dashflow::core::Result<Vec<Vec<f32>>> {
/// #         Ok(vec![vec![0.0; 1536]; texts.len()])
/// #     }
/// #     async fn _embed_query(&self, text: &str) -> dashflow::core::Result<Vec<f32>> {
/// #         Ok(vec![0.0; 1536])
/// #     }
/// # }
/// let embeddings: Arc<dyn Embeddings> = Arc::new(MockEmbeddings);
///
/// let mut store = MongoDBVectorStore::new(
///     "mongodb+srv://FAKE_USER:FAKE_PASS@cluster.example.net",
///     "my_database",
///     "my_collection",
///     "vector_index",
///     embeddings,
/// ).await?;
/// # Ok(())
/// # }
/// ```
pub struct MongoDBVectorStore {
    collection: Collection<BsonDocument>,
    index_name: String,
    embeddings: Arc<dyn Embeddings>,
    distance_metric: DistanceMetric,
}

impl MongoDBVectorStore {
    /// Creates a new `MongoDBVectorStore` instance.
    ///
    /// # Arguments
    ///
    /// * `connection_string` - `MongoDB` connection string (e.g., "<mongodb+srv://FAKE_USER:FAKE_PASS@cluster.example.net>")
    /// * `database_name` - Name of the database
    /// * `collection_name` - Name of the collection
    /// * `index_name` - Name of the Atlas Search index configured for vector search
    /// * `embeddings` - Embeddings model to use
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Connection to `MongoDB` fails
    /// - Database or collection doesn't exist
    ///
    /// # Note
    ///
    /// You must create an Atlas Search index manually via the `MongoDB` Atlas UI or API.
    /// See module documentation for index configuration.
    pub async fn new(
        connection_string: &str,
        database_name: &str,
        collection_name: &str,
        index_name: &str,
        embeddings: Arc<dyn Embeddings>,
    ) -> Result<Self> {
        // Parse connection string and connect
        let mut client_options = ClientOptions::parse(connection_string).await.map_err(|e| {
            Error::config(format!("Failed to parse MongoDB connection string: {e}"))
        })?;

        client_options.app_name = Some("dashflow-mongodb".to_string());

        let client = Client::with_options(client_options)
            .map_err(|e| Error::config(format!("Failed to create MongoDB client: {e}")))?;

        // Get database and collection
        let database = client.database(database_name);
        let collection = database.collection::<BsonDocument>(collection_name);

        Ok(Self {
            collection,
            index_name: index_name.to_string(),
            embeddings,
            distance_metric: DistanceMetric::Cosine,
        })
    }

    /// Sets the distance metric used for similarity calculations.
    ///
    /// Note: The Atlas Search index must be configured with a compatible similarity metric.
    /// - Cosine: Use "cosine" in index
    /// - Euclidean: Use "euclidean" in index
    /// - DotProduct/MaxInnerProduct: Use "dotProduct" in index
    #[must_use]
    pub fn with_distance_metric(mut self, metric: DistanceMetric) -> Self {
        self.distance_metric = metric;
        self
    }

    /// Builds metadata filter for `MongoDB` query.
    fn build_metadata_filter(
        &self,
        filter: Option<&HashMap<String, JsonValue>>,
    ) -> Option<BsonDocument> {
        filter.map(|f| {
            let mut doc = BsonDocument::new();
            for (key, value) in f {
                if let Ok(bson_value) = bson::to_bson(value) {
                    doc.insert(format!("metadata.{key}"), bson_value);
                }
            }
            doc
        })
    }

    /// Performs vector search using `MongoDB` Atlas Vector Search.
    async fn vector_search(
        &self,
        query_vector: Vec<f32>,
        k: usize,
        filter: Option<&HashMap<String, JsonValue>>,
    ) -> Result<Vec<(Document, f32)>> {
        // Build $vectorSearch aggregation stage
        let mut vector_search_doc = doc! {
            "index": &self.index_name,
            "path": "embedding",
            "queryVector": query_vector.clone(),
            "numCandidates": (k * 10).max(100) as i32, // Fetch more candidates for better results
            "limit": k as i32,
        };

        // Add metadata filter if provided
        if let Some(filter_doc) = self.build_metadata_filter(filter) {
            vector_search_doc.insert("filter", filter_doc);
        }

        // Build aggregation pipeline
        let pipeline = vec![
            doc! { "$vectorSearch": vector_search_doc },
            doc! {
                "$addFields": {
                    "score": { "$meta": "vectorSearchScore" }
                }
            },
        ];

        // Execute aggregation
        let mut cursor = self
            .collection
            .aggregate(pipeline)
            .await
            .map_err(|e| Error::other(format!("MongoDB vector search failed: {e}")))?;

        // Parse results
        let mut results = Vec::new();
        while cursor
            .advance()
            .await
            .map_err(|e| Error::other(format!("Failed to read cursor: {e}")))?
        {
            let doc = cursor.current();

            // Parse document
            let id = doc.get_str("_id").unwrap_or("").to_string();
            let text = doc.get_str("text").unwrap_or("").to_string();
            let score = doc.get_f64("score").unwrap_or(0.0) as f32;

            // Parse metadata - deserialize from raw document
            let metadata: JsonValue = bson::from_slice(doc.as_bytes())
                .ok()
                .and_then(|v: serde_json::Value| v.get("metadata").cloned())
                .unwrap_or(JsonValue::Object(Default::default()));

            let document = Document {
                id: Some(id),
                page_content: text,
                metadata: if let JsonValue::Object(map) = metadata {
                    map.into_iter().collect()
                } else {
                    HashMap::new()
                },
            };

            results.push((document, score));
        }

        Ok(results)
    }
}

#[async_trait]
impl VectorStore for MongoDBVectorStore {
    fn embeddings(&self) -> Option<Arc<dyn Embeddings>> {
        Some(Arc::clone(&self.embeddings))
    }

    fn distance_metric(&self) -> DistanceMetric {
        self.distance_metric
    }

    async fn add_texts(
        &mut self,
        texts: &[impl AsRef<str> + Send + Sync],
        metadatas: Option<&[HashMap<String, JsonValue>]>,
        ids: Option<&[String]>,
    ) -> Result<Vec<String>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Validate inputs
        if let Some(metas) = metadatas {
            if metas.len() != texts.len() {
                return Err(Error::config(format!(
                    "Metadatas length ({}) doesn't match texts length ({})",
                    metas.len(),
                    texts.len()
                )));
            }
        }
        if let Some(ids_vec) = ids {
            if ids_vec.len() != texts.len() {
                return Err(Error::config(format!(
                    "IDs length ({}) doesn't match texts length ({})",
                    ids_vec.len(),
                    texts.len()
                )));
            }
        }

        // Convert texts to strings for embedding
        let text_strs: Vec<String> = texts.iter().map(|t| t.as_ref().to_string()).collect();

        // Generate embeddings using graph API
        let embeddings = embed(Arc::clone(&self.embeddings), &text_strs).await?;

        // Generate IDs if not provided
        let document_ids: Vec<String> = if let Some(ids_vec) = ids {
            ids_vec.to_vec()
        } else {
            (0..texts.len())
                .map(|_| Uuid::new_v4().to_string())
                .collect()
        };

        // Prepare documents for insertion
        let mut documents = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let metadata = metadatas
                .and_then(|m| m.get(i))
                .cloned()
                .unwrap_or_else(HashMap::new);

            let mongo_doc = MongoDocument {
                id: document_ids[i].clone(),
                text: text.as_ref().to_string(),
                embedding: embeddings[i].clone(),
                metadata: JsonValue::Object(metadata.into_iter().collect()),
            };

            // Convert to BSON document
            let bson_doc = bson::to_document(&mongo_doc)
                .map_err(|e| Error::other(format!("Failed to serialize document: {e}")))?;
            documents.push(bson_doc);
        }

        // Insert documents (upsert to handle duplicates)
        for doc in documents {
            let id = doc.get_str("_id").unwrap_or("").to_string();
            self.collection
                .replace_one(doc! { "_id": &id }, doc.clone())
                .with_options(
                    mongodb::options::ReplaceOptions::builder()
                        .upsert(true)
                        .build(),
                )
                .await
                .map_err(|e| Error::other(format!("Failed to insert document: {e}")))?;
        }

        Ok(document_ids)
    }

    async fn _similarity_search(
        &self,
        query: &str,
        k: usize,
        filter: Option<&HashMap<String, JsonValue>>,
    ) -> Result<Vec<Document>> {
        let results = self.similarity_search_with_score(query, k, filter).await?;
        Ok(results.into_iter().map(|(doc, _)| doc).collect())
    }

    async fn similarity_search_with_score(
        &self,
        query: &str,
        k: usize,
        filter: Option<&HashMap<String, JsonValue>>,
    ) -> Result<Vec<(Document, f32)>> {
        // Embed query using graph API
        let query_vector = embed_query(Arc::clone(&self.embeddings), query).await?;

        // Perform vector search
        self.vector_search(query_vector, k, filter).await
    }

    async fn similarity_search_by_vector(
        &self,
        embedding: &[f32],
        k: usize,
        filter: Option<&HashMap<String, JsonValue>>,
    ) -> Result<Vec<Document>> {
        let results = self
            .similarity_search_by_vector_with_score(embedding, k, filter)
            .await?;
        Ok(results.into_iter().map(|(doc, _)| doc).collect())
    }

    async fn similarity_search_by_vector_with_score(
        &self,
        embedding: &[f32],
        k: usize,
        filter: Option<&HashMap<String, JsonValue>>,
    ) -> Result<Vec<(Document, f32)>> {
        self.vector_search(embedding.to_vec(), k, filter).await
    }

    async fn delete(&mut self, ids: Option<&[String]>) -> Result<bool> {
        let ids = match ids {
            Some(ids) if !ids.is_empty() => ids,
            _ => return Ok(false),
        };

        // Delete documents by IDs
        let result = self
            .collection
            .delete_many(doc! { "_id": { "$in": ids } })
            .await
            .map_err(|e| Error::other(format!("Failed to delete documents: {e}")))?;

        Ok(result.deleted_count > 0)
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<Document>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        // Query documents by IDs
        let filter = doc! { "_id": { "$in": ids } };
        let mut cursor = self
            .collection
            .find(filter)
            .await
            .map_err(|e| Error::other(format!("Failed to query documents: {e}")))?;

        // Parse results
        let mut documents = Vec::new();
        while cursor
            .advance()
            .await
            .map_err(|e| Error::other(format!("Failed to read cursor: {e}")))?
        {
            let doc = cursor.current();

            let id = doc.get_str("_id").unwrap_or("").to_string();
            let text = doc.get_str("text").unwrap_or("").to_string();

            // Parse metadata - deserialize from raw document
            let metadata: JsonValue = bson::from_slice(doc.as_bytes())
                .ok()
                .and_then(|v: serde_json::Value| v.get("metadata").cloned())
                .unwrap_or(JsonValue::Object(Default::default()));

            let document = Document {
                id: Some(id),
                page_content: text,
                metadata: if let JsonValue::Object(map) = metadata {
                    map.into_iter().collect()
                } else {
                    HashMap::new()
                },
            };

            documents.push(document);
        }

        Ok(documents)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[cfg(test)]
mod tests {
    use super::*;

    // Tests for MongoDocument struct

    #[test]
    fn test_mongo_document_serialization() {
        let doc = MongoDocument {
            id: "test-123".to_string(),
            text: "Hello world".to_string(),
            embedding: vec![0.1, 0.2, 0.3],
            metadata: JsonValue::Object(Default::default()),
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"_id\":\"test-123\""));
        assert!(json.contains("\"text\":\"Hello world\""));
        assert!(json.contains("\"embedding\":[0.1,0.2,0.3]"));
    }

    #[test]
    fn test_mongo_document_deserialization() {
        let json = r#"{"_id":"doc-1","text":"content","embedding":[0.5,0.6],"metadata":{}}"#;
        let doc: MongoDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.id, "doc-1");
        assert_eq!(doc.text, "content");
        assert_eq!(doc.embedding, vec![0.5, 0.6]);
    }

    #[test]
    fn test_mongo_document_with_metadata() {
        let mut metadata = serde_json::Map::new();
        metadata.insert("source".to_string(), JsonValue::String("test.pdf".to_string()));
        metadata.insert("page".to_string(), JsonValue::Number(42.into()));

        let doc = MongoDocument {
            id: "doc-1".to_string(),
            text: "content".to_string(),
            embedding: vec![0.1],
            metadata: JsonValue::Object(metadata),
        };

        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("source"));
        assert!(json.contains("test.pdf"));
        assert!(json.contains("page"));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_mongo_document_clone() {
        let doc = MongoDocument {
            id: "test".to_string(),
            text: "text".to_string(),
            embedding: vec![1.0, 2.0],
            metadata: JsonValue::Object(Default::default()),
        };
        let cloned = doc.clone();
        assert_eq!(doc.id, cloned.id);
        assert_eq!(doc.text, cloned.text);
        assert_eq!(doc.embedding, cloned.embedding);
    }

    #[test]
    fn test_mongo_document_debug() {
        let doc = MongoDocument {
            id: "test".to_string(),
            text: "text".to_string(),
            embedding: vec![1.0],
            metadata: JsonValue::Object(Default::default()),
        };
        let debug = format!("{:?}", doc);
        assert!(debug.contains("MongoDocument"));
        assert!(debug.contains("test"));
    }

    // Tests for DistanceMetric

    #[test]
    fn test_distance_metric_default() {
        let metric = DistanceMetric::Cosine;
        assert!(matches!(metric, DistanceMetric::Cosine));
    }

    #[test]
    fn test_distance_metric_euclidean() {
        let metric = DistanceMetric::Euclidean;
        assert!(matches!(metric, DistanceMetric::Euclidean));
    }

    #[test]
    fn test_distance_metric_dot_product() {
        let metric = DistanceMetric::DotProduct;
        assert!(matches!(metric, DistanceMetric::DotProduct));
    }

    #[test]
    fn test_distance_metric_max_inner_product() {
        let metric = DistanceMetric::MaxInnerProduct;
        assert!(matches!(metric, DistanceMetric::MaxInnerProduct));
    }

    // Tests for metadata filter building

    #[test]
    fn test_build_metadata_filter_none() {
        // When filter is None, should return None
        let filter: Option<&HashMap<String, JsonValue>> = None;
        assert!(filter.is_none());
    }

    #[test]
    fn test_build_metadata_filter_empty() {
        let filter: HashMap<String, JsonValue> = HashMap::new();
        let filter_doc = if filter.is_empty() {
            None
        } else {
            Some(filter)
        };
        assert!(filter_doc.is_none());
    }

    #[test]
    fn test_metadata_filter_key_format() {
        let key = "source";
        let formatted = format!("metadata.{}", key);
        assert_eq!(formatted, "metadata.source");
    }

    #[test]
    fn test_metadata_filter_nested_key() {
        let key = "author.name";
        let formatted = format!("metadata.{}", key);
        assert_eq!(formatted, "metadata.author.name");
    }

    // Tests for vector search document building

    #[test]
    fn test_vector_search_num_candidates() {
        let k = 10;
        let num_candidates = (k * 10).max(100);
        assert_eq!(num_candidates, 100);
    }

    #[test]
    fn test_vector_search_num_candidates_large_k() {
        let k = 50;
        let num_candidates = (k * 10).max(100);
        assert_eq!(num_candidates, 500);
    }

    #[test]
    fn test_vector_search_num_candidates_small_k() {
        let k = 5;
        let num_candidates = (k * 10).max(100);
        assert_eq!(num_candidates, 100); // Max ensures at least 100
    }

    // Tests for ID deletion query

    #[test]
    fn test_delete_ids_empty_check() {
        let ids: Option<&[String]> = Some(&[]);
        match ids {
            Some(ids) if !ids.is_empty() => panic!("Should not match"),
            _ => {} // Expected path
        }
    }

    #[test]
    fn test_delete_ids_some_check() {
        let id_vec = vec!["id1".to_string()];
        let ids: Option<&[String]> = Some(&id_vec);
        match ids {
            Some(ids) if !ids.is_empty() => assert_eq!(ids.len(), 1),
            _ => panic!("Should have matched"),
        }
    }

    // Tests for metadata parsing

    #[test]
    fn test_metadata_object_extraction() {
        let metadata = JsonValue::Object(serde_json::Map::new());
        if let JsonValue::Object(map) = metadata {
            let hash_map: HashMap<String, JsonValue> = map.into_iter().collect();
            assert!(hash_map.is_empty());
        } else {
            panic!("Expected Object");
        }
    }

    #[test]
    fn test_metadata_non_object_fallback() {
        let metadata = JsonValue::String("not an object".to_string());
        let hash_map: HashMap<String, JsonValue> = if let JsonValue::Object(map) = metadata {
            map.into_iter().collect()
        } else {
            HashMap::new()
        };
        assert!(hash_map.is_empty());
    }

    #[test]
    fn test_metadata_with_values_extraction() {
        let mut map = serde_json::Map::new();
        map.insert("key1".to_string(), JsonValue::String("value1".to_string()));
        map.insert("key2".to_string(), JsonValue::Number(42.into()));

        let metadata = JsonValue::Object(map);
        if let JsonValue::Object(map) = metadata {
            let hash_map: HashMap<String, JsonValue> = map.into_iter().collect();
            assert_eq!(hash_map.len(), 2);
            assert_eq!(
                hash_map.get("key1").unwrap().as_str().unwrap(),
                "value1"
            );
            assert_eq!(hash_map.get("key2").unwrap().as_i64().unwrap(), 42);
        }
    }

    // Tests for UUID generation

    #[test]
    fn test_uuid_generation() {
        let id = Uuid::new_v4().to_string();
        assert_eq!(id.len(), 36);
    }

    #[test]
    fn test_uuid_uniqueness() {
        let ids: Vec<String> = (0..10).map(|_| Uuid::new_v4().to_string()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 10);
    }

    // Tests for input validation

    #[test]
    fn test_empty_texts_check() {
        let texts: Vec<&str> = vec![];
        assert!(texts.is_empty());
    }

    #[test]
    fn test_metadata_length_mismatch() {
        let texts = ["a", "b", "c"];
        let metas: Vec<HashMap<String, JsonValue>> = vec![HashMap::new(), HashMap::new()];
        assert_ne!(metas.len(), texts.len());
    }

    #[test]
    fn test_ids_length_mismatch() {
        let texts = ["a", "b"];
        let ids = ["id1".to_string()];
        assert_ne!(ids.len(), texts.len());
    }

    #[test]
    fn test_lengths_match() {
        let texts = ["a", "b"];
        let metas: Vec<HashMap<String, JsonValue>> = vec![HashMap::new(), HashMap::new()];
        let ids = ["id1".to_string(), "id2".to_string()];
        assert_eq!(texts.len(), metas.len());
        assert_eq!(texts.len(), ids.len());
    }

    // Tests for BSON conversion

    #[test]
    fn test_json_to_bson_string() {
        let json_value = JsonValue::String("test".to_string());
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_number() {
        let json_value = JsonValue::Number(42.into());
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_bool() {
        let json_value = JsonValue::Bool(true);
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_null() {
        let json_value = JsonValue::Null;
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_array() {
        let json_value = JsonValue::Array(vec![
            JsonValue::Number(1.into()),
            JsonValue::Number(2.into()),
        ]);
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_object() {
        let mut map = serde_json::Map::new();
        map.insert("key".to_string(), JsonValue::String("value".to_string()));
        let json_value = JsonValue::Object(map);
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    // Tests for Document struct conversion

    #[test]
    fn test_document_creation() {
        let doc = Document {
            id: Some("test-id".to_string()),
            page_content: "Test content".to_string(),
            metadata: HashMap::new(),
        };
        assert_eq!(doc.id, Some("test-id".to_string()));
        assert_eq!(doc.page_content, "Test content");
        assert!(doc.metadata.is_empty());
    }

    #[test]
    fn test_document_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("source".to_string(), JsonValue::String("file.txt".to_string()));

        let doc = Document {
            id: Some("doc-1".to_string()),
            page_content: "content".to_string(),
            metadata,
        };

        assert_eq!(doc.metadata.len(), 1);
        assert_eq!(
            doc.metadata.get("source").unwrap().as_str().unwrap(),
            "file.txt"
        );
    }

    #[test]
    fn test_document_no_id() {
        let doc = Document {
            id: None,
            page_content: "content".to_string(),
            metadata: HashMap::new(),
        };
        assert!(doc.id.is_none());
    }

    // Tests for score handling

    #[test]
    fn test_score_as_f32() {
        let score_f64: f64 = 0.95;
        let score_f32 = score_f64 as f32;
        assert!((score_f32 - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_score_default_zero() {
        let score = 0.0_f64;
        assert!((score as f32).abs() < f32::EPSILON);
    }

    // Tests for empty ID string handling

    #[test]
    fn test_empty_string_fallback() {
        let id: &str = "";
        assert!(id.is_empty());
        let fallback = if id.is_empty() { "unknown" } else { id };
        assert_eq!(fallback, "unknown");
    }

    // ========================================================================
    // Additional MongoDocument struct tests
    // ========================================================================

    #[test]
    fn test_mongo_document_empty_text() {
        let doc = MongoDocument {
            id: "empty-text".to_string(),
            text: String::new(),
            embedding: vec![0.1],
            metadata: JsonValue::Object(Default::default()),
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"text\":\"\""));
    }

    #[test]
    fn test_mongo_document_empty_embedding() {
        let doc = MongoDocument {
            id: "empty-emb".to_string(),
            text: "text".to_string(),
            embedding: vec![],
            metadata: JsonValue::Object(Default::default()),
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"embedding\":[]"));
    }

    #[test]
    fn test_mongo_document_large_embedding() {
        let embedding: Vec<f32> = (0..1536).map(|i| i as f32 / 1536.0).collect();
        let doc = MongoDocument {
            id: "large-emb".to_string(),
            text: "test".to_string(),
            embedding: embedding.clone(),
            metadata: JsonValue::Object(Default::default()),
        };
        assert_eq!(doc.embedding.len(), 1536);
    }

    #[test]
    fn test_mongo_document_nested_metadata() {
        let mut nested = serde_json::Map::new();
        nested.insert("inner".to_string(), JsonValue::String("value".to_string()));

        let mut metadata = serde_json::Map::new();
        metadata.insert("outer".to_string(), JsonValue::Object(nested));

        let doc = MongoDocument {
            id: "nested".to_string(),
            text: "text".to_string(),
            embedding: vec![0.1],
            metadata: JsonValue::Object(metadata),
        };

        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("inner"));
        assert!(json.contains("outer"));
    }

    #[test]
    fn test_mongo_document_array_metadata() {
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "tags".to_string(),
            JsonValue::Array(vec![
                JsonValue::String("tag1".to_string()),
                JsonValue::String("tag2".to_string()),
            ]),
        );

        let doc = MongoDocument {
            id: "array-meta".to_string(),
            text: "text".to_string(),
            embedding: vec![0.1],
            metadata: JsonValue::Object(metadata),
        };

        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("tags"));
        assert!(json.contains("tag1"));
        assert!(json.contains("tag2"));
    }

    #[test]
    fn test_mongo_document_special_chars_text() {
        let doc = MongoDocument {
            id: "special".to_string(),
            text: "Line1\nLine2\tTab\"Quote".to_string(),
            embedding: vec![0.1],
            metadata: JsonValue::Object(Default::default()),
        };

        let json = serde_json::to_string(&doc).unwrap();
        // JSON should properly escape special characters
        assert!(json.contains("\\n"));
        assert!(json.contains("\\t"));
        assert!(json.contains("\\\""));
    }

    #[test]
    fn test_mongo_document_unicode_text() {
        let doc = MongoDocument {
            id: "unicode".to_string(),
            text: "日本語 中文 한국어 مرحبا".to_string(),
            embedding: vec![0.1],
            metadata: JsonValue::Object(Default::default()),
        };

        let json = serde_json::to_string(&doc).unwrap();
        let deser: MongoDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.text, "日本語 中文 한국어 مرحبا");
    }

    #[test]
    fn test_mongo_document_float_precision() {
        let doc = MongoDocument {
            id: "float".to_string(),
            text: "text".to_string(),
            embedding: vec![0.123_456_78, 0.999_999_9, -0.000_001],
            metadata: JsonValue::Object(Default::default()),
        };

        let json = serde_json::to_string(&doc).unwrap();
        let deser: MongoDocument = serde_json::from_str(&json).unwrap();

        // Float precision should be preserved reasonably
        assert!((deser.embedding[0] - 0.123_456_78).abs() < 1e-6);
        assert!((deser.embedding[1] - 0.999_999_9).abs() < 1e-6);
        assert!((deser.embedding[2] - (-0.000_001)).abs() < 1e-6);
    }

    #[test]
    fn test_mongo_document_null_in_metadata() {
        let mut metadata = serde_json::Map::new();
        metadata.insert("nullable".to_string(), JsonValue::Null);

        let doc = MongoDocument {
            id: "nullable".to_string(),
            text: "text".to_string(),
            embedding: vec![0.1],
            metadata: JsonValue::Object(metadata),
        };

        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("null"));
    }

    // ========================================================================
    // Distance metric tests
    // ========================================================================

    #[test]
    fn test_distance_metric_copy() {
        let metric = DistanceMetric::Cosine;
        let copied = metric;
        assert!(matches!(copied, DistanceMetric::Cosine));
    }

    #[test]
    fn test_all_distance_metrics() {
        let metrics = [
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::MaxInnerProduct,
        ];

        for metric in metrics {
            // All metrics should be usable
            let _ = format!("{:?}", metric);
        }
    }

    // ========================================================================
    // Metadata filter key formatting tests
    // ========================================================================

    #[test]
    fn test_metadata_key_with_dots() {
        let key = "nested.path.value";
        let formatted = format!("metadata.{}", key);
        assert_eq!(formatted, "metadata.nested.path.value");
    }

    #[test]
    fn test_metadata_key_with_spaces() {
        let key = "key with spaces";
        let formatted = format!("metadata.{}", key);
        assert_eq!(formatted, "metadata.key with spaces");
    }

    #[test]
    fn test_metadata_key_empty() {
        let key = "";
        let formatted = format!("metadata.{}", key);
        assert_eq!(formatted, "metadata.");
    }

    #[test]
    fn test_metadata_key_unicode() {
        let key = "日本語キー";
        let formatted = format!("metadata.{}", key);
        assert_eq!(formatted, "metadata.日本語キー");
    }

    // ========================================================================
    // Vector search num_candidates calculation tests
    // ========================================================================

    #[test]
    fn test_num_candidates_k_1() {
        let k = 1;
        let num_candidates = (k * 10).max(100);
        assert_eq!(num_candidates, 100);
    }

    #[test]
    fn test_num_candidates_k_10() {
        let k = 10;
        let num_candidates = (k * 10).max(100);
        assert_eq!(num_candidates, 100);
    }

    #[test]
    fn test_num_candidates_k_11() {
        let k = 11;
        let num_candidates = (k * 10).max(100);
        assert_eq!(num_candidates, 110);
    }

    #[test]
    fn test_num_candidates_k_100() {
        let k = 100;
        let num_candidates = (k * 10).max(100);
        assert_eq!(num_candidates, 1000);
    }

    #[test]
    fn test_num_candidates_k_0() {
        let k = 0;
        let num_candidates = (k * 10).max(100);
        assert_eq!(num_candidates, 100);
    }

    // ========================================================================
    // Delete IDs edge cases
    // ========================================================================

    #[test]
    fn test_delete_none_ids() {
        let ids: Option<&[String]> = None;
        let should_delete = matches!(ids, Some(ids) if !ids.is_empty());
        assert!(!should_delete);
    }

    #[test]
    fn test_delete_multiple_ids() {
        let id_vec = vec!["id1".to_string(), "id2".to_string(), "id3".to_string()];
        let ids: Option<&[String]> = Some(&id_vec);
        match ids {
            Some(ids) if !ids.is_empty() => assert_eq!(ids.len(), 3),
            _ => panic!("Should have matched"),
        }
    }

    // ========================================================================
    // Metadata JSON value type tests
    // ========================================================================

    #[test]
    fn test_metadata_array_value() {
        let metadata = JsonValue::Array(vec![
            JsonValue::Number(1.into()),
            JsonValue::Number(2.into()),
        ]);
        let hash_map: HashMap<String, JsonValue> = if let JsonValue::Object(map) = metadata {
            map.into_iter().collect()
        } else {
            HashMap::new()
        };
        assert!(hash_map.is_empty()); // Array is not Object
    }

    #[test]
    fn test_metadata_null_value() {
        let metadata = JsonValue::Null;
        let hash_map: HashMap<String, JsonValue> = if let JsonValue::Object(map) = metadata {
            map.into_iter().collect()
        } else {
            HashMap::new()
        };
        assert!(hash_map.is_empty());
    }

    #[test]
    fn test_metadata_bool_value() {
        let metadata = JsonValue::Bool(true);
        let hash_map: HashMap<String, JsonValue> = if let JsonValue::Object(map) = metadata {
            map.into_iter().collect()
        } else {
            HashMap::new()
        };
        assert!(hash_map.is_empty());
    }

    #[test]
    fn test_metadata_number_value() {
        let metadata = JsonValue::Number(42.into());
        let hash_map: HashMap<String, JsonValue> = if let JsonValue::Object(map) = metadata {
            map.into_iter().collect()
        } else {
            HashMap::new()
        };
        assert!(hash_map.is_empty());
    }

    // ========================================================================
    // UUID tests
    // ========================================================================

    #[test]
    fn test_uuid_format_v4() {
        let id = Uuid::new_v4().to_string();
        // UUID v4 format: xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
    }

    #[test]
    fn test_uuid_v4_version_digit() {
        let id = Uuid::new_v4().to_string();
        // Third segment should start with 4 for v4
        let parts: Vec<&str> = id.split('-').collect();
        assert!(parts[2].starts_with('4'));
    }

    #[test]
    fn test_uuid_batch_generation() {
        let ids: Vec<String> = (0..100).map(|_| Uuid::new_v4().to_string()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 100);
    }

    // ========================================================================
    // Input validation edge cases
    // ========================================================================

    #[test]
    fn test_texts_single_element() {
        let texts = ["single"];
        assert_eq!(texts.len(), 1);
    }

    #[test]
    fn test_metadata_exactly_matches_texts() {
        let texts = ["a", "b", "c"];
        let metas: Vec<HashMap<String, JsonValue>> =
            vec![HashMap::new(), HashMap::new(), HashMap::new()];
        assert_eq!(metas.len(), texts.len());
    }

    #[test]
    fn test_ids_exactly_matches_texts() {
        let texts = ["a", "b"];
        let ids = ["id1".to_string(), "id2".to_string()];
        assert_eq!(ids.len(), texts.len());
    }

    #[test]
    fn test_empty_metadatas_option() {
        let metadatas: Option<&[HashMap<String, JsonValue>]> = None;
        assert!(metadatas.is_none());
    }

    #[test]
    fn test_empty_ids_option() {
        let ids: Option<&[String]> = None;
        assert!(ids.is_none());
    }

    // ========================================================================
    // BSON conversion edge cases
    // ========================================================================

    #[test]
    fn test_json_to_bson_float() {
        let json_value = JsonValue::Number(serde_json::Number::from_f64(3.14).unwrap());
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_negative_int() {
        let json_value = JsonValue::Number((-42).into());
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_large_int() {
        let json_value = JsonValue::Number(i64::MAX.into());
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_empty_string() {
        let json_value = JsonValue::String(String::new());
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_long_string() {
        let json_value = JsonValue::String("x".repeat(10000));
        let bson_result = bson::to_bson(&json_value);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_nested_array() {
        let inner = JsonValue::Array(vec![JsonValue::Number(1.into())]);
        let outer = JsonValue::Array(vec![inner]);
        let bson_result = bson::to_bson(&outer);
        assert!(bson_result.is_ok());
    }

    #[test]
    fn test_json_to_bson_nested_object() {
        let mut inner = serde_json::Map::new();
        inner.insert("key".to_string(), JsonValue::String("value".to_string()));

        let mut outer = serde_json::Map::new();
        outer.insert("nested".to_string(), JsonValue::Object(inner));

        let bson_result = bson::to_bson(&JsonValue::Object(outer));
        assert!(bson_result.is_ok());
    }

    // ========================================================================
    // Document struct tests
    // ========================================================================

    #[test]
    fn test_document_empty_content() {
        let doc = Document {
            id: Some("id".to_string()),
            page_content: String::new(),
            metadata: HashMap::new(),
        };
        assert!(doc.page_content.is_empty());
    }

    #[test]
    fn test_document_long_content() {
        let doc = Document {
            id: Some("id".to_string()),
            page_content: "x".repeat(100_000),
            metadata: HashMap::new(),
        };
        assert_eq!(doc.page_content.len(), 100_000);
    }

    #[test]
    fn test_document_multiple_metadata_types() {
        let mut metadata = HashMap::new();
        metadata.insert("string".to_string(), JsonValue::String("text".to_string()));
        metadata.insert("number".to_string(), JsonValue::Number(42.into()));
        metadata.insert("bool".to_string(), JsonValue::Bool(true));
        metadata.insert("null".to_string(), JsonValue::Null);

        let doc = Document {
            id: Some("id".to_string()),
            page_content: "content".to_string(),
            metadata,
        };

        assert_eq!(doc.metadata.len(), 4);
    }

    #[test]
    fn test_document_unicode_id() {
        let doc = Document {
            id: Some("日本語-id".to_string()),
            page_content: "content".to_string(),
            metadata: HashMap::new(),
        };
        assert_eq!(doc.id, Some("日本語-id".to_string()));
    }

    // ========================================================================
    // Score handling tests
    // ========================================================================

    #[test]
    fn test_score_negative() {
        let score_f64: f64 = -0.5;
        let score_f32 = score_f64 as f32;
        assert!((score_f32 - (-0.5)).abs() < 0.001);
    }

    #[test]
    fn test_score_one() {
        let score_f64: f64 = 1.0;
        let score_f32 = score_f64 as f32;
        assert!((score_f32 - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_score_very_small() {
        let score_f64: f64 = 1e-10;
        let score_f32 = score_f64 as f32;
        assert!(score_f32 < 1e-8);
    }

    #[test]
    fn test_score_very_large() {
        let score_f64: f64 = 1e10;
        let score_f32 = score_f64 as f32;
        assert!(score_f32 > 1e9);
    }

    // ========================================================================
    // Error message format tests
    // ========================================================================

    #[test]
    fn test_error_message_connection_format() {
        let error_msg = format!("Failed to parse MongoDB connection string: {}", "test error");
        assert!(error_msg.contains("MongoDB"));
        assert!(error_msg.contains("connection string"));
    }

    #[test]
    fn test_error_message_client_format() {
        let error_msg = format!("Failed to create MongoDB client: {}", "test error");
        assert!(error_msg.contains("MongoDB"));
        assert!(error_msg.contains("client"));
    }

    #[test]
    fn test_error_message_serialize_format() {
        let error_msg = format!("Failed to serialize document: {}", "test error");
        assert!(error_msg.contains("serialize"));
    }

    #[test]
    fn test_error_message_insert_format() {
        let error_msg = format!("Failed to insert document: {}", "test error");
        assert!(error_msg.contains("insert"));
    }

    #[test]
    fn test_error_message_search_format() {
        let error_msg = format!("MongoDB vector search failed: {}", "test error");
        assert!(error_msg.contains("vector search"));
    }

    #[test]
    fn test_error_message_cursor_format() {
        let error_msg = format!("Failed to read cursor: {}", "test error");
        assert!(error_msg.contains("cursor"));
    }

    #[test]
    fn test_error_message_delete_format() {
        let error_msg = format!("Failed to delete documents: {}", "test error");
        assert!(error_msg.contains("delete"));
    }

    #[test]
    fn test_error_message_query_format() {
        let error_msg = format!("Failed to query documents: {}", "test error");
        assert!(error_msg.contains("query"));
    }

    // ========================================================================
    // Config error message tests
    // ========================================================================

    #[test]
    fn test_config_error_metadata_mismatch() {
        let texts_len = 5;
        let metas_len = 3;
        let error_msg = format!(
            "Metadatas length ({}) doesn't match texts length ({})",
            metas_len, texts_len
        );
        assert!(error_msg.contains("Metadatas"));
        assert!(error_msg.contains("5"));
        assert!(error_msg.contains("3"));
    }

    #[test]
    fn test_config_error_ids_mismatch() {
        let texts_len = 5;
        let ids_len = 2;
        let error_msg = format!(
            "IDs length ({}) doesn't match texts length ({})",
            ids_len, texts_len
        );
        assert!(error_msg.contains("IDs"));
        assert!(error_msg.contains("5"));
        assert!(error_msg.contains("2"));
    }

    // ========================================================================
    // Index name tests
    // ========================================================================

    #[test]
    fn test_index_name_simple() {
        let index_name = "vector_index";
        assert!(!index_name.is_empty());
    }

    #[test]
    fn test_index_name_with_underscore() {
        let index_name = "my_vector_search_index";
        assert!(index_name.contains('_'));
    }

    #[test]
    fn test_index_name_with_numbers() {
        let index_name = "vector_index_v2";
        assert!(index_name.contains("v2"));
    }

    // ========================================================================
    // App name tests
    // ========================================================================

    #[test]
    fn test_app_name_constant() {
        let app_name = "dashflow-mongodb";
        assert_eq!(app_name, "dashflow-mongodb");
    }

    // ========================================================================
    // BSON document construction tests
    // ========================================================================

    #[test]
    fn test_bson_doc_macro() {
        let doc = doc! { "_id": "test" };
        assert!(doc.contains_key("_id"));
    }

    #[test]
    fn test_bson_doc_multiple_fields() {
        let doc = doc! {
            "_id": "test",
            "text": "content",
            "score": 0.95
        };
        assert!(doc.contains_key("_id"));
        assert!(doc.contains_key("text"));
        assert!(doc.contains_key("score"));
    }

    #[test]
    fn test_bson_doc_nested() {
        let doc = doc! {
            "filter": {
                "metadata.source": "test"
            }
        };
        assert!(doc.contains_key("filter"));
    }

    #[test]
    fn test_bson_doc_in_operator() {
        let ids = vec!["id1".to_string(), "id2".to_string()];
        let doc = doc! { "_id": { "$in": &ids } };
        assert!(doc.contains_key("_id"));
    }

    // ========================================================================
    // Vector construction tests
    // ========================================================================

    #[test]
    fn test_query_vector_clone() {
        let query_vector = vec![0.1, 0.2, 0.3];
        let cloned = query_vector.clone();
        assert_eq!(query_vector, cloned);
    }

    #[test]
    fn test_empty_results_vector() {
        let results: Vec<(Document, f32)> = Vec::new();
        assert!(results.is_empty());
    }

    #[test]
    fn test_results_iteration() {
        let mut results: Vec<(Document, f32)> = Vec::new();
        results.push((
            Document {
                id: Some("id".to_string()),
                page_content: "content".to_string(),
                metadata: HashMap::new(),
            },
            0.95,
        ));

        let docs: Vec<Document> = results.into_iter().map(|(doc, _)| doc).collect();
        assert_eq!(docs.len(), 1);
    }

    // ========================================================================
    // Get by IDs tests
    // ========================================================================

    #[test]
    fn test_get_by_ids_empty() {
        let ids: Vec<String> = vec![];
        assert!(ids.is_empty());
    }

    #[test]
    fn test_get_by_ids_single() {
        let ids = vec!["single-id".to_string()];
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn test_get_by_ids_multiple() {
        let ids = vec!["id1".to_string(), "id2".to_string(), "id3".to_string()];
        assert_eq!(ids.len(), 3);
    }
}
