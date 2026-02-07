use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRecord {
  pub project_id: String,
  pub file_id: String,
  pub file_path: String,
  pub file_hash: String,
  pub chunk_id: String,
  pub chunk_index: i32,
  pub text: String,
  pub embedding: Vec<f32>,
  pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
  pub project_id: String,
  pub file_id: String,
  pub file_path: String,
  pub file_hash: String,
  pub mtime: Option<i64>,
  pub size: Option<i64>,
  pub is_deleted: Option<bool>,
  pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkHit {
  pub project_id: String,
  pub file_id: String,
  pub file_path: String,
  pub chunk_id: String,
  pub chunk_index: i32,
  pub text: String,
  pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedFile {
  pub path: String,
  pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexReport {
  pub project_id: String,
  pub root_dir: Option<String>,
  pub indexed_files: usize,
  pub updated_files: usize,
  pub deleted_files: usize,
  pub skipped_files: Vec<SkippedFile>,
  pub chunks_added: usize,
  pub chunks_deleted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexAddRequest {
  pub project_id: String,
  pub file_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSyncRequest {
  pub project_id: String,
  pub root_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRemoveRequest {
  pub project_id: String,
  pub file_paths: Option<Vec<String>>,
  pub file_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagSearchRequest {
  pub query: String,
  pub project_ids: Vec<String>,
  pub top_k: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagSearchResponse {
  pub hits: Vec<ChunkHit>,
}
