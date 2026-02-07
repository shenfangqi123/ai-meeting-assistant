use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::path::Path;

static ALLOWED_EXTENSIONS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
  HashSet::from([
    "md", "markdown", "txt", "rst", "adoc", "org",
    "rs", "go", "py", "js", "jsx", "ts", "tsx", "java", "kt", "swift",
    "rb", "php", "cs", "fs", "scala", "c", "h", "cc", "cpp", "hpp",
    "json", "toml", "yaml", "yml", "ini", "cfg", "conf", "env",
    "sql", "html", "css", "scss", "less", "xml", "vue", "svelte",
    "sh", "bat", "ps1", "dockerfile", "makefile", "gradle", "properties",
  ])
});

static IGNORED_DIRS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
  HashSet::from([
    ".git", ".hg", ".svn", ".idea", ".vscode",
    "node_modules", "dist", "build", "target", "out", "coverage",
    ".next", ".turbo", ".cache", ".venv", "__pycache__",
  ])
});

static DISALLOWED_EXTENSIONS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
  HashSet::from([
    "png", "jpg", "jpeg", "gif", "bmp", "svg", "webp",
    "mp3", "wav", "flac", "ogg", "mp4", "mkv", "avi", "mov",
    "zip", "rar", "7z", "tar", "gz", "bz2",
    "pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx",
    "exe", "dll", "so", "dylib",
  ])
});

pub fn should_skip_path(path: &Path) -> Option<String> {
  for component in path.components() {
    let name = component.as_os_str().to_string_lossy().to_lowercase();
    if IGNORED_DIRS.contains(name.as_str()) {
      return Some(format!("ignored dir: {name}"));
    }
  }
  None
}

pub fn extension_allowed(path: &Path) -> bool {
  let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    return name.eq_ignore_ascii_case("dockerfile") || name.eq_ignore_ascii_case("makefile");
  };
  let ext = ext.to_lowercase();
  if DISALLOWED_EXTENSIONS.contains(ext.as_str()) {
    return false;
  }
  ALLOWED_EXTENSIONS.contains(ext.as_str())
}

pub fn is_minified_code(path: &Path, text: &str) -> bool {
  let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
    return false;
  };
  let ext = ext.to_lowercase();
  if !matches!(ext.as_str(), "js" | "jsx" | "ts" | "tsx") {
    return false;
  }
  let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
  let name_lower = name.to_lowercase();
  if name_lower.contains(".min.") {
    return true;
  }

  let mut max_line = 0usize;
  let mut lines = 0usize;
  for line in text.lines() {
    lines += 1;
    let len = line.chars().count();
    if len > max_line {
      max_line = len;
    }
    if max_line > 4000 {
      return true;
    }
  }

  if max_line > 2000 && lines < 5 {
    return true;
  }

  if lines <= 3 && text.len() > 200_000 {
    return true;
  }

  false
}

#[cfg(test)]
mod tests {
  use super::{extension_allowed, is_minified_code};
  use std::path::Path;

  #[test]
  fn extension_allows_text() {
    assert!(extension_allowed(Path::new("readme.md")));
    assert!(!extension_allowed(Path::new("image.png")));
  }

  #[test]
  fn minified_detection() {
    let path = Path::new("bundle.min.js");
    assert!(is_minified_code(path, "var a=1;"));
  }
}
