import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const listEl = document.getElementById("segmentList");
const emptyHint = document.getElementById("emptyHint");
const statusEl = document.getElementById("segmentStatus");
const headerPromptEl = document.getElementById("headerPrompt");
const boardEl = document.getElementById("segmentBoard");
const splitBarEl = document.getElementById("columnSplitBar");
const translateToggle = document.getElementById("translateToggle");

const liveFinalEl = document.getElementById("liveFinal");
const livePartialEl = document.getElementById("livePartial");
const liveMetaEl = document.getElementById("liveMeta");
const liveSpeakerEl = document.getElementById("liveSpeaker");

const SPLIT_STORAGE_KEY = "segment_board_split_ratio";
const DEFAULT_SPLIT_RATIO = 0.52;
const MIN_SPLIT_RATIO = 0.28;
const MAX_SPLIT_RATIO = 0.72;

const segmentMap = new Map();
const liveTranslated = new Set();

let translateEnabled = false;
let draggingSplit = false;
let liveStreamOrder = Number.NEGATIVE_INFINITY;
let liveStreamId = "";
let liveStreamText = "";

const normalizeText = (value) => {
  if (!value) return "";
  return value.replace(/\s+/g, " ").trim();
};

const clamp = (value, min, max) => Math.max(min, Math.min(max, value));

const setHeaderPrompt = (text) => {
  if (!headerPromptEl) return;
  const value = normalizeText(text);
  headerPromptEl.textContent = value ? `(${value})` : "";
};

const setLiveSpeaker = (speakerId, mixed) => {
  if (!liveSpeakerEl) return;
  if (mixed || speakerId === null || speakerId === undefined) {
    liveSpeakerEl.textContent = "Speaker ?";
    liveSpeakerEl.dataset.state = "unknown";
    return;
  }
  liveSpeakerEl.textContent = `Speaker ${speakerId}`;
  delete liveSpeakerEl.dataset.state;
};

const setLivePartial = (text) => {
  if (!livePartialEl) return;
  const value = normalizeText(text);
  if (value) {
    livePartialEl.textContent = value;
    livePartialEl.dataset.state = "ready";
    setHeaderPrompt("");
  } else {
    livePartialEl.textContent = "";
    livePartialEl.dataset.state = "pending";
    setHeaderPrompt("Waiting for speech...");
  }
};

const setLiveFinal = (text, state = "ready") => {
  if (!liveFinalEl) return;
  liveFinalEl.textContent = text || "";
  liveFinalEl.dataset.state = state;
  if (liveFinalEl.scrollHeight > liveFinalEl.clientHeight) {
    liveFinalEl.scrollTop = liveFinalEl.scrollHeight;
  }
};

const resetLiveState = () => {
  liveStreamOrder = Number.NEGATIVE_INFINITY;
  liveStreamId = "";
  liveStreamText = "";
  setLiveSpeaker(null, true);
  if (liveMetaEl) {
    liveMetaEl.textContent = "Idle";
  }
  setLivePartial("");
  setLiveFinal("", "pending");
};

