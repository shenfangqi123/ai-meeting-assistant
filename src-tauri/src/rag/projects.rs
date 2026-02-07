use crate::rag::paths::projects_path;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Runtime};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
  pub project_id: String,
  pub root_dir: String,
  pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectsIndex {
  pub projects: Vec<ProjectEntry>,
}

pub fn load_projects<R: Runtime>(app: &AppHandle<R>) -> ProjectsIndex {
  let path = match projects_path(app) {
    Ok(path) => path,
    Err(_) => return ProjectsIndex::default(),
  };
  if let Ok(content) = fs::read_to_string(&path) {
    if let Ok(parsed) = serde_json::from_str::<ProjectsIndex>(&content) {
      return parsed;
    }
  }
  ProjectsIndex::default()
}

pub fn save_projects<R: Runtime>(app: &AppHandle<R>, index: &ProjectsIndex) -> Result<(), String> {
  let path = projects_path(app)?;
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;
  }
  let content = serde_json::to_string_pretty(index).map_err(|err| err.to_string())?;
  fs::write(path, content).map_err(|err| err.to_string())
}

pub fn get_project_root<R: Runtime>(app: &AppHandle<R>, project_id: &str) -> Option<PathBuf> {
  let index = load_projects(app);
  index
    .projects
    .iter()
    .find(|entry| entry.project_id == project_id)
    .map(|entry| PathBuf::from(&entry.root_dir))
}

pub fn upsert_project_root<R: Runtime>(
  app: &AppHandle<R>,
  project_id: &str,
  root_dir: &PathBuf,
) -> Result<(), String> {
  let mut index = load_projects(app);
  let root_dir = root_dir.to_string_lossy().to_string();
  if let Some(entry) = index
    .projects
    .iter_mut()
    .find(|entry| entry.project_id == project_id)
  {
    entry.root_dir = root_dir;
    entry.updated_at = Utc::now().to_rfc3339();
  } else {
    index.projects.push(ProjectEntry {
      project_id: project_id.to_string(),
      root_dir,
      updated_at: Utc::now().to_rfc3339(),
    });
  }
  save_projects(app, &index)
}
