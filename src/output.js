import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const listEl = document.getElementById("segmentList");
const emptyHint = document.getElementById("emptyHint");
const statusEl = document.getElementById("segmentStatus");
const headerPromptEl = document.getElementById("headerPrompt");
const boardEl = document.getElementById("segmentBoard");
const splitBarEl = document.getElementById("columnSplitBar");
const questionSplitBarEl = document.getElementById("questionSplitBar");
const translateToggle = document.getElementById("translateToggle");
const questionsToggle = document.getElementById("questionsToggle");
const autoScrollToggle = document.getElementById("autoScrollToggle");

const liveFinalEl = document.getElementById("liveFinal");
const livePartialEl = document.getElementById("livePartial");
const liveMetaEl = document.getElementById("liveMeta");
const liveSpeakerEl = document.getElementById("liveSpeaker");

const MAIN_SPLIT_STORAGE_KEY = "segment_board_main_split_ratio";
const QUESTION_SPLIT_STORAGE_KEY = "segment_board_question_split_ratio";
const AUTO_SCROLL_STORAGE_KEY = "segment_auto_scroll_enabled";
const DEFAULT_MAIN_SPLIT_RATIO = 0.52;
const MIN_MAIN_SPLIT_RATIO = 0.28;
const MAX_MAIN_SPLIT_RATIO = 0.72;
const DEFAULT_QUESTION_SPLIT_RATIO = 0.5;
const MIN_QUESTION_SPLIT_RATIO = 0.2;
const MAX_QUESTION_SPLIT_RATIO = 0.8;
const SPLIT_BAR_PIXEL_WIDTH = 12;

const segmentMap = new Map();
const rowTranslationRequested = new Set();
const translationInvokeQueue = [];
const translationInvokeQueued = new Set();
const TRANSLATION_INVOKE_INTERVAL_MS = 80;

let translateEnabled = false;
let questionsEnabled = false;
let autoScrollEnabled = false;
let draggingSplit = null;
let translationInvokeRunning = false;
let liveStreamOrder = Number.NEGATIVE_INFINITY;
let liveStreamId = "";
let liveStreamText = "";
let mainSplitRatio = DEFAULT_MAIN_SPLIT_RATIO;
let questionSplitRatio = DEFAULT_QUESTION_SPLIT_RATIO;

const normalizeText = (value) => {
  if (!value) return "";
  return value.replace(/\s+/g, " ").trim();
};

const hasTranslationText = (value) => normalizeText(value).length > 0;

const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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

const orderValue = (info) => {
  const raw = Number(info?.order);
  if (Number.isFinite(raw)) {
    return raw;
  }
  return parseOrder(info);
};

const compareInfoOrder = (left, right) => {
  const leftOrder = orderValue(left);
  const rightOrder = orderValue(right);
  if (leftOrder !== rightOrder) {
    return leftOrder - rightOrder;
  }
  const leftName = left?.name || "";
  const rightName = right?.name || "";
  return leftName.localeCompare(rightName);
};

const saveAutoScrollEnabled = (enabled) => {
  try {
    localStorage.setItem(AUTO_SCROLL_STORAGE_KEY, enabled ? "1" : "0");
  } catch (_) {
    // Ignore unavailable storage.
  }
};

const loadAutoScrollEnabled = () => {
  try {
    return localStorage.getItem(AUTO_SCROLL_STORAGE_KEY) === "1";
  } catch (_) {
    return false;
  }
};

const saveMainSplitRatio = (ratio) => {
  try {
    localStorage.setItem(MAIN_SPLIT_STORAGE_KEY, String(ratio));
  } catch (_) {
    // Ignore unavailable storage.
  }
};

const loadMainSplitRatio = () => {
  try {
    const raw = localStorage.getItem(MAIN_SPLIT_STORAGE_KEY);
    if (!raw) return DEFAULT_MAIN_SPLIT_RATIO;
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) return DEFAULT_MAIN_SPLIT_RATIO;
    return clamp(parsed, MIN_MAIN_SPLIT_RATIO, MAX_MAIN_SPLIT_RATIO);
  } catch (_) {
    return DEFAULT_MAIN_SPLIT_RATIO;
  }
};

const saveQuestionSplitRatio = (ratio) => {
  try {
    localStorage.setItem(QUESTION_SPLIT_STORAGE_KEY, String(ratio));
  } catch (_) {
    // Ignore unavailable storage.
  }
};

