use crate::rag::chunker::chunk_text;
use crate::rag::embedder::{normalize_embeddings, Embedder, FastEmbedder};
use crate::rag::file_filter::{extension_allowed, is_minified_code, should_skip_path};
use crate::rag::lancedb_store::LanceDbStore;
use crate::rag::paths::lancedb_path;
use crate::rag::projects::{get_project_root, upsert_project_root};
use crate::rag::store::{RagManifestStore, RagStore};
use crate::rag::types::{ChunkHit, ChunkRecord, FileRecord, IndexReport, SkippedFile};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};

const DEFAULT_CHUNK_SIZE: usize = 1000;
const DEFAULT_CHUNK_OVERLAP: usize = 150;
const DEFAULT_MAX_FILE_SIZE: u64 = 1_048_576;
const DEFAULT_EMBEDDING_DIMENSION: usize = 384;

const QUERY_PREFIX: &str = "query: ";
const PASSAGE_PREFIX: &str = "passage: ";

pub struct RagService {
    store: Box<dyn RagManifestStore>,
    embedder: Box<dyn Embedder>,
    chunk_size: usize,
    chunk_overlap: usize,
    max_file_size: u64,
}

impl RagService {
    pub fn new<R: Runtime>(app: &AppHandle<R>) -> Result<Self, String> {
        let embedder = Box::new(FastEmbedder::new()?);
        let dimension = embedder.dimension();
        let db_path = lancedb_path(app)?;
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let store = Box::new(LanceDbStore::new(db_path, dimension)?);
        Ok(Self {
            store,
            embedder,
            chunk_size: DEFAULT_CHUNK_SIZE,
            chunk_overlap: DEFAULT_CHUNK_OVERLAP,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        })
    }

    pub fn new_with(store: Box<dyn RagManifestStore>, embedder: Box<dyn Embedder>) -> Self {
        Self {
            store,
            embedder,
            chunk_size: DEFAULT_CHUNK_SIZE,
            chunk_overlap: DEFAULT_CHUNK_OVERLAP,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        }
    }

