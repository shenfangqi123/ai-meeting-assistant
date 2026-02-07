import { invoke } from "@tauri-apps/api/core";

const meetingUrlDefault = "https://zoom.us/signin";
const SELECTED_PROJECT_STORAGE_KEY = "rag_selected_project_id";

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

const projectSettingsBtn = document.getElementById("projectSettingsBtn");
const currentProjectLabel = document.getElementById("currentProjectLabel");
const projectModal = document.getElementById("projectModal");
const projectModalClose = document.getElementById("projectModalClose");
const projectNewBtn = document.getElementById("projectNewBtn");
const projectCreateStatus = document.getElementById("projectCreateStatus");
const projectDraft = document.getElementById("projectDraft");
const projectNameInput = document.getElementById("projectNameInput");
const projectChooseDirBtn = document.getElementById("projectChooseDirBtn");
const projectRootPath = document.getElementById("projectRootPath");
const projectCreateCancelBtn = document.getElementById("projectCreateCancelBtn");
const projectList = document.getElementById("projectList");

const projectProgressModal = document.getElementById("projectProgressModal");
const projectProgressPercent = document.getElementById("projectProgressPercent");
const projectProgressFill = document.getElementById("projectProgressFill");
const projectProgressText = document.getElementById("projectProgressText");
const projectProgressPath = document.getElementById("projectProgressPath");
const projectProgressMetrics = document.getElementById("projectProgressMetrics");
const projectProgressLogs = document.getElementById("projectProgressLogs");
const projectProgressSkippedWrap = document.getElementById("projectProgressSkippedWrap");
const projectProgressSkippedList = document.getElementById("projectProgressSkippedList");
const projectProgressDoneBtn = document.getElementById("projectProgressDoneBtn");

let resizeState = null;
let pendingResize = null;
let resizeFrame = null;
let isCapturing = false;
let currentAsrProvider = "whisperserver";
let selectedProjectIds = [];
let selectedProjectName = "";
let projects = [];
const projectActionMap = new Map();
let isProjectModalOpen = false;

let isCreateDraftOpen = false;
let createDraftName = "";
let createDraftRootDir = "";
let createDraftBusy = false;

let progressRunning = false;
let progressInterval = null;
let progressValue = 0;

const normalizeUrl = (raw) => {
  if (!raw) return "";
  if (raw.startsWith("http://") || raw.startsWith("https://")) return raw;
  return `https://${raw}`;
};

const normalizeRootPath = (value) => {
  if (!value) return "";
  const path = value.replace(/\\/g, "/").replace(/\/+$/g, "");
  return path.toLowerCase();
};

