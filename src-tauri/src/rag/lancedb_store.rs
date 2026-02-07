use crate::rag::store::{RagManifestStore, RagStore};
use crate::rag::types::{ChunkHit, ChunkRecord, FileRecord};
use arrow_array::{
  Array, ArrayRef, BooleanArray, Float32Array, Float64Array, FixedSizeListArray, Int32Array,
  Int64Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures_util::TryStreamExt;
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::Table;
use std::path::PathBuf;
use std::sync::Arc;

const CHUNKS_TABLE: &str = "chunks";
const FILES_TABLE: &str = "files";

pub struct LanceDbStore {
  db: Connection,
  chunks: Table,
  files: Table,
  dimension: usize,
}

impl LanceDbStore {
  pub fn new(path: PathBuf, dimension: usize) -> Result<Self, String> {
    let path_str = path.to_string_lossy().to_string();
    let (db, chunks, files) = tauri::async_runtime::block_on(async move {
      let db = lancedb::connect(&path_str).execute().await.map_err(|err| err.to_string())?;
      let chunks_schema = chunks_schema(dimension);
      let files_schema = files_schema();

      let chunks = open_or_create_table(&db, CHUNKS_TABLE, chunks_schema).await?;
      let files = open_or_create_table(&db, FILES_TABLE, files_schema).await?;
      Ok::<_, String>((db, chunks, files))
    })?;

    Ok(Self {
      db,
      chunks,
      files,
      dimension,
    })
  }
}

impl RagStore for LanceDbStore {
  fn add_chunks(&mut self, chunks: Vec<ChunkRecord>) -> Result<(), String> {
    if chunks.is_empty() {
      return Ok(());
    }
    let batch = chunks_to_batch(&chunks, self.dimension)?;
    let schema = batch.schema();
    let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    tauri::async_runtime::block_on(async {
      self
        .chunks
        .add(reader)
        .execute()
        .await
        .map_err(|err| err.to_string())
    })
  }

  fn delete_by_file(&mut self, project_id: &str, file_id: &str) -> Result<usize, String> {
    let filter = format!(
      "project_id = '{}' AND file_id = '{}'",
      escape_literal(project_id),
      escape_literal(file_id)
    );
    tauri::async_runtime::block_on(async {
      self
        .chunks
        .delete(&filter)
        .await
        .map_err(|err| err.to_string())
    })?;
    Ok(0)
  }

  fn search(
    &self,
    query_embedding: &[f32],
    project_ids: &[String],
    top_k: usize,
  ) -> Result<Vec<ChunkHit>, String> {
    let filter = build_project_filter(project_ids);
    tauri::async_runtime::block_on(async {
      let mut query = self
        .chunks
        .vector_search(query_embedding.to_vec())
        .map_err(|err| err.to_string())?
        .column("embedding");
      if let Some(filter) = filter {
        query = query.only_if(filter);
      }
      let stream = query
        .limit(top_k)
        .execute()
        .await
        .map_err(|err| err.to_string())?;

      let batches: Vec<RecordBatch> = stream.try_collect().await.map_err(|err| err.to_string())?;
      let mut hits = Vec::new();
      for batch in batches {
        hits.extend(parse_chunk_hits(&batch)?);
      }
      Ok(hits)
    })
  }

  fn upsert_file_manifest(&mut self, record: FileRecord) -> Result<(), String> {
    let filter = format!(
      "project_id = '{}' AND file_id = '{}'",
      escape_literal(&record.project_id),
      escape_literal(&record.file_id)
    );
    tauri::async_runtime::block_on(async {
      self
        .files
        .delete(&filter)
        .await
        .map_err(|err| err.to_string())?;

      let batch = files_to_batch(&[record])?;
      let schema = batch.schema();
      let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
      self
        .files
        .add(reader)
        .execute()
        .await
        .map_err(|err| err.to_string())
    })
  }
}

impl RagManifestStore for LanceDbStore {
  fn list_files(&self, project_id: &str) -> Result<Vec<FileRecord>, String> {
    tauri::async_runtime::block_on(async {
      let filter = format!("project_id = '{}'", escape_literal(project_id));
      let stream = self
        .files
        .query()
        .only_if(filter)
        .execute()
        .await
        .map_err(|err| err.to_string())?;

      let batches: Vec<RecordBatch> = stream.try_collect().await.map_err(|err| err.to_string())?;
      let mut records = Vec::new();
      for batch in batches {
        records.extend(parse_file_records(&batch)?);
      }
      Ok(records)
    })
  }

  fn get_file_manifest(
    &self,
    project_id: &str,
    file_id: &str,
  ) -> Result<Option<FileRecord>, String> {
    tauri::async_runtime::block_on(async {
      let filter = format!(
        "project_id = '{}' AND file_id = '{}'",
        escape_literal(project_id),
        escape_literal(file_id)
      );
      let stream = self
        .files
        .query()
        .only_if(filter)
        .limit(1)
        .execute()
        .await
        .map_err(|err| err.to_string())?;

      let mut batches: Vec<RecordBatch> = stream.try_collect().await.map_err(|err| err.to_string())?;
      if let Some(batch) = batches.pop() {
        let mut records = parse_file_records(&batch)?;
        Ok(records.pop())
      } else {
        Ok(None)
      }
    })
  }
}

async fn open_or_create_table(
  db: &Connection,
  name: &str,
  schema: Schema,
) -> Result<Table, String> {
  let tables = db
    .table_names()
    .execute()
    .await
    .map_err(|err| err.to_string())?;
  if tables.iter().any(|table| table == name) {
    db.open_table(name).execute().await.map_err(|err| err.to_string())
  } else {
    db.create_empty_table(name, Arc::new(schema))
      .execute()
      .await
      .map_err(|err| err.to_string())
  }
}

fn chunks_schema(dimension: usize) -> Schema {
  let embedding_field = Field::new(
    "embedding",
    DataType::FixedSizeList(
      Arc::new(Field::new("item", DataType::Float32, false)),
      dimension as i32,
    ),
    false,
  );
  Schema::new(vec![
    Field::new("project_id", DataType::Utf8, false),
    Field::new("file_id", DataType::Utf8, false),
    Field::new("file_path", DataType::Utf8, false),
    Field::new("file_hash", DataType::Utf8, false),
    Field::new("chunk_id", DataType::Utf8, false),
    Field::new("chunk_index", DataType::Int32, false),
    Field::new("text", DataType::Utf8, false),
    embedding_field,
    Field::new("updated_at", DataType::Utf8, false),
  ])
}

fn files_schema() -> Schema {
  Schema::new(vec![
    Field::new("project_id", DataType::Utf8, false),
    Field::new("file_id", DataType::Utf8, false),
    Field::new("file_path", DataType::Utf8, false),
    Field::new("file_hash", DataType::Utf8, false),
    Field::new("mtime", DataType::Int64, true),
    Field::new("size", DataType::Int64, true),
    Field::new("is_deleted", DataType::Boolean, true),
    Field::new("updated_at", DataType::Utf8, false),
  ])
}

fn chunks_to_batch(chunks: &[ChunkRecord], dimension: usize) -> Result<RecordBatch, String> {
  let project_ids = StringArray::from(
    chunks
      .iter()
      .map(|c| c.project_id.as_str())
      .collect::<Vec<_>>(),
  );
  let file_ids = StringArray::from(
    chunks
      .iter()
      .map(|c| c.file_id.as_str())
      .collect::<Vec<_>>(),
  );
  let file_paths = StringArray::from(
    chunks
      .iter()
      .map(|c| c.file_path.as_str())
      .collect::<Vec<_>>(),
  );
  let file_hashes = StringArray::from(
    chunks
      .iter()
      .map(|c| c.file_hash.as_str())
      .collect::<Vec<_>>(),
  );
  let chunk_ids = StringArray::from(
    chunks
      .iter()
      .map(|c| c.chunk_id.as_str())
      .collect::<Vec<_>>(),
  );
  let chunk_indexes = Int32Array::from(
    chunks
      .iter()
      .map(|c| c.chunk_index)
      .collect::<Vec<_>>(),
  );
  let texts = StringArray::from(chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>());
  let updated_at =
    StringArray::from(chunks.iter().map(|c| c.updated_at.as_str()).collect::<Vec<_>>());

  let mut flat = Vec::with_capacity(chunks.len() * dimension);
  for chunk in chunks {
    if chunk.embedding.len() != dimension {
      return Err("embedding dimension mismatch".to_string());
    }
    flat.extend_from_slice(&chunk.embedding);
  }
  let values = Float32Array::from(flat);
  let list_field = Arc::new(Field::new("item", DataType::Float32, false));
  let embedding = FixedSizeListArray::try_new(
    list_field.clone(),
    dimension as i32,
    Arc::new(values) as ArrayRef,
    None,
  )
  .map_err(|err| err.to_string())?;

  let schema = Arc::new(chunks_schema(dimension));
  RecordBatch::try_new(
    schema,
    vec![
      Arc::new(project_ids) as ArrayRef,
      Arc::new(file_ids),
      Arc::new(file_paths),
      Arc::new(file_hashes),
      Arc::new(chunk_ids),
      Arc::new(chunk_indexes),
      Arc::new(texts),
      Arc::new(embedding),
      Arc::new(updated_at),
    ],
  )
  .map_err(|err| err.to_string())
}

fn files_to_batch(records: &[FileRecord]) -> Result<RecordBatch, String> {
  let project_ids = StringArray::from(
    records
      .iter()
      .map(|c| c.project_id.as_str())
      .collect::<Vec<_>>(),
  );
  let file_ids =
    StringArray::from(records.iter().map(|c| c.file_id.as_str()).collect::<Vec<_>>());
  let file_paths =
    StringArray::from(records.iter().map(|c| c.file_path.as_str()).collect::<Vec<_>>());
  let file_hashes =
    StringArray::from(records.iter().map(|c| c.file_hash.as_str()).collect::<Vec<_>>());
  let mtimes = Int64Array::from(records.iter().map(|c| c.mtime).collect::<Vec<_>>());
  let sizes = Int64Array::from(records.iter().map(|c| c.size).collect::<Vec<_>>());
  let is_deleted =
    BooleanArray::from(records.iter().map(|c| c.is_deleted).collect::<Vec<_>>());
  let updated_at =
    StringArray::from(records.iter().map(|c| c.updated_at.as_str()).collect::<Vec<_>>());

  let schema = Arc::new(files_schema());
  RecordBatch::try_new(
    schema,
    vec![
      Arc::new(project_ids) as ArrayRef,
      Arc::new(file_ids),
      Arc::new(file_paths),
      Arc::new(file_hashes),
      Arc::new(mtimes),
      Arc::new(sizes),
      Arc::new(is_deleted),
      Arc::new(updated_at),
    ],
  )
  .map_err(|err| err.to_string())
}

fn parse_chunk_hits(batch: &RecordBatch) -> Result<Vec<ChunkHit>, String> {
  let project_ids = batch
    .column_by_name("project_id")
    .ok_or_else(|| "project_id missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "project_id type mismatch".to_string())?;
  let file_ids = batch
    .column_by_name("file_id")
    .ok_or_else(|| "file_id missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "file_id type mismatch".to_string())?;
  let file_paths = batch
    .column_by_name("file_path")
    .ok_or_else(|| "file_path missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "file_path type mismatch".to_string())?;
  let chunk_ids = batch
    .column_by_name("chunk_id")
    .ok_or_else(|| "chunk_id missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "chunk_id type mismatch".to_string())?;
  let chunk_indexes = batch
    .column_by_name("chunk_index")
    .ok_or_else(|| "chunk_index missing".to_string())?
    .as_any()
    .downcast_ref::<Int32Array>()
    .ok_or_else(|| "chunk_index type mismatch".to_string())?;
  let texts = batch
    .column_by_name("text")
    .ok_or_else(|| "text missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "text type mismatch".to_string())?;

  let scores = batch
    .column_by_name("_score")
    .or_else(|| batch.column_by_name("_distance"));

  let mut hits = Vec::with_capacity(batch.num_rows());
  for row in 0..batch.num_rows() {
    let score = match scores {
      Some(column) => {
        if let Some(array) = column.as_any().downcast_ref::<Float32Array>() {
          array.value(row)
        } else if let Some(array) = column.as_any().downcast_ref::<Float64Array>() {
          array.value(row) as f32
        } else {
          0.0
        }
      }
      None => 0.0,
    };

    hits.push(ChunkHit {
      project_id: project_ids.value(row).to_string(),
      file_id: file_ids.value(row).to_string(),
      file_path: file_paths.value(row).to_string(),
      chunk_id: chunk_ids.value(row).to_string(),
      chunk_index: chunk_indexes.value(row),
      text: texts.value(row).to_string(),
      score,
    });
  }

  Ok(hits)
}

fn parse_file_records(batch: &RecordBatch) -> Result<Vec<FileRecord>, String> {
  let project_ids = batch
    .column_by_name("project_id")
    .ok_or_else(|| "project_id missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "project_id type mismatch".to_string())?;
  let file_ids = batch
    .column_by_name("file_id")
    .ok_or_else(|| "file_id missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "file_id type mismatch".to_string())?;
  let file_paths = batch
    .column_by_name("file_path")
    .ok_or_else(|| "file_path missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "file_path type mismatch".to_string())?;
  let file_hashes = batch
    .column_by_name("file_hash")
    .ok_or_else(|| "file_hash missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "file_hash type mismatch".to_string())?;
  let mtimes = batch
    .column_by_name("mtime")
    .ok_or_else(|| "mtime missing".to_string())?
    .as_any()
    .downcast_ref::<Int64Array>()
    .ok_or_else(|| "mtime type mismatch".to_string())?;
  let sizes = batch
    .column_by_name("size")
    .ok_or_else(|| "size missing".to_string())?
    .as_any()
    .downcast_ref::<Int64Array>()
    .ok_or_else(|| "size type mismatch".to_string())?;
  let is_deleted = batch
    .column_by_name("is_deleted")
    .ok_or_else(|| "is_deleted missing".to_string())?
    .as_any()
    .downcast_ref::<BooleanArray>()
    .ok_or_else(|| "is_deleted type mismatch".to_string())?;
  let updated_at = batch
    .column_by_name("updated_at")
    .ok_or_else(|| "updated_at missing".to_string())?
    .as_any()
    .downcast_ref::<StringArray>()
    .ok_or_else(|| "updated_at type mismatch".to_string())?;

  let mut records = Vec::with_capacity(batch.num_rows());
  for row in 0..batch.num_rows() {
    records.push(FileRecord {
      project_id: project_ids.value(row).to_string(),
      file_id: file_ids.value(row).to_string(),
      file_path: file_paths.value(row).to_string(),
      file_hash: file_hashes.value(row).to_string(),
      mtime: if mtimes.is_null(row) {
        None
      } else {
        Some(mtimes.value(row))
      },
      size: if sizes.is_null(row) {
        None
      } else {
        Some(sizes.value(row))
      },
      is_deleted: if is_deleted.is_null(row) {
        None
      } else {
        Some(is_deleted.value(row))
      },
      updated_at: updated_at.value(row).to_string(),
    });
  }
  Ok(records)
}

fn build_project_filter(project_ids: &[String]) -> Option<String> {
  if project_ids.is_empty() {
    return None;
  }
  let list = project_ids
    .iter()
    .map(|id| format!("'{}'", escape_literal(id)))
    .collect::<Vec<_>>()
    .join(",");
  Some(format!("project_id IN ({})", list))
}

fn escape_literal(input: &str) -> String {
  input.replace('\'', "''")
}