    pub fn index_add_files<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        project_id: &str,
        file_paths: Vec<PathBuf>,
    ) -> Result<IndexReport, String> {
        let mut report = IndexReport {
            project_id: project_id.to_string(),
            root_dir: None,
            ..IndexReport::default()
        };

        let root_dir = resolve_project_root(app, project_id, &file_paths)?;
        if let Some(root_dir) = root_dir.as_ref() {
            if !root_dir.exists() {
                return Err(format!("root dir not found: {}", root_dir.display()));
            }
            report.root_dir = Some(root_dir.to_string_lossy().to_string());
            let _ = upsert_project_root(app, project_id, root_dir);
        }

        for path in file_paths {
            let Some(candidate) =
                self.prepare_file_candidate(project_id, &path, root_dir.as_deref())?
            else {
                report.skipped_files.push(SkippedFile {
                    path: path.to_string_lossy().to_string(),
                    reason: "filtered".to_string(),
                });
                continue;
            };

            let existing = self
                .store
                .get_file_manifest(project_id, &candidate.file_id)?;

            if let Some(existing) = existing.as_ref() {
                if existing.file_hash == candidate.file_hash && existing.is_deleted != Some(true) {
                    report.skipped_files.push(SkippedFile {
                        path: candidate.file_path.clone(),
                        reason: "unchanged".to_string(),
                    });
                    continue;
                }
                let deleted = self.store.delete_by_file(project_id, &candidate.file_id)?;
                report.chunks_deleted += deleted;
                report.updated_files += 1;
            } else {
                report.indexed_files += 1;
            }

            let chunks = self.build_chunks(project_id, &candidate)?;
            report.chunks_added += chunks.len();
            self.store.add_chunks(chunks)?;

            let file_record = FileRecord {
                project_id: project_id.to_string(),
                file_id: candidate.file_id.clone(),
                file_path: candidate.file_path.clone(),
                file_hash: candidate.file_hash.clone(),
                mtime: candidate.mtime,
                size: candidate.size,
                is_deleted: Some(false),
                updated_at: Utc::now().to_rfc3339(),
            };
            self.store.upsert_file_manifest(file_record)?;
        }

        Ok(report)
    }

    pub fn index_sync_project<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        project_id: &str,
        root_dir_override: Option<PathBuf>,
    ) -> Result<IndexReport, String> {
        let mut report = IndexReport {
            project_id: project_id.to_string(),
            ..IndexReport::default()
        };

        let root_dir = if let Some(root_dir) = root_dir_override {
            if !root_dir.exists() {
                return Err(format!("root dir not found: {}", root_dir.display()));
            }
            let _ = upsert_project_root(app, project_id, &root_dir);
            root_dir
        } else {
            let root = get_project_root(app, project_id)
                .ok_or_else(|| "project root not set".to_string())?;
            if !root.exists() {
                return Err(format!("root dir not found: {}", root.display()));
            }
            root
        };
        report.root_dir = Some(root_dir.to_string_lossy().to_string());

        let candidates = self.scan_project_files(project_id, &root_dir)?;
        let mut current = HashMap::new();
        for candidate in candidates {
            current.insert(candidate.file_id.clone(), candidate);
        }

        let existing_records = self.store.list_files(project_id)?;
        let mut existing: HashMap<String, FileRecord> = HashMap::new();
        for record in existing_records {
            if record.is_deleted == Some(true) {
                continue;
            }
            existing.insert(record.file_id.clone(), record);
        }

        let current_ids: HashSet<String> = current.keys().cloned().collect();
        for (file_id, record) in existing.iter() {
            if !current_ids.contains(file_id) {
                let deleted = self.store.delete_by_file(project_id, file_id)?;
                report.chunks_deleted += deleted;
                report.deleted_files += 1;
                let mut updated = record.clone();
                updated.is_deleted = Some(true);
                updated.updated_at = Utc::now().to_rfc3339();
                self.store.upsert_file_manifest(updated)?;
            }
        }

        for (file_id, candidate) in current.iter() {
            let existing = existing.get(file_id);
            let should_index = match existing {
                None => true,
                Some(record) => record.file_hash != candidate.file_hash,
            };
            if !should_index {
                continue;
            }

            if existing.is_some() {
                let deleted = self.store.delete_by_file(project_id, file_id)?;
                report.chunks_deleted += deleted;
                report.updated_files += 1;
            } else {
                report.indexed_files += 1;
            }

            let chunks = self.build_chunks(project_id, candidate)?;
            report.chunks_added += chunks.len();
            self.store.add_chunks(chunks)?;

            let file_record = FileRecord {
                project_id: project_id.to_string(),
                file_id: candidate.file_id.clone(),
                file_path: candidate.file_path.clone(),
                file_hash: candidate.file_hash.clone(),
                mtime: candidate.mtime,
                size: candidate.size,
                is_deleted: Some(false),
                updated_at: Utc::now().to_rfc3339(),
            };
            self.store.upsert_file_manifest(file_record)?;
        }

        Ok(report)
    }

    pub fn index_remove_files<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        project_id: &str,
        file_paths: Option<Vec<PathBuf>>,
        file_ids: Option<Vec<String>>,
    ) -> Result<IndexReport, String> {
        let mut report = IndexReport {
            project_id: project_id.to_string(),
            ..IndexReport::default()
        };

        let mut ids = Vec::new();
        if let Some(file_ids) = file_ids {
            ids.extend(file_ids);
        }

        if let Some(file_paths) = file_paths {
            let root_dir = get_project_root(app, project_id)
                .ok_or_else(|| "project root not set".to_string())?;
            for path in file_paths {
                let relative = normalize_relative_path(&root_dir, &path)?;
                let file_id = hash_text(&format!("{project_id}:{relative}"));
                ids.push(file_id);
            }
        }

        for file_id in ids {
            let deleted = self.store.delete_by_file(project_id, &file_id)?;
            report.chunks_deleted += deleted;
            report.deleted_files += 1;
            if let Some(record) = self.store.get_file_manifest(project_id, &file_id)? {
                let mut updated = record;
                updated.is_deleted = Some(true);
                updated.updated_at = Utc::now().to_rfc3339();
                self.store.upsert_file_manifest(updated)?;
            }
        }

        Ok(report)
    }

    pub fn search(
        &mut self,
        query: &str,
        project_ids: Vec<String>,
        top_k: usize,
    ) -> Result<Vec<ChunkHit>, String> {
        if project_ids.is_empty() {
            return Err("project_ids is empty".to_string());
        }
        let input = format!("{QUERY_PREFIX}{query}");
        let mut embedding = self.embedder.embed_query(&input)?;
        crate::rag::embedder::normalize_embedding(&mut embedding);
        self.store.search(&embedding, &project_ids, top_k)
    }

    fn build_chunks(
        &mut self,
        project_id: &str,
        candidate: &FileCandidate,
    ) -> Result<Vec<ChunkRecord>, String> {
        let chunks = chunk_text(&candidate.text, self.chunk_size, self.chunk_overlap);
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let mut embed_texts = Vec::with_capacity(chunks.len());
        for chunk in &chunks {
            embed_texts.push(format!("{PASSAGE_PREFIX}{chunk}"));
        }
        let mut embeddings = self.embedder.embed_documents(&embed_texts)?;
        normalize_embeddings(&mut embeddings);

        let mut records = Vec::with_capacity(chunks.len());
        for (index, (chunk, embedding)) in
            chunks.into_iter().zip(embeddings.into_iter()).enumerate()
        {
            records.push(ChunkRecord {
                project_id: project_id.to_string(),
                file_id: candidate.file_id.clone(),
                file_path: candidate.file_path.clone(),
                file_hash: candidate.file_hash.clone(),
                chunk_id: format!("{}:{}", candidate.file_id, index),
                chunk_index: index as i32,
                text: chunk,
                embedding,
                updated_at: Utc::now().to_rfc3339(),
            });
        }
        Ok(records)
    }

    fn scan_project_files(
        &mut self,
        project_id: &str,
        root_dir: &Path,
    ) -> Result<Vec<FileCandidate>, String> {
        let mut candidates = Vec::new();
        for entry in walkdir::WalkDir::new(root_dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| should_skip_path(entry.path()).is_none())
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(candidate) = self.prepare_file_candidate(project_id, path, Some(root_dir))?
            else {
                continue;
            };
            candidates.push(candidate);
        }
        Ok(candidates)
    }

    fn prepare_file_candidate(
        &self,
        project_id: &str,
        path: &Path,
        root_dir: Option<&Path>,
    ) -> Result<Option<FileCandidate>, String> {
        if should_skip_path(path).is_some() {
            return Ok(None);
        }
        if !extension_allowed(path) {
            return Ok(None);
        }
        let text = match read_text(path, self.max_file_size) {
            Ok(text) => text,
            Err(_) => return Ok(None),
        };
        if is_minified_code(path, &text) {
            return Ok(None);
        }
        let relative = if let Some(root_dir) = root_dir {
            normalize_relative_path(root_dir, path)?
        } else {
            normalize_filename_only(path)
        };
        let file_hash = hash_text(text.as_bytes());
        let file_id = hash_text(&format!("{project_id}:{relative}"));
        let metadata = fs::metadata(path).ok();
        let mtime = metadata
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|time| time.as_secs() as i64);
        let size = metadata.map(|meta| meta.len() as i64);

        Ok(Some(FileCandidate {
            file_id,
            file_path: relative,
            file_hash,
            text,
            mtime,
            size,
        }))
    }
}

