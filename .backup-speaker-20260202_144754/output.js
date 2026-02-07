import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const listEl = document.getElementById("segmentList");
const emptyHint = document.getElementById("emptyHint");
const statusEl = document.getElementById("segmentStatus");
const player = document.getElementById("player");
const playerMeta = document.getElementById("playerMeta");

const providerToggle = document.getElementById("providerToggle");
let currentProvider = "ollama";

const updateProviderUi = () => {
  if (!providerToggle) return;
  providerToggle.dataset.provider = currentProvider;
  providerToggle.textContent = currentProvider === "ollama" ? "Ollama" : "ChatGPT";
};

providerToggle?.addEventListener("click", () => {
  currentProvider = currentProvider === "ollama" ? "openai" : "ollama";
  updateProviderUi();
});

updateProviderUi();

const segmentMap = new Map();
let currentUrl = null;
let activeRow = null;

const formatDuration = (ms) => {
  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
};

const formatTime = (iso) => {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString();
};

const updateStatus = () => {
  const count = segmentMap.size;
  statusEl.textContent = count ? `已保存 ${count} 条` : "暂无片段";
  if (emptyHint) {
    emptyHint.style.display = count ? "none" : "block";
  }
};

const formatSeconds = (ms) => {
  if (ms === null || ms === undefined) return "";
  const seconds = ms / 1000;
  return `${seconds.toFixed(1)}s`;
};


const setActiveRow = (row) => {
  if (activeRow) {
    activeRow.classList.remove("active");
  }
  activeRow = row;
  if (activeRow) {
    activeRow.classList.add("active");
  }
};

const playSegment = async (info, row) => {
  const bytes = await invoke("read_segment_bytes", { name: info.name });
  const blob = new Blob([new Uint8Array(bytes)], { type: "audio/wav" });
  if (currentUrl) {
    URL.revokeObjectURL(currentUrl);
  }
  currentUrl = URL.createObjectURL(blob);
  player.src = currentUrl;
  await player.play();
  playerMeta.textContent = `${info.name} · ${formatDuration(info.duration_ms)} · ${formatTime(info.created_at)}`;
  setActiveRow(row);
};

const createRow = (info) => {
  const row = document.createElement("div");
  row.className = "segment";

  const title = document.createElement("div");
  title.className = "segment-title";

  const name = document.createElement("strong");
  name.textContent = info.name;
  title.appendChild(name);

  const meta = document.createElement("div");
  meta.className = "segment-meta";
  meta.textContent = `${formatDuration(info.duration_ms)} · ${formatTime(info.created_at)}`;
  title.appendChild(meta);

  const transcript = document.createElement("div");
  transcript.className = "segment-transcript";
  if (info.transcript && info.transcript.trim()) {
    const stamp = formatSeconds(info.transcript_ms);
    transcript.textContent = stamp ? `${info.transcript} ? ${stamp}` : info.transcript;
    transcript.dataset.state = "ready";
  } else {
    transcript.textContent = "转写中...";
    transcript.dataset.state = "pending";
  }
  title.appendChild(transcript);

  const translation = document.createElement("div");
  translation.className = "segment-translation";
  if (info.translation && info.translation.trim()) {
    const stamp = formatSeconds(info.translation_ms);
    translation.textContent = stamp ? `${info.translation} ? ${stamp}` : info.translation;
    translation.dataset.state = "ready";
  } else {
    translation.textContent = "Translating...";
    translation.dataset.state = "pending";
  }
  title.appendChild(translation);

  const button = document.createElement("button");
  button.className = "play-button";
  button.setAttribute("aria-label", "Play");
  button.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z"/></svg>';
  button.addEventListener("click", () => playSegment(info, row));

  const translateButton = document.createElement("button");
  translateButton.className = "translate-button";
  translateButton.setAttribute("aria-label", "Translate");
  translateButton.innerHTML =
    '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 5h8v2H9.8c.7 1.4 1.7 2.6 2.9 3.7.8-.9 1.4-1.8 2-2.9l1.8.9c-.7 1.4-1.6 2.7-2.7 3.8l2.2 2.1-1.4 1.4-2.2-2.1c-1.4 1.2-3 2.2-4.8 3l-.8-1.8c1.6-.7 3-1.5 4.2-2.6-1.4-1.3-2.6-2.9-3.4-4.7H4V5zm11.5 5H18l3 9h-2l-.7-2h-4.6l-.7 2h-2l3.5-9zm-1.4 5h3.2l-1.6-4.5-1.6 4.5z"/></svg>';
  translateButton.addEventListener("click", () => translateSegment(info, row));

  const actions = document.createElement("div");
  actions.className = "segment-actions";
  actions.appendChild(button);
  actions.appendChild(translateButton);

  row.appendChild(title);
  row.appendChild(actions);
  row.addEventListener("dblclick", () => playSegment(info, row));

  return row;
};

