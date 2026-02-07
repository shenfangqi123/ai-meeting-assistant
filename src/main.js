import { invoke } from "@tauri-apps/api/core";

const meetingUrlDefault = "https://zoom.us/signin";
const urlInput = document.getElementById("urlInput");
const loadBtn = document.getElementById("loadBtn");
const introBtn = document.getElementById("introBtn");
const asrProviderToggle = document.getElementById("asrProviderToggle");
const asrFallbackToggle = document.getElementById("asrFallbackToggle");
const asrLanguageSelect = document.getElementById("asrLanguage");

const asrStart = document.getElementById("asrStart");
const captureStatus = document.getElementById("captureStatus");
const clearSegmentsBtn = document.getElementById("clearSegments");
const splitter = document.getElementById("splitter");

let resizeState = null;
let pendingResize = null;
let resizeFrame = null;
let isCapturing = false;
let currentAsrProvider = "whisperserver";

const normalizeUrl = (raw) => {
  if (!raw) return "";
  if (raw.startsWith("http://") || raw.startsWith("https://")) return raw;
  return `https://${raw}`;
};

const logError = (message) => {
  if (message) {
    console.warn(message);
  }
};

const updateAsrUi = () => {
  if (!asrProviderToggle) return;
  asrProviderToggle.dataset.provider = currentAsrProvider;
  asrProviderToggle.textContent =
    currentAsrProvider === "openai" ? "OpenAI" : "Whisper Server";
};

const loadAsrSettings = async () => {
  if (!asrProviderToggle) return;
  try {
    const [provider, fallback, language] = await invoke("get_asr_settings");
    if (provider) {
      currentAsrProvider = provider;
    }
    if (asrFallbackToggle) {
      asrFallbackToggle.checked = !!fallback;
    }
    if (asrLanguageSelect && language) {
      asrLanguageSelect.value = language;
    }
  } catch (error) {
    logError(`asr load error: ${error}`);
  } finally {
    updateAsrUi();
  }
};

const updateCaptureUi = (active) => {
  isCapturing = active;
  asrStart.textContent = active ? "Stop Capture" : "Start Capture";
  if (captureStatus) {
    captureStatus.textContent = active ? "Capturing..." : "Idle";
  }
};

const scheduleResize = (height) => {
  pendingResize = height;
  if (resizeFrame) return;
  resizeFrame = requestAnimationFrame(async () => {
    try {
      await invoke("set_top_height", { height: pendingResize });
    } catch (error) {
      logError(`resize error: ${error}`);
    }
    resizeFrame = null;
  });
};

const startCapture = async () => {
  if (isCapturing) return;
  await invoke("start_loopback_capture");
  updateCaptureUi(true);
};

const stopCapture = async () => {
  if (!isCapturing) return;
  await invoke("stop_loopback_capture");
  updateCaptureUi(false);
};

loadBtn.addEventListener("click", async () => {
  const url = normalizeUrl(urlInput.value.trim());
  if (!url) return;
  try {
    await invoke("content_navigate", { url });
  } catch (error) {
    logError(`load error: ${error}`);
  }
});

urlInput.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    loadBtn.click();
  }
});

splitter.addEventListener("pointerdown", (event) => {
  resizeState = {
    startY: event.clientY,
    startHeight: window.innerHeight,
  };
  splitter.setPointerCapture(event.pointerId);
  splitter.classList.add("dragging");
});

window.addEventListener("pointermove", (event) => {
  if (!resizeState) return;
  const nextHeight = resizeState.startHeight + (event.clientY - resizeState.startY);
  scheduleResize(nextHeight);
});

const endResize = () => {
  if (!resizeState) return;
  resizeState = null;
  splitter.classList.remove("dragging");
};

window.addEventListener("pointerup", endResize);
window.addEventListener("pointercancel", endResize);

asrStart.addEventListener("click", async () => {
  try {
    if (isCapturing) {
      await stopCapture();
    } else {
      await startCapture();
    }
  } catch (error) {
    logError(`capture error: ${error}`);
  }
});

clearSegmentsBtn?.addEventListener("click", async () => {
  try {
    await invoke("clear_segments");
    updateCaptureUi(false);
  } catch (error) {
    logError(`clear error: ${error}`);
  }
});

introBtn?.addEventListener("click", async () => {
  try {
    await invoke("open_intro_window");
  } catch (error) {
    logError(`intro window error: ${error}`);
  }
});

asrProviderToggle?.addEventListener("click", async () => {
  const next =
    currentAsrProvider === "whisperserver" ? "openai" : "whisperserver";
  try {
    const updated = await invoke("set_asr_provider", { provider: next });
    currentAsrProvider = updated || next;
    updateAsrUi();
  } catch (error) {
    logError(`asr provider error: ${error}`);
  }
});

asrFallbackToggle?.addEventListener("change", async () => {
  try {
    await invoke("set_asr_fallback", { fallback: asrFallbackToggle.checked });
  } catch (error) {
    logError(`asr fallback error: ${error}`);
  }
});

asrLanguageSelect?.addEventListener("change", async () => {
  try {
    const updated = await invoke("set_asr_language", { language: asrLanguageSelect.value });
    if (updated) {
      asrLanguageSelect.value = updated;
    }
  } catch (error) {
    logError(`asr language error: ${error}`);
  }
});

loadAsrSettings();

urlInput.value = meetingUrlDefault;
