use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

const RAG_DIR: &str = "rag";
const PROJECTS_FILE: &str = "projects.json";
const LANCEDB_DIR: &str = "lancedb";

pub fn rag_base_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
  let base = app
    .path()
    .app_data_dir()
    .map_err(|err| err.to_string())?;
  Ok(base.join(RAG_DIR))
}

pub fn projects_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
  Ok(rag_base_dir(app)?.join(PROJECTS_FILE))
}

pub fn lancedb_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
  Ok(rag_base_dir(app)?.join(LANCEDB_DIR))
}