const translateSegment = async (info, row) => {
  const translation = row?.querySelector(".segment-translation");
  if (translation) {
    translation.textContent = "Translating...";
    translation.dataset.state = "pending";
  }
  try {
    await invoke("translate_segment", { name: info.name, provider: currentProvider });
  } catch (error) {
    console.warn("translate error", error);
    if (translation) {
      translation.textContent = "Translation failed";
      translation.dataset.state = "error";
    }
  }
};


const addSegment = (info, { prepend = false } = {}) => {
  if (!info || !info.name || segmentMap.has(info.name)) {
    return;
  }
  const row = createRow(info);
  segmentMap.set(info.name, row);
  if (prepend) {
    listEl.prepend(row);
  } else {
    listEl.appendChild(row);
  }
  updateStatus();
};

const updateTranscript = (info) => {
  if (!info || !info.name) return;
  const row = segmentMap.get(info.name);
  if (!row) return;
  const transcript = row.querySelector(".segment-transcript");
  if (!transcript) return;
  if (info.transcript && info.transcript.trim()) {
    const stamp = formatSeconds(info.transcript_ms);
    transcript.textContent = stamp ? `${info.transcript} ? ${stamp}` : info.transcript;
    transcript.dataset.state = "ready";
  } else {
    transcript.textContent = "转写失败";
    transcript.dataset.state = "error";
  }
};

const updateTranslation = (info) => {
  if (!info || !info.name) return;
  const row = segmentMap.get(info.name);
  if (!row) return;
  const translation = row.querySelector(".segment-translation");
  if (!translation) return;
  if (info.translation === null || info.translation === undefined) {
    translation.textContent = "Translating...";
    translation.dataset.state = "pending";
  } else if (info.translation && info.translation.trim()) {
    const stamp = formatSeconds(info.translation_ms);
    translation.textContent = stamp ? `${info.translation} ? ${stamp}` : info.translation;
    translation.dataset.state = "ready";
  } else {
    translation.textContent = "Translation failed";
    translation.dataset.state = "error";
  }
};


const clearSegmentsUi = () => {
  segmentMap.clear();
  listEl.innerHTML = "";
  if (emptyHint) {
    listEl.appendChild(emptyHint);
  }
  if (currentUrl) {
    URL.revokeObjectURL(currentUrl);
    currentUrl = null;
  }
  player.removeAttribute("src");
  player.load();
  playerMeta.textContent = "未选择片段";
  setActiveRow(null);
  updateStatus();
};

const loadSegments = async () => {
  try {
    const segments = await invoke("list_segments");
    segments
      .slice()
      .sort((a, b) => new Date(a.created_at) - new Date(b.created_at))
      .forEach((segment) => addSegment(segment));
  } catch (error) {
    console.warn("load segments error", error);
  } finally {
    updateStatus();
  }
};

listen("segment_created", (event) => {
  if (event?.payload) {
    addSegment(event.payload, { prepend: true });
  }
});

listen("segment_transcribed", (event) => {
  if (event?.payload) {
    updateTranscript(event.payload);
    updateTranslation({ ...event.payload, translation: null });
  }
});

listen("segment_translated", (event) => {
  if (event?.payload) {
    updateTranslation(event.payload);
  }
});

listen("segment_list_cleared", () => {
  clearSegmentsUi();
});

loadSegments();
