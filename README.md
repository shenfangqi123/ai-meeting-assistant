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

## Whisper `-t` 自动配置

`whisper-server` 启动时会自动根据推理模式和物理核心数设置 `-t`（`--threads`）。

GPU 推理模式（推荐）：

| 物理核心数 | 推荐 `-t` |
| --- | --- |
| 2 核 | 2 |
| 4 核 | 2~3 |
| 6 核 | 3~4 |
| 8 核 | 4 |
| 10 核 | 5 |
| 12 核 | 6 |
| 14 核 | 7 |
| 16 核 | 8 |
| 20 核 | 10 |
| 24 核 | 12（上限） |

CPU 推理模式（没有 GPU）：

| 物理核心数 | 推荐 `-t` |
| --- | --- |
| 2 核 | 2 |
| 4 核 | 3 |
| 6 核 | 5 |
| 8 核 | 7 |
| 12 核 | 10 |
| 16 核 | 14 |
| 24 核 | 20 |

说明：
- GPU 模式中存在范围值（例如 `4 核 -> 2~3`）时，当前实现取区间上限。
- GPU 模式 `-t` 自动限制在 12 以内；CPU 模式在高核心数时自动提升到 20。
