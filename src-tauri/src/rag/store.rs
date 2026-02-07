use crate::rag::types::{ChunkHit, ChunkRecord, FileRecord};
use std::collections::HashMap;

pub trait RagStore: Send + Sync {
  fn add_chunks(&mut self, chunks: Vec<ChunkRecord>) -> Result<(), String>;
  fn delete_by_file(&mut self, project_id: &str, file_id: &str) -> Result<usize, String>;
  fn search(
    &self,
    query_embedding: &[f32],
    project_ids: &[String],
    top_k: usize,
  ) -> Result<Vec<ChunkHit>, String>;
  fn upsert_file_manifest(&mut self, record: FileRecord) -> Result<(), String>;
}

pub trait RagManifestStore: RagStore {
  fn list_files(&self, project_id: &str) -> Result<Vec<FileRecord>, String>;
  fn get_file_manifest(
    &self,
    project_id: &str,
    file_id: &str,
  ) -> Result<Option<FileRecord>, String>;
}

pub struct MemoryStore {
  chunks: Vec<ChunkRecord>,
  files: HashMap<(String, String), FileRecord>,
}

impl MemoryStore {
  pub fn new() -> Self {
    Self {
      chunks: Vec::new(),
      files: HashMap::new(),
    }
  }
}

#[cfg(test)]
impl MemoryStore {
  pub fn chunk_count(&self) -> usize {
    self.chunks.len()
  }

  pub fn chunk_count_for_file(&self, project_id: &str, file_id: &str) -> usize {
    self
      .chunks
      .iter()
      .filter(|chunk| chunk.project_id == project_id && chunk.file_id == file_id)
      .count()
  }

  pub fn file_record(&self, project_id: &str, file_id: &str) -> Option<FileRecord> {
    self
      .files
      .get(&(project_id.to_string(), file_id.to_string()))
      .cloned()
  }
}

impl RagStore for MemoryStore {
  fn add_chunks(&mut self, chunks: Vec<ChunkRecord>) -> Result<(), String> {
    self.chunks.extend(chunks);
    Ok(())
  }

  fn delete_by_file(&mut self, project_id: &str, file_id: &str) -> Result<usize, String> {
    let before = self.chunks.len();
    self.chunks.retain(|chunk| !(chunk.project_id == project_id && chunk.file_id == file_id));
    Ok(before - self.chunks.len())
  }

  fn search(
    &self,
    query_embedding: &[f32],
    project_ids: &[String],
    top_k: usize,
  ) -> Result<Vec<ChunkHit>, String> {
    let mut hits: Vec<ChunkHit> = self
      .chunks
      .iter()
      .filter(|chunk| project_ids.contains(&chunk.project_id))
      .filter_map(|chunk| {
        if chunk.embedding.len() != query_embedding.len() {
          return None;
        }
        let score = cosine_similarity(&chunk.embedding, query_embedding);
        Some(ChunkHit {
          project_id: chunk.project_id.clone(),
          file_id: chunk.file_id.clone(),
          file_path: chunk.file_path.clone(),
          chunk_id: chunk.chunk_id.clone(),
          chunk_index: chunk.chunk_index,
          text: chunk.text.clone(),
          score,
        })
      })
      .collect();
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(top_k);
    Ok(hits)
  }

  fn upsert_file_manifest(&mut self, record: FileRecord) -> Result<(), String> {
    self
      .files
      .insert((record.project_id.clone(), record.file_id.clone()), record);
    Ok(())
  }
}

impl RagManifestStore for MemoryStore {
  fn list_files(&self, project_id: &str) -> Result<Vec<FileRecord>, String> {
    Ok(self
      .files
      .values()
      .filter(|record| record.project_id == project_id)
      .cloned()
      .collect())
  }

  fn get_file_manifest(
    &self,
    project_id: &str,
    file_id: &str,
  ) -> Result<Option<FileRecord>, String> {
    Ok(self
      .files
      .get(&(project_id.to_string(), file_id.to_string()))
      .cloned())
  }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
  let mut dot = 0.0f32;
  let mut norm_left = 0.0f32;
  let mut norm_right = 0.0f32;
  for (a, b) in left.iter().zip(right.iter()) {
    dot += a * b;
    norm_left += a * a;
    norm_right += b * b;
  }
  if norm_left == 0.0 || norm_right == 0.0 {
    return 0.0;
  }
  dot / (norm_left.sqrt() * norm_right.sqrt())
}
