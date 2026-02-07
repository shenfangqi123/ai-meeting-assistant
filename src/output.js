import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const listEl = document.getElementById("segmentList");
const emptyHint = document.getElementById("emptyHint");
const statusEl = document.getElementById("segmentStatus");
const liveFinalEl = document.getElementById("liveFinal");
const livePartialEl = document.getElementById("livePartial");
const liveMetaEl = document.getElementById("liveMeta");
const liveSpeakerEl = document.getElementById("liveSpeaker");

const translateToggle = document.getElementById("translateToggle");
let translateEnabled = false;

const getTranslateProvider = async () => {
  try {
    const provider = await invoke("get_translate_provider");
    if (provider === "openai" || provider === "ollama") {
      return provider;
    }
  } catch (_) {
    // Fallback to ollama if provider state is unavailable.
  }
  return "ollama";
};

const updateTranslateUi = () => {
  if (translateToggle) {
    translateToggle.checked = translateEnabled;
  }
  for (const row of segmentMap.values()) {
    const button = row.querySelector(".translate-button");
    if (button) {
      button.disabled = !translateEnabled;
      button.dataset.state = translateEnabled ? "ready" : "disabled";
    }
    const translation = row.querySelector(".segment-translation");
    if (translation && !translateEnabled) {
      translation.textContent = "Translation off";
      translation.dataset.state = "pending";
    }
  }
};

translateToggle?.addEventListener("change", () => {
  translateEnabled = translateToggle.checked;
  updateTranslateUi();
});

const segmentMap = new Map();
let liveFinalLog = "";
let liveFinalFlat = "";
let liveStable = "";
let livePartial = "";
let lastWindowText = "";
let pendingCandidate = "";
let pendingCount = 0;
let draftBuffer = "";
let lastDraftWindow = "";
const FINAL_ONLY = false;
const STABLE_HITS = 2;
const MAX_OVERLAP = 200;
const LOG_CAPTION = true;
let liveLogIndex = 0;
const liveTranslated = new Set();
const finalSegments = new Map();

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
  statusEl.textContent = count ? `Saved ${count}` : "No segments";
  if (emptyHint) {
    emptyHint.style.display = count ? "none" : "block";
  }
};

const formatSeconds = (ms) => {
  if (ms === null || ms === undefined) return "";
  const seconds = ms / 1000;
  return `${seconds.toFixed(1)}s`;
};

const formatSpeaker = (info) => {
  if (!info || info.speaker_id === null || info.speaker_id === undefined) {
    return { text: "Speaker ?", unknown: true };
  }
  return { text: `Speaker ${info.speaker_id}`, unknown: false };
};

const applyLiveSpeaker = (speakerId, mixed) => {
  if (!liveSpeakerEl) return;
  if (mixed || speakerId === null || speakerId === undefined) {
    liveSpeakerEl.textContent = "Speaker ?";
    liveSpeakerEl.dataset.state = "unknown";
    return;
  }
  liveSpeakerEl.textContent = `Speaker ${speakerId}`;
  delete liveSpeakerEl.dataset.state;
};

const normalizeText = (value) => {
  if (!value) return "";
  return value.replace(/\s+/g, " ").trim();
};

const ensureLiveFinalNodes = () => {
  if (!liveFinalEl) return {};
  let list = liveFinalEl.querySelector(".live-final-list");
  let stable = liveFinalEl.querySelector(".live-final-stable");
  if (!list) {
    list = document.createElement("div");
    list.className = "live-final-list";
    liveFinalEl.appendChild(list);
  }
  if (!stable) {
    stable = document.createElement("div");
    stable.className = "live-final-stable";
    liveFinalEl.appendChild(stable);
  }
  return { list, stable };
};

const captureLiveScroll = () => {
  if (!liveFinalEl) return null;
  const maxScroll = liveFinalEl.scrollHeight - liveFinalEl.clientHeight;
  const atBottom = maxScroll <= 0 || liveFinalEl.scrollTop >= maxScroll - 6;
  return {
    top: liveFinalEl.scrollTop,
    height: liveFinalEl.scrollHeight,
    atBottom,
  };
};

const restoreLiveScroll = (state) => {
  if (!state || !liveFinalEl) return;
  const newHeight = liveFinalEl.scrollHeight;
  if (state.atBottom) {
    liveFinalEl.scrollTop = newHeight;
  } else {
    const delta = newHeight - state.height;
    liveFinalEl.scrollTop = Math.max(0, state.top + delta);
  }
};


