mod chunker;
mod embedder;
mod file_filter;
mod lancedb_store;
mod paths;
mod projects;
mod service;
mod store;
mod types;

pub use types::{
  IndexAddRequest, IndexRemoveRequest, IndexReport, IndexSyncRequest, RagProject,
  RagProjectCreateRequest, RagProjectDeleteReport, RagProjectDeleteRequest, RagProjectListResponse,
  RagSearchRequest, RagSearchResponse,
};

use projects::{create_project, list_projects, remove_project};
use service::{delete_project_index, RagService};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

pub struct RagState {
  inner: Mutex<Option<RagService>>,
}

impl RagState {
  pub fn new() -> Self {
    Self {
      inner: Mutex::new(None),
    }
  }

  pub fn with_service<T, F>(&self, app: &AppHandle, f: F) -> Result<T, String>
  where
    F: FnOnce(&mut RagService) -> Result<T, String>,
  {
    let mut guard = self.inner.lock().map_err(|_| "rag state poisoned".to_string())?;
    if guard.is_none() {
      *guard = Some(RagService::new(app)?);
    }
    let service = guard.as_mut().ok_or_else(|| "rag init failed".to_string())?;
    f(service)
  }
}

#[tauri::command]
pub async fn rag_index_add_files(
  app: AppHandle,
  state: State<'_, Arc<RagState>>,
  request: IndexAddRequest,
) -> Result<IndexReport, String> {
  let state = state.inner().clone();
  let app = app.clone();
  tauri::async_runtime::spawn_blocking(move || {
    let paths = request
      .file_paths
      .into_iter()
      .map(PathBuf::from)
      .collect();
    state.with_service(&app, |service| service.index_add_files(&app, &request.project_id, paths))
  })
  .await
  .map_err(|err| err.to_string())?
}

#[tauri::command]
pub async fn rag_index_sync_project(
  app: AppHandle,
  state: State<'_, Arc<RagState>>,
  request: IndexSyncRequest,
) -> Result<IndexReport, String> {
  let state = state.inner().clone();
  let app = app.clone();
  tauri::async_runtime::spawn_blocking(move || {
    let root_dir = request.root_dir.map(PathBuf::from);
    state.with_service(&app, |service| {
      service.index_sync_project(&app, &request.project_id, root_dir)
    })
  })
  .await
  .map_err(|err| err.to_string())?
}

#[tauri::command]
pub async fn rag_index_remove_files(
  app: AppHandle,
  state: State<'_, Arc<RagState>>,
  request: IndexRemoveRequest,
) -> Result<IndexReport, String> {
  let state = state.inner().clone();
  let app = app.clone();
  tauri::async_runtime::spawn_blocking(move || {
    let paths = request.file_paths.map(|paths| paths.into_iter().map(PathBuf::from).collect());
    state.with_service(&app, |service| {
      service.index_remove_files(&app, &request.project_id, paths, request.file_ids)
    })
  })
  .await
  .map_err(|err| err.to_string())?
}

#[tauri::command]
pub async fn rag_search(
  app: AppHandle,
  state: State<'_, Arc<RagState>>,
  request: RagSearchRequest,
) -> Result<RagSearchResponse, String> {
  let state = state.inner().clone();
  let app = app.clone();
  tauri::async_runtime::spawn_blocking(move || {
    state.with_service(&app, |service| {
      let top_k = request.top_k.unwrap_or(8);
      let hits = service.search(&request.query, request.project_ids, top_k)?;
      Ok(RagSearchResponse { hits })
    })
  })
  .await
  .map_err(|err| err.to_string())?
}

#[tauri::command]
pub fn rag_pick_folder() -> Option<String> {
  rfd::FileDialog::new()
    .pick_folder()
    .map(|path| path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn rag_project_list(app: AppHandle) -> Result<RagProjectListResponse, String> {
  Ok(RagProjectListResponse {
    projects: list_projects(&app),
  })
}

#[tauri::command]
pub fn rag_project_create(
  app: AppHandle,
  request: RagProjectCreateRequest,
) -> Result<RagProject, String> {
  let root = PathBuf::from(request.root_dir);
  create_project(&app, &request.project_name, &root)
}

#[tauri::command]
pub async fn rag_project_delete(
  app: AppHandle,
  request: RagProjectDeleteRequest,
) -> Result<RagProjectDeleteReport, String> {
  let app = app.clone();
  tauri::async_runtime::spawn_blocking(move || {
    let (deleted_files, deleted_chunks) = delete_project_index(&app, &request.project_id)?;
    let _ = remove_project(&app, &request.project_id)?;
    Ok(RagProjectDeleteReport {
      project_id: request.project_id,
      deleted_files,
      deleted_chunks,
    })
  })
  .await
  .map_err(|err| err.to_string())?
}
