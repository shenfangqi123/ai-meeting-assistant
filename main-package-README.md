# 主包打包流程（Tauri -> MSI -> MSIX）

本文整理主包（CPU + GPU 120a 产物）打包流程。包含 Tauri 配置调整、MSI 生成与 MSIX Packaging Tool 转包步骤。

## 一、前置准备

### 1) 资源目录准备
确保以下资源在 `src-tauri/resources` 下：

```
src-tauri/resources/
  models/                         # 现有资源（如 pyannote_embedding.onnx）
  whisper/
    cpu/whisper-server.exe         # CPU 版 whisper-server
    gpu/120a/whisper-server.exe    # GPU 版 whisper-server
    gpu/120a/ggml-cuda.dll         # GPU 依赖
```

如果还没有，把以下目录内容复制进去：

```
install/cpu/*        -> src-tauri/resources/whisper/cpu/
install/gpu-120a/*   -> src-tauri/resources/whisper/gpu/120a/
```

### 2) Tauri 配置（tauri.conf.json）
确保 `bundle` 配置已开启并包含图标与资源：

```json
"bundle": {
  "active": true,
  "icon": [
    "icons/icon.ico",
    "icons/icon.png"
  ],
  "resources": [
    "resources/models",
    "resources/whisper/cpu/whisper-server.exe",
    "resources/whisper/gpu/120a/whisper-server.exe",
    "resources/whisper/gpu/120a/ggml-cuda.dll"
  ]
}
```

> 注意：如果未指定 icon，会报 `Couldn't find a .ico icon`。

## 二、生成 MSI（Tauri 打包）

在项目根目录执行：

```powershell
pnpm tauri build -- --bundles msi
```

生成的 MSI 位置：

```
src-tauri/target/release/bundle/msi/AI Shepherd_0.1.0_x64_en-US.msi
```

## 三、用 MSIX Packaging Tool 转成 MSIX（主包）

### 1) 创建包
1. 打开 **MSIX Packaging Tool**
2. 选择 **Create package**
3. 选择 **在此计算机上创建程序包**
4. 选择 **Create package from an installer**
5. 选择 MSI 文件：
   - `C:\Codes\ai-shepherd-interview\src-tauri\target\release\bundle\msi\AI Shepherd_0.1.0_x64_en-US.msi`
6. 安装参数（可选）：
   - 如果提示需要重启，填写：
     `REBOOT=ReallySuppress REBOOTPROMPT=Suppress`

### 2) 程序包信息
建议填写：
- 程序包名称：`AIShepherd`
- 显示名称：`AI Shepherd`
- 发布者名称：`CN=shenfq`（开发用）
- 版本：`0.1.0.0`

### 3) 捕获安装
按安装向导完成安装。
- 不要勾选 “Launch AI Shepherd”
- 安装完成后回到 Packaging Tool，点 **下一步** 继续。

### 4) 首次启动任务
主包可以保持默认，不运行应用，直接 **下一步**。

### 5) 创建程序包
选择输出目录，如：
- `C:\Codes\ai-shepherd-interview\msix-out`
点 **创建**。

生成 MSIX 后，会看到 `.msix` 文件出现在输出目录。

## 四、获取 AppxManifest.xml
将 `.msix` 改成 `.zip` 解压即可获取：

```powershell
$msix = "C:\Codes\ai-shepherd-interview\msix-out\AIShepherd_0.1.0.0_x64__xxxx.msix"
Copy-Item $msix "$msix.zip" -Force
Expand-Archive -Path "$msix.zip" -DestinationPath "C:\Codes\ai-shepherd-interview\msix-out\_inspect_main"
```

`AppxManifest.xml` 位于解压目录根部。

## 五、常见问题

### 1) 找不到 .msix 文件
- 说明还没进入最后的“创建程序包”页面。
- 需要在输出位置页面点击 **创建**。

### 2) 报错 `Couldn't find a .ico icon`
- 检查 `src-tauri/icons/icon.ico` 是否存在。
- 确保 `tauri.conf.json` 的 `bundle.icon` 配置正确。

### 3) MSI 提示需要重启
- 在“安装程序参数”里加：
  `REBOOT=ReallySuppress REBOOTPROMPT=Suppress`

## 交付清单（主包）
- MSI：`src-tauri/target/release/bundle/msi/AI Shepherd_0.1.0_x64_en-US.msi`
- MSIX：`C:\Codes\ai-shepherd-interview\msix-out\*.msix`

如需，我可以继续整理“签名、发布、商店上传”的流程。