const parseSegmentOrder = (info) => {
  if (!info) return Date.now();
  const createdAt = info.created_at ? Date.parse(info.created_at) : NaN;
  if (Number.isFinite(createdAt)) {
    return createdAt;
  }
  const name = info.name || "";
  const match = name.match(/segment_(\d{8})_(\d{6})_(\d{3})/);
  if (match) {
    const date = match[1];
    const time = match[2];
    const ms = Number(match[3]);
    const year = Number(date.slice(0, 4));
    const month = Number(date.slice(4, 6)) - 1;
    const day = Number(date.slice(6, 8));
    const hour = Number(time.slice(0, 2));
    const minute = Number(time.slice(2, 4));
    const second = Number(time.slice(4, 6));
    const ts = new Date(year, month, day, hour, minute, second, ms).getTime();
    if (Number.isFinite(ts)) {
      return ts;
    }
  }
  return Date.now();
};

const rebuildFinalLog = () => {
  const scrollState = captureLiveScroll();
  const ordered = Array.from(finalSegments.values()).sort((a, b) => {
    if (a.order !== b.order) return a.order - b.order;
    return a.name.localeCompare(b.name);
  });
  liveFinalLog = ordered.map((item) => item.text).join("\n");
  liveFinalFlat = normalizeText(liveFinalLog);
  const { list } = ensureLiveFinalNodes();
  if (list) {
    list.innerHTML = "";
    const fragment = document.createDocumentFragment();
    ordered.forEach((item) => {
      const line = document.createElement("div");
      line.className = "live-final-line";
      if (item.inserted) {
        line.classList.add("inserted");
      }
      line.textContent = item.text;
      fragment.appendChild(line);
    });
    list.appendChild(fragment);
  }
  liveStable = "";
  livePartial = "";
  lastWindowText = "";
  pendingCandidate = "";
  pendingCount = 0;
  updateLiveUi();
  restoreLiveScroll(scrollState);
};

const commonPrefix = (left, right) => {
  const limit = Math.min(left.length, right.length);
  let index = 0;
  while (index < limit && left[index] === right[index]) {
    index += 1;
  }
  return left.slice(0, index);
};

const computeOverlapPrefix = (finalText, windowText) => {
  if (!finalText || !windowText) return 0;
  const tail = finalText.slice(Math.max(0, finalText.length - MAX_OVERLAP));
  const max = Math.min(tail.length, windowText.length);
  for (let len = max; len > 0; len -= 1) {
    if (windowText.startsWith(tail.slice(tail.length - len))) {
      return len;
    }
  }
  return 0;
};

const updateLiveUi = () => {
  if (!liveFinalEl || !livePartialEl) return;
  const finalText = liveFinalLog
    ? liveStable
      ? `${liveFinalLog}\n${liveStable}`
      : liveFinalLog
    : liveStable;
  const { stable } = ensureLiveFinalNodes();
  if (stable) {
    stable.textContent = liveStable ? liveStable : "";
    stable.style.display = liveStable ? "block" : "none";
  }

  if (FINAL_ONLY) {
    livePartialEl.textContent = "";
    livePartialEl.dataset.empty = "true";
    livePartialEl.style.display = "none";
    return;
  }

  if (livePartial) {
    livePartialEl.textContent = livePartial;
    livePartialEl.dataset.empty = "false";
  } else {
    livePartialEl.textContent = finalText ? "" : "Waiting for speech...";
    livePartialEl.dataset.empty = "true";
  }
};

const resetLiveUi = () => {
  liveStable = "";
  livePartial = "";
  lastWindowText = "";
  pendingCandidate = "";
  pendingCount = 0;
  updateLiveUi();
  applyLiveSpeaker(null, true);
  if (liveMetaEl) {
    liveMetaEl.textContent = "Idle";
  }
};

const appendFinalLog = (info) => {
  const cleaned = normalizeText(info?.transcript || "");
  if (!cleaned) return;
  if (LOG_CAPTION) {
    liveLogIndex += 1;
    invoke("log_live_line", {
      index: liveLogIndex,
      line: `[final] ${cleaned}`,
    }).catch((error) => console.warn("log_live_line error", error));
  }
  const name = info?.name || `${Date.now()}`;
  const order = parseSegmentOrder(info);
  let insertion = false;
  if (finalSegments.size > 0) {
    let maxOrder = Number.NEGATIVE_INFINITY;
    for (const value of finalSegments.values()) {
      if (value.order > maxOrder) {
        maxOrder = value.order;
      }
    }
    insertion = order < maxOrder;
  }
  const existing = finalSegments.get(name);
  finalSegments.set(name, {
    name,
    order,
    text: cleaned,
    inserted: existing?.inserted || insertion,
  });
  rebuildFinalLog();
};