pub fn delete_project_index<R: Runtime>(
    app: &AppHandle<R>,
    project_id: &str,
) -> Result<(usize, usize), String> {
    let db_path = lancedb_path(app)?;
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let mut store = LanceDbStore::new(db_path, DEFAULT_EMBEDDING_DIMENSION)?;
    store.delete_by_project(project_id)
}

struct FileCandidate {
    file_id: String,
    file_path: String,
    file_hash: String,
    text: String,
    mtime: Option<i64>,
    size: Option<i64>,
}

fn read_text(path: &Path, max_size: u64) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|err| err.to_string())?;
    if metadata.len() > max_size {
        return Err("file too large".to_string());
    }
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    if bytes.iter().any(|value| *value == 0) {
        return Err("binary file".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "decode failed".to_string())
}

fn normalize_relative_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let text = relative.to_string_lossy().replace('\\', "/");
    let text = text.trim_start_matches("./").to_string();
    Ok(text.to_lowercase())
}

fn normalize_filename_only(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_lowercase()
}

fn resolve_project_root<R: Runtime>(
    app: &AppHandle<R>,
    project_id: &str,
    file_paths: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    if let Some(root) = get_project_root(app, project_id) {
        return Ok(Some(root));
    }
    if file_paths.is_empty() {
        return Ok(None);
    }
    let mut common = file_paths
        .iter()
        .filter_map(|path| path.parent().map(|parent| parent.to_path_buf()))
        .collect::<Vec<_>>();
    if common.is_empty() {
        return Ok(None);
    }
    let mut prefix = common.remove(0);
    for path in common {
        prefix = common_prefix(&prefix, &path).unwrap_or_else(|| prefix.clone());
    }
    Ok(Some(prefix))
}