const basenameFromPath = (value) => {
  if (!value) return "untitled";
  const normalized = value.replace(/\\/g, "/").replace(/\/+$/g, "");
  const segments = normalized.split("/");
  return segments[segments.length - 1] || "untitled";
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

const updateCaptureUi = (active) => {
  isCapturing = active;
  if (asrStart) {
    asrStart.textContent = active ? "Stop Capture" : "Start Capture";
  }
  if (captureStatus) {
    captureStatus.textContent = active ? "Capturing..." : "Idle";
  }
};

const updateCurrentProjectLabel = () => {
  if (!currentProjectLabel) return;
  if (!selectedProjectIds.length || !selectedProjectName) {
    currentProjectLabel.textContent = "当前项目：未选择";
    return;
  }
  currentProjectLabel.textContent = `当前项目：${selectedProjectName}`;
};

const setCreateStatus = (text, isError = false) => {
  if (!projectCreateStatus) return;
  projectCreateStatus.textContent = text || "";
  projectCreateStatus.style.color = isError ? "#b64422" : "";
};

const setCurrentProject = (project) => {
  if (!project) {
    selectedProjectIds = [];
    selectedProjectName = "";
    localStorage.removeItem(SELECTED_PROJECT_STORAGE_KEY);
    updateCurrentProjectLabel();
    renderProjectList();
    return;
  }
  selectedProjectIds = [project.project_id];
  selectedProjectName = project.project_name;
  localStorage.setItem(SELECTED_PROJECT_STORAGE_KEY, project.project_id);
  updateCurrentProjectLabel();
  renderProjectList();
};

const syncSelectedProject = () => {
  const selectedId = selectedProjectIds[0];
  if (!selectedId) {
    selectedProjectName = "";
    updateCurrentProjectLabel();
    return;
  }
  const selected = projects.find((project) => project.project_id === selectedId);
  if (!selected) {
    setCurrentProject(null);
    return;
  }
  selectedProjectName = selected.project_name;
  updateCurrentProjectLabel();
};

const setProjectAction = (projectId, action) => {
  if (!projectId) return;
  if (!action) {
    projectActionMap.delete(projectId);
  } else {
    projectActionMap.set(projectId, action);
  }
  renderProjectList();
};

const formatIndexReport = (report) => {
  if (!report) return "";
  return [
    `indexed=${report.indexed_files ?? 0}`,
    `updated=${report.updated_files ?? 0}`,
    `deleted=${report.deleted_files ?? 0}`,
    `chunks_added=${report.chunks_added ?? 0}`,
    `chunks_deleted=${report.chunks_deleted ?? 0}`,
    `skipped=${Array.isArray(report.skipped_files) ? report.skipped_files.length : 0}`,
  ].join(" | ");
};

const renderProjectDraft = () => {
  if (!projectDraft || !projectNameInput || !projectRootPath || !projectChooseDirBtn || !projectNewBtn) return;

  if (!isCreateDraftOpen) {
    projectDraft.classList.add("hidden");
    projectNameInput.value = "";
    projectRootPath.textContent = "未选择";
    projectChooseDirBtn.disabled = true;
    projectNewBtn.disabled = false;
    return;
  }

  projectDraft.classList.remove("hidden");
  projectNameInput.value = createDraftName;
  projectNameInput.disabled = createDraftBusy;
  projectRootPath.textContent = createDraftRootDir || "未选择";
  projectNewBtn.disabled = createDraftBusy;

  const hasValidName = createDraftName.trim().length > 0;
  projectChooseDirBtn.disabled = createDraftBusy || !hasValidName;
  if (projectCreateCancelBtn) {
    projectCreateCancelBtn.disabled = createDraftBusy;
  }
};

const openCreateDraft = () => {
  isCreateDraftOpen = true;
  createDraftName = "";
  createDraftRootDir = "";
  createDraftBusy = false;
  setCreateStatus("");
  renderProjectDraft();
  projectNameInput?.focus();
};

const closeCreateDraft = () => {
  if (createDraftBusy) return;
  isCreateDraftOpen = false;
  createDraftName = "";
  createDraftRootDir = "";
  renderProjectDraft();
  setCreateStatus("");
};

const setProgress = (value, text) => {
  progressValue = Math.max(0, Math.min(100, value));
  if (projectProgressFill) {
    projectProgressFill.style.width = `${progressValue}%`;
  }
  if (projectProgressPercent) {
    projectProgressPercent.textContent = `${Math.round(progressValue)}%`;
  }
  if (text && projectProgressText) {
    projectProgressText.textContent = text;
  }
};

const addProgressLog = (line) => {
  if (!projectProgressLogs) return;
  const now = new Date();
  const stamp = `${String(now.getHours()).padStart(2, "0")}:${String(now.getMinutes()).padStart(2, "0")}:${String(now.getSeconds()).padStart(2, "0")}`;
  const text = `[${stamp}] ${line}`;
  projectProgressLogs.textContent = projectProgressLogs.textContent
    ? `${projectProgressLogs.textContent}\n${text}`
    : text;
  projectProgressLogs.scrollTop = projectProgressLogs.scrollHeight;
};

const openProgressModal = ({ projectName, rootDir }) => {
  progressRunning = true;
  progressValue = 0;
  if (!projectProgressModal) return;

  projectProgressModal.classList.remove("hidden");
  projectProgressModal.setAttribute("aria-hidden", "false");

  if (projectProgressDoneBtn) {
    projectProgressDoneBtn.classList.add("hidden");
  }
  if (projectProgressMetrics) {
    projectProgressMetrics.textContent = "";
  }
  if (projectProgressPath) {
    projectProgressPath.textContent = `项目：${projectName} | 目录：${rootDir}`;
  }
  if (projectProgressSkippedWrap) {
    projectProgressSkippedWrap.classList.add("hidden");
  }
  if (projectProgressSkippedList) {
    projectProgressSkippedList.innerHTML = "";
  }
  if (projectProgressLogs) {
    projectProgressLogs.textContent = "";
  }

  setProgress(0, "准备开始...");
  addProgressLog("开始创建项目");
};

const startFakeProgress = () => {
  if (progressInterval) {
    clearInterval(progressInterval);
  }
  progressInterval = setInterval(() => {
    if (!progressRunning) return;
    if (progressValue >= 90) return;
    setProgress(progressValue + 2);
  }, 350);
};

const stopFakeProgress = () => {
  if (!progressInterval) return;
  clearInterval(progressInterval);
  progressInterval = null;
};

const showProgressResult = (report) => {
  progressRunning = false;
  stopFakeProgress();
  setProgress(100, "索引完成");

  const skipped = Array.isArray(report?.skipped_files) ? report.skipped_files : [];
  if (projectProgressMetrics) {
    projectProgressMetrics.textContent = formatIndexReport(report);
  }

  if (projectProgressSkippedWrap && projectProgressSkippedList) {
    if (skipped.length) {
      projectProgressSkippedWrap.classList.remove("hidden");
      projectProgressSkippedList.innerHTML = skipped
        .map((item) => `<div>${item.path} (${item.reason})</div>`)
        .join("");
    } else {
      projectProgressSkippedWrap.classList.add("hidden");
      projectProgressSkippedList.innerHTML = "";
    }
  }

  addProgressLog("索引完成");
  if (projectProgressDoneBtn) {
    projectProgressDoneBtn.classList.remove("hidden");
  }
};

const showProgressError = (error) => {
  progressRunning = false;
  stopFakeProgress();
  if (projectProgressText) {
    projectProgressText.textContent = "索引失败";
  }
  if (projectProgressMetrics) {
    projectProgressMetrics.textContent = String(error);
    projectProgressMetrics.style.color = "#b64422";
  }
  addProgressLog(`失败：${error}`);
  if (projectProgressDoneBtn) {
    projectProgressDoneBtn.classList.remove("hidden");
  }
};

const closeProgressModal = () => {
  if (progressRunning || !projectProgressModal) return;
  projectProgressModal.classList.add("hidden");
  projectProgressModal.setAttribute("aria-hidden", "true");
  if (projectProgressMetrics) {
    projectProgressMetrics.style.color = "";
  }
};

const renderProjectList = () => {
  if (!projectList) return;
  projectList.innerHTML = "";

  if (!projects.length) {
    const empty = document.createElement("div");
    empty.className = "project-empty";
    empty.textContent = "暂无项目，点击“新建项目”开始。";
    projectList.appendChild(empty);
    return;
  }

  for (const project of projects) {
    const row = document.createElement("article");
    row.className = "project-row";

    const head = document.createElement("div");
    head.className = "project-row-head";

    const name = document.createElement("span");
    name.className = "project-row-name";
    name.textContent = project.project_name || "未命名项目";
    head.appendChild(name);

    if (selectedProjectIds.includes(project.project_id)) {
      const tag = document.createElement("span");
      tag.className = "project-row-tag";
      tag.textContent = "当前项目";
      head.appendChild(tag);
    }

    row.appendChild(head);

    const path = document.createElement("div");
    path.className = "project-row-path";
    path.textContent = project.root_dir;
    path.title = project.root_dir;
    row.appendChild(path);

    const actions = document.createElement("div");
    actions.className = "project-row-actions";

    const busyText = projectActionMap.get(project.project_id);
    const isBusy = !!busyText;

    const showBtn = document.createElement("button");
    showBtn.type = "button";
    showBtn.textContent = "显示";
    showBtn.disabled = isBusy;
    showBtn.addEventListener("click", () => {
      setCurrentProject(project);
      closeProjectModal();
    });

    const syncBtn = document.createElement("button");
    syncBtn.type = "button";
    syncBtn.textContent = "更新";
    syncBtn.disabled = isBusy;
    syncBtn.addEventListener("click", () => {
      void updateProject(project);
    });

    const deleteBtn = document.createElement("button");
    deleteBtn.type = "button";
    deleteBtn.textContent = "删除";
    deleteBtn.disabled = isBusy;
    deleteBtn.addEventListener("click", () => {
      void deleteProject(project);
    });

    actions.appendChild(showBtn);
    actions.appendChild(syncBtn);
    actions.appendChild(deleteBtn);

    if (busyText) {
      const loading = document.createElement("span");
      loading.className = "project-row-loading";
      loading.textContent = busyText;
      actions.appendChild(loading);
    }

    row.appendChild(actions);
    projectList.appendChild(row);
  }
};

const loadProjects = async () => {
  try {
    const response = await invoke("rag_project_list");
    projects = Array.isArray(response?.projects) ? response.projects : [];
    syncSelectedProject();
    renderProjectList();
  } catch (error) {
    logError(`project list error: ${error}`);
    setCreateStatus(`项目列表读取失败：${error}`, true);
  }
};

const openProjectModal = async () => {
  if (!projectModal) return;
  isProjectModalOpen = true;
  projectModal.classList.remove("hidden");
  projectModal.setAttribute("aria-hidden", "false");
  await loadProjects();
  renderProjectDraft();
};

const closeProjectModal = () => {
  if (!projectModal || progressRunning) return;
  isProjectModalOpen = false;
  projectModal.classList.add("hidden");
  projectModal.setAttribute("aria-hidden", "true");
};

const createProjectAndSyncFromSelection = async (projectName, rootDir) => {
  createDraftBusy = true;
  renderProjectDraft();
  setCreateStatus("目录已选择，开始创建并索引...");

  openProgressModal({ projectName, rootDir });
  startFakeProgress();
  setProgress(10, "创建项目记录...");
  addProgressLog("正在创建项目配置");

  try {
    const created = await invoke("rag_project_create", {
      request: {
        project_name: projectName,
        root_dir: rootDir,
      },
    });

    setProgress(25, "开始扫描并索引...");
    addProgressLog("开始执行增量同步");

    const report = await invoke("rag_index_sync_project", {
      request: {
        project_id: created.project_id,
        root_dir: created.root_dir,
      },
    });

    await loadProjects();
    setCurrentProject(created);
    isCreateDraftOpen = false;
    createDraftName = "";
    createDraftRootDir = "";
    createDraftBusy = false;
    renderProjectDraft();
    setCreateStatus("创建成功，索引已完成。");
    showProgressResult(report);
  } catch (error) {
    createDraftBusy = false;
    renderProjectDraft();
    if (String(error).includes("project root already exists")) {
      setCreateStatus("该目录已存在项目", true);
    } else {
      setCreateStatus(`创建失败：${error}`, true);
    }
    showProgressError(error);
  }
};

const chooseDirectoryAndStart = async () => {
  const name = (createDraftName || "").trim();
  if (!name) {
    setCreateStatus("请先输入项目名称", true);
    projectNameInput?.focus();
    return;
  }

  try {
    const rootDir = await invoke("rag_pick_folder");
    if (!rootDir) return;

    createDraftRootDir = rootDir;
    renderProjectDraft();

    const nextRoot = normalizeRootPath(rootDir);
    const exists = projects.find(
      (project) => normalizeRootPath(project.root_dir) === nextRoot
    );
    if (exists) {
      setCreateStatus("该目录已存在项目", true);
      return;
    }

    setCreateStatus("目录已选择，准备索引...");
    await createProjectAndSyncFromSelection(name, rootDir);
  } catch (error) {
    setCreateStatus(`目录选择失败：${error}`, true);
  }
};

const updateProject = async (project) => {
  if (!project || projectActionMap.has(project.project_id)) return;
  setProjectAction(project.project_id, "更新中");
  try {
    const report = await invoke("rag_index_sync_project", {
      request: {
        project_id: project.project_id,
        root_dir: project.root_dir,
      },
    });
    window.alert(`项目更新完成\n${formatIndexReport(report)}`);
  } catch (error) {
    window.alert(`项目更新失败：${error}`);
  } finally {
    setProjectAction(project.project_id, "");
  }
};

const deleteProject = async (project) => {
  if (!project || projectActionMap.has(project.project_id)) return;
  const confirmed = window.confirm(
    "将移除该项目在向量库中的索引数据（chunks/manifest），不会删除磁盘上的文件。是否继续？"
  );
  if (!confirmed) return;

  setProjectAction(project.project_id, "删除中");
  try {
    const report = await invoke("rag_project_delete", {
      request: { project_id: project.project_id },
    });
    await loadProjects();
    if (selectedProjectIds.includes(project.project_id)) {
      setCurrentProject(null);
    }
    window.alert(
      `项目已删除\ndeleted_files=${report.deleted_files ?? 0}, deleted_chunks=${report.deleted_chunks ?? 0}`
    );
  } catch (error) {
    window.alert(`删除失败：${error}`);
  } finally {
    setProjectAction(project.project_id, "");
  }
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

loadBtn?.addEventListener("click", async () => {
  const url = normalizeUrl(urlInput?.value.trim());
  if (!url) return;
  try {
    await invoke("content_navigate", { url });
  } catch (error) {
    logError(`load error: ${error}`);
  }
});

urlInput?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    loadBtn?.click();
  }
});

