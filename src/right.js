import { listen } from "@tauri-apps/api/event";

const listEl = document.getElementById("translationList");
const emptyEl = document.getElementById("translationEmpty");
const textEl = document.getElementById("translationText");

const pending = new Map();
const orderQueue = [];
let streamingOrder = null;
let streamingBuffer = "";
let fullText = "";

const updateEmpty = () => {
  if (!emptyEl) return;
  emptyEl.style.display = fullText || streamingBuffer ? "none" : "block";
};

const appendChunk = (chunk) => {
  if (!textEl || !chunk) return;
  streamingBuffer += chunk;
  textEl.textContent = fullText + streamingBuffer;
  textEl.dataset.state = "streaming";
  if (listEl) {
    listEl.scrollTop = listEl.scrollHeight;
  }
  updateEmpty();
};

const enqueueOrder = (order) => {
  if (orderQueue.includes(order)) return;
  orderQueue.push(order);
  orderQueue.sort((a, b) => a - b);
};

const flushReady = () => {
  let advanced = false;
  while (orderQueue.length) {
    const next = orderQueue[0];
    const entry = pending.get(next);
    if (!entry) break;
    const text = entry.text || "";
    if (text) {
      fullText += (fullText ? "\n" : "") + text;
    }
    pending.delete(next);
    orderQueue.shift();
    advanced = true;
    if (streamingOrder === next) {
      streamingOrder = null;
      streamingBuffer = "";
    }
  }
  if (textEl && advanced) {
    textEl.textContent = fullText + streamingBuffer;
  }
  if (listEl) {
    listEl.scrollTop = listEl.scrollHeight;
  }
  updateEmpty();
};

listen("live_translation_start", (event) => {
  const payload = event?.payload || {};
  const id = payload.id;
  const order = Number(payload.order);
  if (!id || !Number.isFinite(order)) return;
  enqueueOrder(order);
  if (streamingOrder === null || orderQueue[0] === order) {
    streamingOrder = orderQueue[0];
    streamingBuffer = "";
  }
  if (textEl) {
    textEl.dataset.state = "pending";
    if (!fullText && !streamingBuffer) {
      textEl.textContent = "";
    }
  }
  updateEmpty();
});

listen("live_translation_chunk", (event) => {
  const payload = event?.payload || {};
  const id = payload.id;
  const chunk = payload.chunk;
  const order = Number(payload.order);
  if (!id || !chunk || !Number.isFinite(order)) return;
  if (streamingOrder === null) {
    streamingOrder = orderQueue[0] ?? order;
  }
  if (order !== streamingOrder) {
    return;
  }
  appendChunk(chunk);
});

listen("live_translation_done", (event) => {
  const payload = event?.payload || {};
  const id = payload.id;
  const order = Number(payload.order);
  if (!id || !Number.isFinite(order)) return;
  const translation = payload.translation || "";
  pending.set(order, { text: translation || "Translation failed" });
  if (textEl) {
    textEl.dataset.state = translation ? "ready" : "error";
  }
  flushReady();
});

listen("live_translation_error", (event) => {
  const payload = event?.payload || {};
  const id = payload.id;
  const order = Number(payload.order);
  if (!id || !Number.isFinite(order)) return;
  pending.set(order, { text: payload.error || "Translation failed" });
  if (textEl) {
    textEl.dataset.state = "error";
  }
  flushReady();
});

listen("live_translation_cleared", () => {
  pending.clear();
  orderQueue.length = 0;
  streamingOrder = null;
  streamingBuffer = "";
  fullText = "";
  if (textEl) {
    textEl.textContent = "";
    textEl.dataset.state = "pending";
  }
  updateEmpty();
});