const loadQuestionSplitRatio = () => {
  try {
    const raw = localStorage.getItem(QUESTION_SPLIT_STORAGE_KEY);
    if (!raw) return DEFAULT_QUESTION_SPLIT_RATIO;
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) return DEFAULT_QUESTION_SPLIT_RATIO;
    return clamp(parsed, MIN_QUESTION_SPLIT_RATIO, MAX_QUESTION_SPLIT_RATIO);
  } catch (_) {
    return DEFAULT_QUESTION_SPLIT_RATIO;
  }
};

const setMainSplitRatio = (ratio, persist = true) => {
  if (!boardEl) return;
  const clamped = clamp(ratio, MIN_MAIN_SPLIT_RATIO, MAX_MAIN_SPLIT_RATIO);
  mainSplitRatio = clamped;
  boardEl.style.setProperty("--left-width", `${(clamped * 100).toFixed(2)}%`);
  if (persist) {
    saveMainSplitRatio(clamped);
  }
};

const setQuestionSplitRatio = (ratio, persist = true) => {
  if (!boardEl) return;
  const clamped = clamp(ratio, MIN_QUESTION_SPLIT_RATIO, MAX_QUESTION_SPLIT_RATIO);
  questionSplitRatio = clamped;
  boardEl.style.setProperty("--middle-share", `${clamped.toFixed(4)}`);
  if (persist) {
    saveQuestionSplitRatio(clamped);
  }
};

