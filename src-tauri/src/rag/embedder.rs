use std::sync::Arc;

pub trait Embedder: Send + Sync {
  fn embed_documents(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
  fn embed_query(&mut self, text: &str) -> Result<Vec<f32>, String>;
  fn dimension(&self) -> usize;
}

pub struct FastEmbedder {
  model: fastembed::TextEmbedding,
  dimension: usize,
}

impl FastEmbedder {
  pub fn new() -> Result<Self, String> {
    let options =
      fastembed::TextInitOptions::new(fastembed::EmbeddingModel::MultilingualE5Small);
    let mut model =
      fastembed::TextEmbedding::try_new(options).map_err(|err| err.to_string())?;

    // warm-up to discover dimension
    let test = model
      .embed(vec!["query: test".to_string()], None)
      .map_err(|err| err.to_string())?;
    let dimension = test.get(0).map(|v| v.len()).unwrap_or(0);

    Ok(Self { model, dimension })
  }
}

impl Embedder for FastEmbedder {
  fn embed_documents(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
      return Ok(Vec::new());
    }
    self
      .model
      .embed(texts.to_vec(), None)
      .map_err(|err| err.to_string())
  }

  fn embed_query(&mut self, text: &str) -> Result<Vec<f32>, String> {
    let mut results = self
      .model
      .embed(vec![text.to_string()], None)
      .map_err(|err| err.to_string())?;
    results
      .pop()
      .ok_or_else(|| "embedding missing".to_string())
  }

  fn dimension(&self) -> usize {
    self.dimension
  }
}

pub struct MockEmbedder {
  dimension: usize,
}

impl MockEmbedder {
  pub fn new(dimension: usize) -> Self {
    Self { dimension }
  }
}

impl Embedder for MockEmbedder {
  fn embed_documents(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    Ok(texts
      .iter()
      .map(|text| fake_embedding(text, self.dimension))
      .collect())
  }

  fn embed_query(&mut self, text: &str) -> Result<Vec<f32>, String> {
    Ok(fake_embedding(text, self.dimension))
  }

  fn dimension(&self) -> usize {
    self.dimension
  }
}

fn fake_embedding(text: &str, dimension: usize) -> Vec<f32> {
  let mut seed: u64 = 1469598103934665603;
  for byte in text.as_bytes() {
    seed ^= *byte as u64;
    seed = seed.wrapping_mul(1099511628211);
  }
  let mut out = Vec::with_capacity(dimension);
  let mut value = seed;
  for _ in 0..dimension {
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    let float = (value as f32) / (u64::MAX as f32);
    out.push(float);
  }
  normalize_embedding(&mut out);
  out
}

pub fn normalize_embedding(embedding: &mut [f32]) {
  let mut norm = 0.0f32;
  for value in embedding.iter() {
    norm += value * value;
  }
  let norm = norm.sqrt();
  if norm == 0.0 {
    return;
  }
  for value in embedding.iter_mut() {
    *value /= norm;
  }
}

pub fn normalize_embeddings(embeddings: &mut [Vec<f32>]) {
  for embedding in embeddings {
    normalize_embedding(embedding);
  }
}

pub fn boxed_embedder(embedder: impl Embedder + 'static) -> Box<dyn Embedder> {
  Box::new(embedder)
}

pub type SharedEmbedder = Arc<std::sync::Mutex<Box<dyn Embedder>>>;