const translateLiveFinal = async (info) => {
  if (!translateEnabled) return;
  const text = normalizeText(info?.transcript || "");
  if (!text) return;
  const id = info?.name || `${Date.now()}`;
  const createdAt = info?.created_at ? Date.parse(info.created_at) : NaN;
  const order = Number.isFinite(createdAt) ? createdAt : Date.now();
  if (liveTranslated.has(id)) return;
  liveTranslated.add(id);
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

const applyWindowTranscript = (payload) => {
  if (!payload) return;
  if (FINAL_ONLY) {
    return;
  }
  const cleaned = normalizeText(payload.text);
  if (LOG_CAPTION && cleaned) {
    liveLogIndex += 1;
    invoke("log_live_line", {
      index: liveLogIndex,
      line: `[window] ${cleaned}`,
    }).catch((error) => console.warn("log_live_line error", error));
  }
  const overlap = computeOverlapPrefix(liveFinalFlat, cleaned);
  const windowText = normalizeText(cleaned.slice(overlap));
  const lcp = commonPrefix(lastWindowText, windowText);
  let candidate = lcp;

  if (liveStable && !candidate.startsWith(liveStable)) {
    candidate = liveStable;
  }

  if (candidate.length > liveStable.length) {
    if (candidate === pendingCandidate) {
      pendingCount += 1;
    } else {
      pendingCandidate = candidate;
      pendingCount = 1;
    }
    if (pendingCount >= STABLE_HITS) {
      liveStable = candidate;
      pendingCandidate = "";
      pendingCount = 0;
    }
  } else {
    pendingCandidate = "";
    pendingCount = 0;
  }

  if (windowText.startsWith(liveStable)) {
    livePartial = windowText.slice(liveStable.length).trimStart();
  } else {
    livePartial = windowText;
  }

  lastWindowText = windowText;
  updateLiveUi();
  updateLiveDraft();

  if (liveMetaEl) {
    const latency = Number.isFinite(payload.elapsed_ms)
      ? `${(payload.elapsed_ms / 1000).toFixed(1)}s`
      : "";
    const windowSize = Number.isFinite(payload.window_ms)
      ? `${(payload.window_ms / 1000).toFixed(1)}s window`
      : "";
    const meta = [windowSize, latency].filter(Boolean).join(" | ");
    liveMetaEl.textContent = meta || "Listening...";
  }
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
  meta.textContent = `${formatDuration(info.duration_ms)} | ${formatTime(info.created_at)}`;
  title.appendChild(meta);

  const speaker = document.createElement("div");
  speaker.className = "segment-speaker";
  const speakerLabel = formatSpeaker(info);
  speaker.textContent = speakerLabel.text;
  if (speakerLabel.unknown) {
    speaker.dataset.state = "unknown";
  }
  title.appendChild(speaker);

  const transcript = document.createElement("div");
  transcript.className = "segment-transcript";
  if (info.transcript && info.transcript.trim()) {
    const stamp = formatSeconds(info.transcript_ms);
    transcript.textContent = stamp ? `${info.transcript} | ${stamp}` : info.transcript;
    transcript.dataset.state = "ready";
  } else {
    transcript.textContent = "Transcribing...";
    transcript.dataset.state = "pending";
  }
  title.appendChild(transcript);

  const translation = document.createElement("div");
  translation.className = "segment-translation";
  if (info.translation && info.translation.trim()) {
    const stamp = formatSeconds(info.translation_ms);
    translation.textContent = stamp ? `${info.translation} | ${stamp}` : info.translation;
    translation.dataset.state = "ready";
  } else {
    translation.textContent = translateEnabled ? "Translating..." : "Translation off";
    translation.dataset.state = "pending";
  }
  title.appendChild(translation);

  const translateButton = document.createElement("button");
  translateButton.className = "translate-button";
  translateButton.setAttribute("aria-label", "Translate");
  translateButton.disabled = !translateEnabled;
  translateButton.dataset.state = translateEnabled ? "ready" : "disabled";
  translateButton.innerHTML =
    '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 5h8v2H9.8c.7 1.4 1.7 2.6 2.9 3.7.8-.9 1.4-1.8 2-2.9l1.8.9c-.7 1.4-1.6 2.7-2.7 3.8l2.2 2.1-1.4 1.4-2.2-2.1c-1.4 1.2-3 2.2-4.8 3l-.8-1.8c1.6-.7 3-1.5 4.2-2.6-1.4-1.3-2.6-2.9-3.4-4.7H4V5zm11.5 5H18l3 9h-2l-.7-2h-4.6l-.7 2h-2l3.5-9zm-1.4 5h3.2l-1.6-4.5-1.6 4.5z"/></svg>';
  translateButton.addEventListener("click", () => translateSegment(info, row));

  const actions = document.createElement("div");
  actions.className = "segment-actions";
  actions.appendChild(translateButton);

  row.appendChild(title);
  row.appendChild(actions);

  return row;
};

const updateSpeaker = (info) => {
  if (!info || !info.name) return;
  const row = segmentMap.get(info.name);
  const speakerLabel = formatSpeaker(info);
  if (row) {
    const speaker = row.querySelector(".segment-speaker");
    if (speaker) {
      speaker.textContent = speakerLabel.text;
      if (speakerLabel.unknown) {
        speaker.dataset.state = "unknown";
      } else {
        delete speaker.dataset.state;
      }
    }
  }
};

const updateLiveDraft = () => {
  const raw = livePartialEl
    ? livePartialEl.dataset.empty === "true"
      ? ""
      : livePartialEl.textContent || ""
    : livePartial;
  const text = normalizeText(raw);
  if (!text) return;

  if (!lastDraftWindow) {
    draftBuffer = text;
  } else if (text === lastDraftWindow) {
    return;
  } else if (text.startsWith(lastDraftWindow)) {
    draftBuffer += text.slice(lastDraftWindow.length);
  } else if (lastDraftWindow.startsWith(text)) {
    // Rollback: keep accumulated buffer, just reset cursor.
  } else {
    draftBuffer += (draftBuffer ? "\n" : "") + text;
  }
  lastDraftWindow = text;
  invoke("emit_live_draft", { text: draftBuffer }).catch(() => {});
};

const translateSegment = async (info, row) => {
  if (!translateEnabled) {
    return;
  }
  const translation = row?.querySelector(".segment-translation");
  if (translation) {
    translation.textContent = "Translating...";
    translation.dataset.state = "pending";
  }
  try {
    const provider = await getTranslateProvider();
    await invoke("translate_segment", { name: info.name, provider });
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
  if (listEl) {
    if (prepend) {
      listEl.prepend(row);
    } else {
      listEl.appendChild(row);
    }
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
    transcript.textContent = stamp ? `${info.transcript} | ${stamp}` : info.transcript;
    transcript.dataset.state = "ready";
  } else {
    transcript.textContent = "Transcription failed";
    transcript.dataset.state = "error";
  }
};

const updateTranslation = (info) => {
  if (!info || !info.name) return;
  const row = segmentMap.get(info.name);
  if (!row) return;
  const translation = row.querySelector(".segment-translation");
  if (!translation) return;
  if (!translateEnabled && (!info.translation || !info.translation.trim())) {
    translation.textContent = "Translation off";
    translation.dataset.state = "pending";
    return;
  }
  if (info.translation === null || info.translation === undefined) {
    translation.textContent = "Translating...";
    translation.dataset.state = "pending";
  } else if (info.translation && info.translation.trim()) {
    const stamp = formatSeconds(info.translation_ms);
    translation.textContent = stamp ? `${info.translation} | ${stamp}` : info.translation;
    translation.dataset.state = "ready";
  } else {
    translation.textContent = "Translation failed";
    translation.dataset.state = "error";
  }
};


const clearSegmentsUi = () => {
  segmentMap.clear();
  if (listEl) {
    listEl.innerHTML = "";
    if (emptyHint) {
      listEl.appendChild(emptyHint);
    }
  }
  finalSegments.clear();
  if (liveFinalEl) {
    const { list, stable } = ensureLiveFinalNodes();
    if (list) {
      list.innerHTML = "";
    }
    if (stable) {
      stable.textContent = "";
      stable.style.display = "none";
    }
  }
  updateStatus();
  liveFinalLog = "";
  liveFinalFlat = "";
  resetLiveUi();
};

const loadSegments = async () => {
  try {
    const segments = await invoke("list_segments");
    const ordered = segments
      .slice()
      .sort((a, b) => new Date(a.created_at) - new Date(b.created_at))
    ordered.forEach((segment) => addSegment(segment));
    finalSegments.clear();
    ordered.forEach((segment) => {
      const cleaned = normalizeText(segment.transcript || "");
      if (!cleaned) return;
      finalSegments.set(segment.name, {
        name: segment.name,
        order: parseSegmentOrder(segment),
        text: cleaned,
        inserted: false,
      });
    });
    rebuildFinalLog();
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
    if (translateEnabled) {
      updateTranslation({ ...event.payload, translation: null });
    } else {
      updateTranslation(event.payload);
    }
    updateSpeaker(event.payload);
    appendFinalLog(event.payload);
    if (translateEnabled) {
      translateLiveFinal(event.payload);
    }
  }
});

listen("segment_translated", (event) => {
  if (event?.payload) {
    updateTranslation(event.payload);
  }
});

listen("segment_speakered", (event) => {
  if (event?.payload) {
    updateSpeaker(event.payload);
  }
});

listen("segment_list_cleared", () => {
  clearSegmentsUi();
  liveTranslated.clear();
});

listen("window_transcribed", (event) => {
  if (event?.payload) {
    applyWindowTranscript(event.payload);
    if ("speaker_id" in event.payload || "speaker_mixed" in event.payload) {
      applyLiveSpeaker(event.payload.speaker_id, event.payload.speaker_mixed);
    }
  }
});

loadSegments();
updateTranslateUi();