fn common_prefix(left: &Path, right: &Path) -> Option<PathBuf> {
    let left_components: Vec<_> = left.components().collect();
    let right_components: Vec<_> = right.components().collect();
    let mut shared = Vec::new();
    for (a, b) in left_components.iter().zip(right_components.iter()) {
        if a == b {
            shared.push(*a);
        } else {
            break;
        }
    }
    if shared.is_empty() {
        return None;
    }
    let mut path = PathBuf::new();
    for comp in shared {
        path.push(comp.as_os_str());
    }
    Some(path)
}

fn hash_text<T: AsRef<[u8]>>(data: T) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_ref());
    let result = hasher.finalize();
    hex::encode(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::embedder::MockEmbedder;
    use crate::rag::store::{MemoryStore, RagManifestStore, RagStore};
    use once_cell::sync::Lazy;
    use std::sync::{Arc, Mutex};

    static TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    struct SharedStore {
        inner: Arc<Mutex<MemoryStore>>,
    }

    impl RagStore for SharedStore {
        fn add_chunks(&mut self, chunks: Vec<ChunkRecord>) -> Result<(), String> {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| "store poisoned".to_string())?;
            RagStore::add_chunks(&mut *guard, chunks)
        }

        fn delete_by_file(&mut self, project_id: &str, file_id: &str) -> Result<usize, String> {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| "store poisoned".to_string())?;
            RagStore::delete_by_file(&mut *guard, project_id, file_id)
        }

        fn delete_by_project(&mut self, project_id: &str) -> Result<(usize, usize), String> {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| "store poisoned".to_string())?;
            RagStore::delete_by_project(&mut *guard, project_id)
        }

        fn search(
            &self,
            query_embedding: &[f32],
            project_ids: &[String],
            top_k: usize,
        ) -> Result<Vec<ChunkHit>, String> {
            let guard = self
                .inner
                .lock()
                .map_err(|_| "store poisoned".to_string())?;
            RagStore::search(&*guard, query_embedding, project_ids, top_k)
        }

        fn upsert_file_manifest(&mut self, record: FileRecord) -> Result<(), String> {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| "store poisoned".to_string())?;
            RagStore::upsert_file_manifest(&mut *guard, record)
        }
    }

    impl RagManifestStore for SharedStore {
        fn list_files(&self, project_id: &str) -> Result<Vec<FileRecord>, String> {
            let guard = self
                .inner
                .lock()
                .map_err(|_| "store poisoned".to_string())?;
            RagManifestStore::list_files(&*guard, project_id)
        }

        fn get_file_manifest(
            &self,
            project_id: &str,
            file_id: &str,
        ) -> Result<Option<FileRecord>, String> {
            let guard = self
                .inner
                .lock()
                .map_err(|_| "store poisoned".to_string())?;
            RagManifestStore::get_file_manifest(&*guard, project_id, file_id)
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let suffix = format!(
            "{}_{:?}_{}",
            label,
            std::thread::current().id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let dir = std::env::temp_dir().join(format!("ai_shepherd_rag_{suffix}"));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn compute_file_id(project_id: &str, root: &Path, path: &Path) -> String {
        let relative = path.strip_prefix(root).unwrap_or(path);
        let text = relative.to_string_lossy().replace('\\', "/");
        let text = text.trim_start_matches("./").to_lowercase();
        hash_text(format!("{project_id}:{text}"))
    }

    #[test]
    fn index_add_and_search() {
        let _guard = TEST_LOCK.lock().unwrap();
        let app = tauri::test::mock_app();
        let app_handle = app.handle();

        let root = temp_root("add");
        let file1 = root.join("file1.txt");
        let file2 = root.join("file2.md");
        fs::write(&file1, "alpha beta gamma").unwrap();
        fs::write(&file2, "delta epsilon zeta").unwrap();

        let store = Arc::new(Mutex::new(MemoryStore::new()));
        let shared = SharedStore {
            inner: store.clone(),
        };
        let embedder = Box::new(MockEmbedder::new(8));
        let mut service = RagService::new_with(Box::new(shared), embedder);

        let report = service
            .index_add_files(&app_handle, "proj_add", vec![file1.clone(), file2.clone()])
            .unwrap();

        assert_eq!(report.indexed_files, 2);
        assert!(report.chunks_added >= 2);
        assert!(report.skipped_files.is_empty());
        assert!(store.lock().unwrap().chunk_count() > 0);

        let hits = service
            .search("alpha", vec!["proj_add".to_string()], 5)
            .unwrap();
        assert!(!hits.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sync_removes_deleted_file() {
        let _guard = TEST_LOCK.lock().unwrap();
        let app = tauri::test::mock_app();
        let app_handle = app.handle();

        let root = temp_root("sync");
        let file1 = root.join("keep.txt");
        let file2 = root.join("remove.txt");
        fs::write(&file1, "keep this file").unwrap();
        fs::write(&file2, "remove this file").unwrap();

        let store = Arc::new(Mutex::new(MemoryStore::new()));
        let shared = SharedStore {
            inner: store.clone(),
        };
        let embedder = Box::new(MockEmbedder::new(8));
        let mut service = RagService::new_with(Box::new(shared), embedder);

        service
            .index_add_files(&app_handle, "proj_sync", vec![file1.clone(), file2.clone()])
            .unwrap();

        let file2_id = compute_file_id("proj_sync", &root, &file2);
        fs::remove_file(&file2).unwrap();

        let report = service
            .index_sync_project(&app_handle, "proj_sync", Some(root.clone()))
            .unwrap();

        assert_eq!(report.deleted_files, 1);
        let guard = store.lock().unwrap();
        assert_eq!(guard.chunk_count_for_file("proj_sync", &file2_id), 0);
        let record = guard.file_record("proj_sync", &file2_id).unwrap();
        assert_eq!(record.is_deleted, Some(true));

        let _ = fs::remove_dir_all(&root);
    }
}