const formatDuration = (ms) => {
  const totalSeconds = Math.max(0, Math.round((ms || 0) / 1000));
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

const parseOrder = (info) => {
  if (!info) return Date.now();
  const createdAt = info.created_at ? Date.parse(info.created_at) : NaN;
  if (Number.isFinite(createdAt)) {
    return createdAt;
  }
  const name = info.name || "";
  const match = name.match(/segment_(\d{8})_(\d{6})_(\d{3})/);
  if (!match) return Date.now();

  const year = Number(match[1].slice(0, 4));
  const month = Number(match[1].slice(4, 6)) - 1;
  const day = Number(match[1].slice(6, 8));
  const hour = Number(match[2].slice(0, 2));
  const minute = Number(match[2].slice(2, 4));
  const second = Number(match[2].slice(4, 6));
  const millisecond = Number(match[3]);
  const ts = new Date(year, month, day, hour, minute, second, millisecond).getTime();
  return Number.isFinite(ts) ? ts : Date.now();
};

const saveSplitRatio = (ratio) => {
  try {
    localStorage.setItem(SPLIT_STORAGE_KEY, String(ratio));
  } catch (_) {
    // Ignore unavailable storage.
  }
};

const loadSplitRatio = () => {
  try {
    const raw = localStorage.getItem(SPLIT_STORAGE_KEY);
    if (!raw) return DEFAULT_SPLIT_RATIO;
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) return DEFAULT_SPLIT_RATIO;
    return clamp(parsed, MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
  } catch (_) {
    return DEFAULT_SPLIT_RATIO;
  }
};

const setSplitRatio = (ratio, persist = true) => {
  if (!boardEl) return;
  const clamped = clamp(ratio, MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
  boardEl.style.setProperty("--left-width", `${(clamped * 100).toFixed(2)}%`);
  if (persist) {
    saveSplitRatio(clamped);
  }
};

const setTranslationVisibility = (visible) => {
  if (!boardEl) return;
  boardEl.classList.toggle("translation-hidden", !visible);
};

const updateStatus = () => {
  const count = segmentMap.size;
  if (statusEl) {
    statusEl.textContent = count ? `Saved ${count}` : "No segments";
  }
  if (emptyHint) {
    emptyHint.style.display = count ? "none" : "block";
  }
};

const getTranslateProvider = async () => {
  try {
    const provider = await invoke("get_translate_provider");
    if (provider === "openai" || provider === "ollama") {
      return provider;
    }
  } catch (_) {
    // Fallback when provider state is unavailable.
  }
  return "ollama";
};

const renderRowMeta = (entry) => {
  const { info, nameEl, metaEl } = entry;
  nameEl.textContent = info.name || "";
  metaEl.textContent = `${formatDuration(info.duration_ms)} | ${formatTime(info.created_at)}`;
};

const renderRowTranscript = (entry) => {
  const transcript = normalizeText(entry.info.transcript);
  if (transcript) {
    entry.transcriptEl.textContent = transcript;
    entry.transcriptEl.dataset.state = "ready";
  } else {
    entry.transcriptEl.textContent = "Transcribing...";
    entry.transcriptEl.dataset.state = "pending";
  }
};

const renderRowTranslation = (entry) => {
  if (!translateEnabled) {
    entry.translationEl.textContent = "";
    entry.translationEl.dataset.state = "pending";
    return;
  }

  const translation = entry.info.translation;
  if (translation === null || translation === undefined) {
    entry.translationEl.textContent = "Translating...";
    entry.translationEl.dataset.state = "pending";
    return;
  }

  const cleaned = normalizeText(translation);
  if (cleaned) {
    entry.translationEl.textContent = cleaned;
    entry.translationEl.dataset.state = "ready";
  } else {
    entry.translationEl.textContent = "Translation failed";
    entry.translationEl.dataset.state = "error";
  }
};

const renderRow = (entry) => {
  renderRowMeta(entry);
  renderRowTranscript(entry);
  renderRowTranslation(entry);
};

const mergeInfo = (entry, payload) => {
  if (!payload) return;
  for (const [key, value] of Object.entries(payload)) {
    if (value !== undefined) {
      entry.info[key] = value;
    }
  }
};

const createRow = (info) => {
  const row = document.createElement("article");
  row.className = "segment-row";
  row.dataset.name = info.name || "";

  const left = document.createElement("div");
  left.className = "cell transcript-cell";

  const metaLine = document.createElement("div");
  metaLine.className = "meta-line";

  const nameEl = document.createElement("strong");
  nameEl.className = "segment-name";

  const metaEl = document.createElement("span");
  metaEl.className = "segment-meta";

  metaLine.appendChild(nameEl);
  metaLine.appendChild(metaEl);

  const transcriptEl = document.createElement("div");
  transcriptEl.className = "entry-text segment-transcript";

  left.appendChild(metaLine);
  left.appendChild(transcriptEl);

  const divider = document.createElement("div");
  divider.className = "divider-cell";

  const right = document.createElement("div");
  right.className = "cell translation-cell";

  const translationTitle = document.createElement("div");
  translationTitle.className = "live-title";
  translationTitle.textContent = "translation";

  const translationEl = document.createElement("div");
  translationEl.className = "entry-text segment-translation";

  right.appendChild(translationTitle);
  right.appendChild(translationEl);

  row.appendChild(left);
  row.appendChild(divider);
  row.appendChild(right);

  const entry = {
    row,
    nameEl,
    metaEl,
    transcriptEl,
    translationEl,
    info: {
      name: info.name,
      created_at: info.created_at,
      duration_ms: info.duration_ms,
      transcript: info.transcript,
      translation: info.translation,
      order: parseOrder(info),
    },
  };

  row.addEventListener("mouseenter", () => {
    row.classList.add("hover-linked");
  });
  row.addEventListener("mouseleave", () => {
    row.classList.remove("hover-linked");
  });

  renderRow(entry);
  return entry;
};

const insertRowElement = (row, prepend = false) => {
  if (!listEl || !row) return;
  if (prepend) {
    listEl.insertBefore(row, listEl.firstChild);
  } else {
    listEl.appendChild(row);
  }
};

const addSegment = (info, { prepend = false } = {}) => {
  if (!info || !info.name) return;
  if (segmentMap.has(info.name)) {
    const existing = segmentMap.get(info.name);
    mergeInfo(existing, info);
    renderRow(existing);
    return;
  }

  const entry = createRow(info);
  segmentMap.set(info.name, entry);
  insertRowElement(entry.row, prepend);
  updateStatus();
};

const updateSegment = (info) => {
  if (!info || !info.name) return;
  const entry = segmentMap.get(info.name);
  if (!entry) {
    addSegment(info, { prepend: true });
    return;
  }
  mergeInfo(entry, info);
  renderRow(entry);
};

const translateLiveFinal = async (info) => {
  if (!translateEnabled) return;
  const text = normalizeText(info?.transcript || "");
  if (!text) return;

  const id = info?.name || `${Date.now()}`;
  if (liveTranslated.has(id)) return;
  liveTranslated.add(id);

  const createdAt = info?.created_at ? Date.parse(info.created_at) : NaN;
  const order = Number.isFinite(createdAt) ? createdAt : Date.now();

  try {
    const provider = await getTranslateProvider();
    await invoke("translate_live", {
      text,
      provider,
      name: id,
      order,
    });
  } catch (error) {
    console.warn("translate_live error", error);
  }
};

const updateTranslateUi = () => {
  if (translateToggle) {
    translateToggle.checked = translateEnabled;
  }
  setTranslationVisibility(translateEnabled);
  if (translateEnabled) {
    setSplitRatio(loadSplitRatio(), false);
  }

  for (const entry of segmentMap.values()) {
    renderRowTranslation(entry);
  }
};

const clearSegmentsUi = () => {
  segmentMap.clear();
  if (listEl) {
    listEl.querySelectorAll(".segment-row").forEach((node) => node.remove());
  }
  liveTranslated.clear();
  resetLiveState();
  updateStatus();
};

const loadSegments = async () => {
  try {
    const segments = await invoke("list_segments");
    const ordered = segments
      .slice()
      .sort((a, b) => parseOrder(b) - parseOrder(a));
    ordered.forEach((segment) => addSegment(segment, { prepend: false }));
  } catch (error) {
    console.warn("load segments error", error);
  } finally {
    updateStatus();
  }
};

const applyWindowTranscript = (payload) => {
  const cleaned = normalizeText(payload?.text || "");
  setLivePartial(cleaned);

  if (liveMetaEl) {
    const latency = Number.isFinite(payload?.elapsed_ms)
      ? `${(payload.elapsed_ms / 1000).toFixed(1)}s`
      : "";
    const windowSize = Number.isFinite(payload?.window_ms)
      ? `${(payload.window_ms / 1000).toFixed(1)}s window`
      : "";
    const meta = [windowSize, latency].filter(Boolean).join(" | ");
    liveMetaEl.textContent = meta || "Listening...";
  }

  if (payload && ("speaker_id" in payload || "speaker_mixed" in payload)) {
    setLiveSpeaker(payload.speaker_id, payload.speaker_mixed);
  }
};

const handleLiveTranslationStart = (payload) => {
  const order = Number(payload?.order);
  if (!Number.isFinite(order)) return;
  if (order < liveStreamOrder) return;

  liveStreamOrder = order;
  liveStreamId = payload?.id || "";
  liveStreamText = "";
  setLiveFinal("Translating...", "pending");
};

const handleLiveTranslationChunk = (payload) => {
  const order = Number(payload?.order);
  if (!Number.isFinite(order)) return;

  if (order < liveStreamOrder) {
    return;
  }
  if (order > liveStreamOrder) {
    liveStreamOrder = order;
    liveStreamId = payload?.id || "";
    liveStreamText = "";
  }
  if (liveStreamId && payload?.id && payload.id !== liveStreamId) {
    return;
  }

  const chunk = payload?.chunk || "";
  if (!chunk) return;

  liveStreamText += chunk;
  setLiveFinal(liveStreamText, "ready");
};

const handleLiveTranslationDone = (payload) => {
  const order = Number(payload?.order);
  if (!Number.isFinite(order)) return;
  if (order < liveStreamOrder) return;

  liveStreamOrder = order;
  liveStreamId = payload?.id || "";
  liveStreamText = payload?.translation || "";
  setLiveFinal(liveStreamText || "Translation failed", liveStreamText ? "ready" : "error");
};

const handleLiveTranslationError = (payload) => {
  const order = Number(payload?.order);
  if (!Number.isFinite(order)) return;
  if (order < liveStreamOrder) return;

  liveStreamOrder = order;
  liveStreamId = payload?.id || "";
  liveStreamText = "";
  setLiveFinal(payload?.error || "Translation failed", "error");
};

const beginSplitDrag = (event) => {
  if (!translateEnabled || !boardEl || !splitBarEl) return;
  draggingSplit = true;
  splitBarEl.setPointerCapture?.(event.pointerId);
  event.preventDefault();
};

const updateSplitDrag = (event) => {
  if (!draggingSplit || !boardEl || !translateEnabled) return;
  const bounds = boardEl.getBoundingClientRect();
  if (bounds.width <= 0) return;

  const offsetX = event.clientX - bounds.left;
  const ratio = clamp(offsetX / bounds.width, MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
  setSplitRatio(ratio);
};

const stopSplitDrag = () => {
  draggingSplit = false;
};

splitBarEl?.addEventListener("pointerdown", beginSplitDrag);
window.addEventListener("pointermove", updateSplitDrag);
window.addEventListener("pointerup", stopSplitDrag);
window.addEventListener("pointercancel", stopSplitDrag);

translateToggle?.addEventListener("change", () => {
  translateEnabled = !!translateToggle.checked;
  updateTranslateUi();
});

listen("segment_created", (event) => {
  if (event?.payload) {
    addSegment(event.payload, { prepend: true });
  }
});

listen("segment_transcribed", (event) => {
  if (!event?.payload) return;

  updateSegment(event.payload);
  if (translateEnabled) {
    void translateLiveFinal(event.payload);
  }
});

listen("segment_translated", (event) => {
  if (event?.payload) {
    updateSegment(event.payload);
  }
});

listen("segment_speakered", (event) => {
  if (event?.payload) {
    updateSegment(event.payload);
  }
});

listen("segment_list_cleared", () => {
  clearSegmentsUi();
});

listen("window_transcribed", (event) => {
  if (event?.payload) {
    applyWindowTranscript(event.payload);
  }
});

listen("live_translation_start", (event) => {
  if (event?.payload) {
    handleLiveTranslationStart(event.payload);
  }
});

listen("live_translation_chunk", (event) => {
  if (event?.payload) {
    handleLiveTranslationChunk(event.payload);
  }
});

listen("live_translation_done", (event) => {
  if (event?.payload) {
    handleLiveTranslationDone(event.payload);
  }
});

listen("live_translation_error", (event) => {
  if (event?.payload) {
    handleLiveTranslationError(event.payload);
  }
});

listen("live_translation_cleared", () => {
  resetLiveState();
});

setSplitRatio(loadSplitRatio(), false);
resetLiveState();
updateTranslateUi();
updateStatus();
void loadSegments();
