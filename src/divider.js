import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";

const webview = getCurrentWebview();
const appWindow = getCurrentWindow();

let dragging = false;
let context = { width: 1, offsetX: 0 };

const refreshContext = async () => {
  const size = await appWindow.innerSize();
  const position = await webview.position();
  context = {
    width: Math.max(1, size.width),
    offsetX: position.x,
  };
};

const sendRatio = async (clientX) => {
  const ratio = (context.offsetX + clientX) / context.width;
  const clamped = Math.max(0, Math.min(1, ratio));
  await invoke("set_bottom_split", { ratio: clamped });
};

window.addEventListener("pointerdown", async (event) => {
  dragging = true;
  await refreshContext();
  await sendRatio(event.clientX);
  if (event.target.setPointerCapture) {
    event.target.setPointerCapture(event.pointerId);
  }
});

window.addEventListener("pointermove", async (event) => {
  if (!dragging) return;
  await sendRatio(event.clientX);
});

const stopDragging = () => {
  dragging = false;
};

window.addEventListener("pointerup", stopDragging);
window.addEventListener("pointercancel", stopDragging);