const applyBoardLayout = () => {
  if (!boardEl) return;
  const hasSecondary = translateEnabled || questionsEnabled;
  const dualPanels = translateEnabled && questionsEnabled;
  boardEl.classList.toggle("translation-hidden", !translateEnabled);
  boardEl.classList.toggle("questions-hidden", !questionsEnabled);
  boardEl.classList.toggle("no-secondary", !hasSecondary);
  if (hasSecondary) {
    setMainSplitRatio(mainSplitRatio, false);
  }
  if (dualPanels) {
    setQuestionSplitRatio(questionSplitRatio, false);
  }
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
    entry.translationEl.textContent = "";
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

const renderRowQuestion = (entry) => {
  entry.questionEl.textContent = "";
  entry.questionEl.dataset.state = "pending";
};

const renderRow = (entry) => {
  renderRowTranscript(entry);
  renderRowTranslation(entry);
  renderRowQuestion(entry);
};

const clearQueuedRowTranslations = () => {
  for (const name of translationInvokeQueue) {
    rowTranslationRequested.delete(name);
  }
  translationInvokeQueue.length = 0;
  translationInvokeQueued.clear();
};

const drainRowTranslationQueue = async () => {
  if (translationInvokeRunning) return;
  translationInvokeRunning = true;

  let provider = "ollama";
  try {
    provider = await getTranslateProvider();
  } catch (_) {
    // Keep fallback provider.
  }

  while (translationInvokeQueue.length > 0) {
    const name = translationInvokeQueue.shift();
    if (!name) {
      continue;
    }
    translationInvokeQueued.delete(name);

    if (!translateEnabled) {
      rowTranslationRequested.delete(name);
      continue;
    }

    const entry = segmentMap.get(name);
    if (!entry) {
      rowTranslationRequested.delete(name);
      continue;
    }
    if (!normalizeText(entry.info.transcript) || hasTranslationText(entry.info.translation)) {
      rowTranslationRequested.delete(name);
      continue;
    }

    try {
      await invoke("translate_segment", { name, provider });
    } catch (error) {
      rowTranslationRequested.delete(name);
      console.warn("translate_segment enqueue error", error);
      entry.translationEl.textContent = "Translation failed";
      entry.translationEl.dataset.state = "error";
    }

    if (translationInvokeQueue.length > 0) {
      await sleep(TRANSLATION_INVOKE_INTERVAL_MS);
    }
  }

  translationInvokeRunning = false;
  if (translationInvokeQueue.length > 0) {
    void drainRowTranslationQueue();
  }
};

const queueRowTranslation = (entry) => {
  if (!translateEnabled || !entry?.info?.name) return;
  const name = entry.info.name;
  if (!normalizeText(entry.info.transcript)) return;
  if (hasTranslationText(entry.info.translation)) return;
  if (rowTranslationRequested.has(name)) return;

  rowTranslationRequested.add(name);
  entry.translationEl.textContent = "";
  entry.translationEl.dataset.state = "pending";

  if (!translationInvokeQueued.has(name)) {
    translationInvokeQueued.add(name);
    translationInvokeQueue.push(name);
  }

  void drainRowTranslationQueue();
};

const queueMissingRowTranslations = () => {
  if (!translateEnabled) return;
  for (const entry of segmentMap.values()) {
    queueRowTranslation(entry);
  }
};

const mergeInfo = (entry, payload) => {
  if (!payload) return;
  for (const [key, value] of Object.entries(payload)) {
    if (value !== undefined) {
      entry.info[key] = value;
    }
  }
  entry.info.order = parseOrder(entry.info);
};

const createRow = (info) => {
  const row = document.createElement("article");
  row.className = "segment-row";
  row.dataset.name = info.name || "";

  const left = document.createElement("div");
  left.className = "cell transcript-cell";

  const transcriptEl = document.createElement("div");
  transcriptEl.className = "entry-text segment-transcript";

  left.appendChild(transcriptEl);

  const dividerMain = document.createElement("div");
  dividerMain.className = "divider-cell divider-main";

  const middle = document.createElement("div");
  middle.className = "cell translation-cell";

  const translationEl = document.createElement("div");
  translationEl.className = "entry-text segment-translation";

  middle.appendChild(translationEl);

  const dividerRight = document.createElement("div");
  dividerRight.className = "divider-cell divider-right";

  const right = document.createElement("div");
  right.className = "cell question-cell";

  const questionEl = document.createElement("div");
  questionEl.className = "entry-text segment-question";

  right.appendChild(questionEl);

  row.appendChild(left);
  row.appendChild(dividerMain);
  row.appendChild(middle);
  row.appendChild(dividerRight);
  row.appendChild(right);

  const entry = {
    row,
    transcriptEl,
    translationEl,
    questionEl,
    info: {
      name: info.name,
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

const compareEntryOrder = (left, right) => compareInfoOrder(left?.info, right?.info);

const rowInsertAnchor = () => {
  if (emptyHint && emptyHint.parentElement === listEl) {
    return emptyHint;
  }
  return null;
};

const insertRowByOrder = (entry) => {
  if (!listEl || !entry?.row) return;
  let insertBefore = null;
  const nodes = listEl.querySelectorAll(".segment-row");
  for (const node of nodes) {
    if (node === entry.row) {
      continue;
    }
    const candidate = segmentMap.get(node.dataset.name || "");
    if (!candidate) {
      continue;
    }
    if (compareEntryOrder(entry, candidate) < 0) {
      insertBefore = node;
      break;
    }
  }
  listEl.insertBefore(entry.row, insertBefore || rowInsertAnchor());
};

const scrollSegmentsToBottom = () => {
  if (!listEl || !autoScrollEnabled) return;
  requestAnimationFrame(() => {
    listEl.scrollTop = listEl.scrollHeight;
  });
};

const addSegment = (info, { scrollToBottom = true } = {}) => {
  if (!info || !info.name) return;
  if (segmentMap.has(info.name)) {
    const existing = segmentMap.get(info.name);
    const previousOrder = existing.info.order;
    mergeInfo(existing, info);
    renderRow(existing);
    if (existing.info.order !== previousOrder) {
      insertRowByOrder(existing);
    }
    return;
  }

  const entry = createRow(info);
  segmentMap.set(info.name, entry);
  insertRowByOrder(entry);
  if (scrollToBottom) {
    scrollSegmentsToBottom();
  }
  updateStatus();
};

const updateSegment = (info) => {
  if (!info || !info.name) return;
  const entry = segmentMap.get(info.name);
  if (!entry) {
    addSegment(info, { scrollToBottom: true });
    return;
  }
  const previousOrder = entry.info.order;
  mergeInfo(entry, info);
  renderRow(entry);
  if (entry.info.order !== previousOrder) {
    insertRowByOrder(entry);
  }
};

const updateBoardUi = () => {
  if (translateToggle) {
    translateToggle.checked = translateEnabled;
  }
  if (questionsToggle) {
    questionsToggle.checked = questionsEnabled;
  }
  applyBoardLayout();
  if (!translateEnabled) {
    clearQueuedRowTranslations();
  }

  for (const entry of segmentMap.values()) {
    renderRow(entry);
  }

  if (translateEnabled) {
    queueMissingRowTranslations();
  }
};

const clearSegmentsUi = () => {
  clearQueuedRowTranslations();
  segmentMap.clear();
  rowTranslationRequested.clear();
  if (listEl) {
    listEl.querySelectorAll(".segment-row").forEach((node) => node.remove());
  }
  resetLiveState();
  updateStatus();
};

const loadSegments = async () => {
  try {
    const segments = await invoke("list_segments");
    const ordered = segments.slice().sort(compareInfoOrder);
    const fragment = document.createDocumentFragment();
    for (const segment of ordered) {
      if (!segment?.name || segmentMap.has(segment.name)) {
        continue;
      }
      const entry = createRow(segment);
      segmentMap.set(segment.name, entry);
      fragment.appendChild(entry.row);
    }
    if (listEl) {
      listEl.insertBefore(fragment, rowInsertAnchor());
    }
    if (autoScrollEnabled) {
      scrollSegmentsToBottom();
    }
    if (translateEnabled) {
      queueMissingRowTranslations();
    }
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
  setLiveFinal("", "pending");
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

const beginMainSplitDrag = (event) => {
  if (!(translateEnabled || questionsEnabled) || !boardEl || !splitBarEl) return;
  draggingSplit = "main";
  splitBarEl.setPointerCapture?.(event.pointerId);
  event.preventDefault();
};

const beginQuestionSplitDrag = (event) => {
  if (!(translateEnabled && questionsEnabled) || !boardEl || !questionSplitBarEl) return;
  draggingSplit = "question";
  questionSplitBarEl.setPointerCapture?.(event.pointerId);
  event.preventDefault();
};

const updateSplitDrag = (event) => {
  if (!draggingSplit || !boardEl) return;
  const bounds = boardEl.getBoundingClientRect();
  if (bounds.width <= 0) return;

  if (draggingSplit === "main") {
    if (!(translateEnabled || questionsEnabled)) return;
    const offsetX = event.clientX - bounds.left;
    const ratio = clamp(offsetX / bounds.width, MIN_MAIN_SPLIT_RATIO, MAX_MAIN_SPLIT_RATIO);
    setMainSplitRatio(ratio);
    return;
  }

  if (draggingSplit === "question") {
    if (!(translateEnabled && questionsEnabled)) return;
    const rightStartPx = bounds.width * mainSplitRatio + SPLIT_BAR_PIXEL_WIDTH;
    const movableWidth = bounds.width - rightStartPx - SPLIT_BAR_PIXEL_WIDTH;
    if (movableWidth <= 0) return;
    const relativeX = event.clientX - bounds.left - rightStartPx;
    const ratio = clamp(
      relativeX / movableWidth,
      MIN_QUESTION_SPLIT_RATIO,
      MAX_QUESTION_SPLIT_RATIO,
    );
    setQuestionSplitRatio(ratio);
  }
};

const stopSplitDrag = () => {
  draggingSplit = null;
};

splitBarEl?.addEventListener("pointerdown", beginMainSplitDrag);
questionSplitBarEl?.addEventListener("pointerdown", beginQuestionSplitDrag);
window.addEventListener("pointermove", updateSplitDrag);
window.addEventListener("pointerup", stopSplitDrag);
window.addEventListener("pointercancel", stopSplitDrag);

translateToggle?.addEventListener("change", () => {
  translateEnabled = !!translateToggle.checked;
  updateBoardUi();
});

questionsToggle?.addEventListener("change", () => {
  questionsEnabled = !!questionsToggle.checked;
  updateBoardUi();
});

autoScrollToggle?.addEventListener("change", () => {
  autoScrollEnabled = !!autoScrollToggle.checked;
  saveAutoScrollEnabled(autoScrollEnabled);
  if (autoScrollEnabled) {
    scrollSegmentsToBottom();
  }
});

listen("segment_created", (event) => {
  if (event?.payload) {
    addSegment(event.payload, { scrollToBottom: true });
  }
});

listen("segment_transcribed", (event) => {
  if (!event?.payload) return;

  updateSegment(event.payload);
  if (translateEnabled) {
    const entry = segmentMap.get(event.payload.name);
    if (entry) {
      queueRowTranslation(entry);
    }
  }
});

listen("segment_translated", (event) => {
  if (event?.payload) {
    rowTranslationRequested.delete(event.payload.name);
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

listen("segment_translation_canceled", () => {
  clearQueuedRowTranslations();
  rowTranslationRequested.clear();
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

mainSplitRatio = loadMainSplitRatio();
questionSplitRatio = loadQuestionSplitRatio();
setMainSplitRatio(mainSplitRatio, false);
setQuestionSplitRatio(questionSplitRatio, false);
autoScrollEnabled = loadAutoScrollEnabled();
if (autoScrollToggle) {
  autoScrollToggle.checked = autoScrollEnabled;
}
resetLiveState();
updateBoardUi();
updateStatus();
void loadSegments();