splitter?.addEventListener("pointerdown", (event) => {
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
  if (!resizeState || !splitter) return;
  resizeState = null;
  splitter.classList.remove("dragging");
};

window.addEventListener("pointerup", endResize);
window.addEventListener("pointercancel", endResize);

asrStart?.addEventListener("click", async () => {
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
  const next = currentAsrProvider === "whisperserver" ? "openai" : "whisperserver";
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

projectSettingsBtn?.addEventListener("click", () => {
  void openProjectModal();
});

projectModalClose?.addEventListener("click", closeProjectModal);

projectModal?.addEventListener("click", (event) => {
  if (event.target === projectModal && !progressRunning) {
    closeProjectModal();
  }
});

window.addEventListener("keydown", (event) => {
  const progressVisible = projectProgressModal && !projectProgressModal.classList.contains("hidden");
  if (event.key === "Escape" && progressVisible) {
    event.preventDefault();
    return;
  }
  if (event.key === "Escape" && isProjectModalOpen) {
    closeProjectModal();
  }
});

projectNewBtn?.addEventListener("click", () => {
  if (createDraftBusy) return;
  openCreateDraft();
});

projectNameInput?.addEventListener("input", () => {
  createDraftName = projectNameInput.value;
  if (createDraftName.trim()) {
    setCreateStatus("");
  }
  renderProjectDraft();
});

projectChooseDirBtn?.addEventListener("click", () => {
  void chooseDirectoryAndStart();
});

projectCreateCancelBtn?.addEventListener("click", () => {
  closeCreateDraft();
});

projectProgressDoneBtn?.addEventListener("click", () => {
  closeProgressModal();
});

const savedProjectId = localStorage.getItem(SELECTED_PROJECT_STORAGE_KEY);
if (savedProjectId) {
  selectedProjectIds = [savedProjectId];
}

updateCurrentProjectLabel();
loadAsrSettings();
void loadProjects();
renderProjectDraft();

if (urlInput) {
  urlInput.value = meetingUrlDefault;
}
