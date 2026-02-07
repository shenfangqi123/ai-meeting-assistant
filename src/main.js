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
const projectPickDirBtn = document.getElementById("projectPickDirBtn");
const projectCreateStatus = document.getElementById("projectCreateStatus");
const projectDraft = document.getElementById("projectDraft");
const projectNameInput = document.getElementById("projectNameInput");
const projectRootPath = document.getElementById("projectRootPath");
const projectCreateConfirmBtn = document.getElementById("projectCreateConfirmBtn");
const projectCreateCancelBtn = document.getElementById("projectCreateCancelBtn");
const projectList = document.getElementById("projectList");

let resizeState = null;
let pendingResize = null;
let resizeFrame = null;
let isCapturing = false;
let currentAsrProvider = "whisperserver";
let selectedProjectIds = [];
let selectedProjectName = "";
let projects = [];
let pendingProjectDraft = null;
const projectActionMap = new Map();
let isProjectModalOpen = false;

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
  if (!value) return "未命名项目";
  const normalized = value.replace(/\\/g, "/").replace(/\/+$/g, "");
  const segments = normalized.split("/");
  return segments[segments.length - 1] || "未命名项目";
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
  asrStart.textContent = active ? "Stop Capture" : "Start Capture";
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
    `indexed_files=${report.indexed_files ?? 0}`,
    `updated_files=${report.updated_files ?? 0}`,
    `deleted_files=${report.deleted_files ?? 0}`,
    `chunks_added=${report.chunks_added ?? 0}`,
    `chunks_deleted=${report.chunks_deleted ?? 0}`,
  ].join(", ");
};

const renderProjectDraft = () => {
  if (!projectDraft || !projectNameInput || !projectRootPath) return;
  if (!pendingProjectDraft) {
    projectDraft.classList.add("hidden");
    projectNameInput.value = "";
    projectRootPath.textContent = "";
    return;
  }

  projectDraft.classList.remove("hidden");
  projectNameInput.value = pendingProjectDraft.projectName;
  projectNameInput.disabled = !!pendingProjectDraft.submitting;
  projectRootPath.textContent = pendingProjectDraft.rootDir;
  if (projectCreateConfirmBtn) {
    projectCreateConfirmBtn.disabled = !!pendingProjectDraft.submitting;
  }
  if (projectCreateCancelBtn) {
    projectCreateCancelBtn.disabled = !!pendingProjectDraft.submitting;
  }
  if (projectPickDirBtn) {
    projectPickDirBtn.disabled = !!pendingProjectDraft.submitting;
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
  setCreateStatus("");
  await loadProjects();
  renderProjectDraft();
};

const closeProjectModal = () => {
  if (!projectModal) return;
  isProjectModalOpen = false;
  projectModal.classList.add("hidden");
  projectModal.setAttribute("aria-hidden", "true");
};

const pickProjectDirectory = async () => {
  try {
    const rootDir = await invoke("rag_pick_folder");
    if (!rootDir) return;

    const nextRoot = normalizeRootPath(rootDir);
    const exists = projects.find(
      (project) => normalizeRootPath(project.root_dir) === nextRoot
    );
    if (exists) {
      setCreateStatus("该目录已存在项目", true);
      return;
    }

    pendingProjectDraft = {
      rootDir,
      projectName: basenameFromPath(rootDir),
      submitting: false,
    };
    setCreateStatus("");
    renderProjectDraft();
  } catch (error) {
    setCreateStatus(`目录选择失败：${error}`, true);
  }
};

const createProjectAndSync = async () => {
  if (!pendingProjectDraft || pendingProjectDraft.submitting) return;

  const projectName = (projectNameInput?.value || "").trim() || basenameFromPath(pendingProjectDraft.rootDir);
  pendingProjectDraft.projectName = projectName;
  pendingProjectDraft.submitting = true;
  renderProjectDraft();
  setCreateStatus("正在创建项目并执行首次索引...");

  try {
    const created = await invoke("rag_project_create", {
      request: {
        project_name: projectName,
        root_dir: pendingProjectDraft.rootDir,
      },
    });

    const report = await invoke("rag_index_sync_project", {
      request: {
        project_id: created.project_id,
        root_dir: created.root_dir,
      },
    });

    pendingProjectDraft = null;
    renderProjectDraft();
    await loadProjects();
    setCurrentProject(created);
    setCreateStatus(`创建并索引完成：${formatIndexReport(report)}`);
  } catch (error) {
    if (String(error).includes("project root already exists")) {
      setCreateStatus("该目录已存在项目", true);
    } else {
      setCreateStatus(`创建失败：${error}`, true);
    }
    pendingProjectDraft.submitting = false;
    renderProjectDraft();
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
  if (event.target === projectModal) {
    closeProjectModal();
  }
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && isProjectModalOpen) {
    closeProjectModal();
  }
});

projectPickDirBtn?.addEventListener("click", () => {
  void pickProjectDirectory();
});

projectCreateConfirmBtn?.addEventListener("click", () => {
  void createProjectAndSync();
});

projectCreateCancelBtn?.addEventListener("click", () => {
  if (pendingProjectDraft?.submitting) return;
  pendingProjectDraft = null;
  renderProjectDraft();
  setCreateStatus("");
});

projectNameInput?.addEventListener("input", () => {
  if (!pendingProjectDraft || pendingProjectDraft.submitting) return;
  pendingProjectDraft.projectName = projectNameInput.value;
});

const savedProjectId = localStorage.getItem(SELECTED_PROJECT_STORAGE_KEY);
if (savedProjectId) {
  selectedProjectIds = [savedProjectId];
}

updateCurrentProjectLabel();
loadAsrSettings();
void loadProjects();

if (urlInput) {
  urlInput.value = meetingUrlDefault;
}
