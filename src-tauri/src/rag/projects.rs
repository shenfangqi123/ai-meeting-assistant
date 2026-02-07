use crate::rag::paths::projects_path;
use crate::rag::types::RagProject;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
  pub project_id: String,
  #[serde(default)]
  pub project_name: Option<String>,
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
    if let Ok(mut parsed) = serde_json::from_str::<ProjectsIndex>(&content) {
      normalize_projects(&mut parsed);
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

pub fn list_projects<R: Runtime>(app: &AppHandle<R>) -> Vec<RagProject> {
  let mut projects = load_projects(app)
    .projects
    .into_iter()
    .map(|entry| to_project_dto(&entry))
    .collect::<Vec<_>>();
  projects.sort_by(|a, b| a.project_name.to_lowercase().cmp(&b.project_name.to_lowercase()));
  projects
}

pub fn create_project<R: Runtime>(
  app: &AppHandle<R>,
  project_name: &str,
  root_dir: &Path,
) -> Result<RagProject, String> {
  if !root_dir.exists() {
    return Err(format!("root dir not found: {}", root_dir.display()));
  }
  if !root_dir.is_dir() {
    return Err(format!("root dir is not a directory: {}", root_dir.display()));
  }
  fs::read_dir(root_dir).map_err(|err| format!("root dir not accessible: {err}"))?;

  let canonical = fs::canonicalize(root_dir).unwrap_or_else(|_| root_dir.to_path_buf());
  let canonical_root = canonical.to_string_lossy().to_string();
  let normalized_root = normalize_root_dir(&canonical_root);
  let project_id = hash_project_id(&normalized_root);

  let mut index = load_projects(app);
  if index
    .projects
    .iter()
    .any(|entry| normalize_root_dir(&entry.root_dir) == normalized_root)
  {
    return Err("project root already exists".to_string());
  }

  let now = Utc::now().to_rfc3339();
  let final_name = resolve_project_name(project_name, &canonical_root, &project_id);
  let entry = ProjectEntry {
    project_id: project_id.clone(),
    project_name: Some(final_name),
    root_dir: canonical_root,
    updated_at: now,
  };
  index.projects.push(entry.clone());
  save_projects(app, &index)?;
  Ok(to_project_dto(&entry))
}

pub fn remove_project<R: Runtime>(
  app: &AppHandle<R>,
  project_id: &str,
) -> Result<bool, String> {
  let mut index = load_projects(app);
  let before = index.projects.len();
  index.projects.retain(|entry| entry.project_id != project_id);
  if before == index.projects.len() {
    return Ok(false);
  }
  save_projects(app, &index)?;
  Ok(true)
}

pub fn upsert_project_root<R: Runtime>(
  app: &AppHandle<R>,
  project_id: &str,
  root_dir: &PathBuf,
) -> Result<(), String> {
  let mut index = load_projects(app);
  let canonical = fs::canonicalize(root_dir).unwrap_or_else(|_| root_dir.clone());
  let root_dir = canonical.to_string_lossy().to_string();
  let root_name = resolve_project_name("", &root_dir, project_id);
  if let Some(entry) = index
    .projects
    .iter_mut()
    .find(|entry| entry.project_id == project_id)
  {
    entry.root_dir = root_dir;
    if entry
      .project_name
      .as_deref()
      .map(|name| name.trim().is_empty())
      .unwrap_or(true)
    {
      entry.project_name = Some(root_name);
    }
    entry.updated_at = Utc::now().to_rfc3339();
  } else {
    index.projects.push(ProjectEntry {
      project_id: project_id.to_string(),
      project_name: Some(root_name),
      root_dir,
      updated_at: Utc::now().to_rfc3339(),
    });
  }
  save_projects(app, &index)
}

fn normalize_projects(index: &mut ProjectsIndex) {
  for entry in &mut index.projects {
    if entry
      .project_name
      .as_deref()
      .map(|name| name.trim().is_empty())
      .unwrap_or(true)
    {
      entry.project_name = Some(resolve_project_name("", &entry.root_dir, &entry.project_id));
    }
  }
}

fn to_project_dto(entry: &ProjectEntry) -> RagProject {
  RagProject {
    project_id: entry.project_id.clone(),
    project_name: resolve_project_name(
      entry.project_name.as_deref().unwrap_or(""),
      &entry.root_dir,
      &entry.project_id,
    ),
    root_dir: entry.root_dir.clone(),
    updated_at: entry.updated_at.clone(),
  }
}

fn resolve_project_name(input: &str, root_dir: &str, fallback: &str) -> String {
  let trimmed = input.trim();
  if !trimmed.is_empty() {
    return trimmed.to_string();
  }
  Path::new(root_dir)
    .file_name()
    .and_then(|name| name.to_str())
    .filter(|name| !name.trim().is_empty())
    .map(|name| name.to_string())
    .unwrap_or_else(|| fallback.to_string())
}

fn normalize_root_dir(root_dir: &str) -> String {
  let mut normalized = root_dir.replace('\\', "/").trim().to_string();
  while normalized.ends_with('/') {
    normalized.pop();
  }
  if cfg!(windows) {
    normalized.to_lowercase()
  } else {
    normalized
  }
}

fn hash_project_id(text: &str) -> String {
  let mut hasher = Sha256::new();
  hasher.update(text.as_bytes());
  let bytes = hasher.finalize();
  format!("proj_{}", hex::encode(bytes))
}
