const DEFAULT_SOFT_WINDOW: usize = 120;

const BOUNDARIES: [char; 12] = [
  '\n', '。', '！', '？', '.', '!', '?', ';', '；', '、', '，', ',', 
];

pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
  if chunk_size == 0 {
    return Vec::new();
  }

  let chars: Vec<char> = text.chars().collect();
  if chars.is_empty() {
    return Vec::new();
  }

  let overlap = overlap.min(chunk_size.saturating_sub(1));
  let mut chunks = Vec::new();
  let mut start = 0usize;

  while start < chars.len() {
    let mut end = (start + chunk_size).min(chars.len());
    if end < chars.len() {
      if let Some(best) = find_boundary(&chars, start, end) {
        if best > start {
          end = best;
        }
      }
    }

    if end <= start {
      end = (start + chunk_size).min(chars.len());
      if end <= start {
        break;
      }
    }

    let chunk: String = chars[start..end].iter().collect();
    if !chunk.trim().is_empty() {
      chunks.push(chunk);
    }

    if end >= chars.len() {
      break;
    }

    let next_start = end.saturating_sub(overlap);
    if next_start <= start {
      start = end;
    } else {
      start = next_start;
    }
  }

  chunks
}

fn find_boundary(chars: &[char], start: usize, end: usize) -> Option<usize> {
  let window_start = end.saturating_sub(DEFAULT_SOFT_WINDOW).max(start);
  for idx in (window_start..end).rev() {
    let ch = chars[idx];
    if BOUNDARIES.contains(&ch) {
      return Some(idx + 1);
    }
  }
  None
}

#[cfg(test)]
mod tests {
  use super::chunk_text;

  #[test]
  fn chunker_respects_size() {
    let text = "a".repeat(2500);
    let chunks = chunk_text(&text, 1000, 150);
    assert!(!chunks.is_empty());
    assert!(chunks[0].len() <= 1000);
  }

  #[test]
  fn chunker_uses_boundaries() {
    let text = "第一句。\n第二句。\n第三句。";
    let chunks = chunk_text(text, 6, 0);
    assert!(chunks.len() >= 2);
  }
}
