# AI Shepherd Tauri Demo

## 运行

```bash
pnpm install
pnpm tauri dev
```

## 说明

- 上下双 Webview：上方是控制台和 LLM/ASR 面板，下方左右两块分别为会议 Webview 与输出 Webview。
- 交互：拖动底部横向分隔条调整上下高度；拖动顶部横向条调整下方左右宽度。
- LLM：支持 OpenAI Chat Completions 兼容接口与 Ollama `/api/generate`。
- WhisperFlow：默认连接 `ws://localhost:8181/ws`，前端会发送 16-bit PCM 数据流。

如需自定义接口地址或模型，直接在顶部面板输入即可。
