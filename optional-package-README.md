# Optional Package (Models) 打包记录与脚本

本文记录了“模型可选包”的完整打包流程，并包含我实际使用过的修复脚本。所有命令为 PowerShell，且每一行都带注释。

## 目标
- 生成可选包（Optional Package），仅包含模型文件（不含可执行代码）。
- 可选包需依赖主包：
  - `Name = AIShepherd`
  - `Publisher = CN=shenfq`
  - `Version = 0.1.0.0`

## 关键路径
- 主包 Identity（已确认）：
  - `AIShepherd` / `CN=shenfq` / `0.1.0.0`
- 模型 staging 目录（必须是文件夹）：
  - `C:\Codes\ai-shepherd-interview\install\optional-models\models\ggml-small-q5_1.bin`
- Optional 包输出目录（建议）：
  - `C:\Codes\ai-shepherd-interview\msix-out\_build`

## 一、使用 MSIX Packaging Tool 创建初始可选包
1. Create package
2. 在此计算机上创建程序包
3. 选择“没有安装程序 / 从文件创建包”（如果界面写的是“Package from files”就选它）
4. 程序包信息（示例）
   - 程序包名称：`AIShepherdGPU120Models`
   - 程序包显示名称：`AI Shepherd GPU120 Models`
   - 发布者名称：`CN=shenfq`
   - 版本：`0.1.0.0`（建议与主包一致）
5. 可选包依赖（若界面有）：
   - 主包依赖 Name：`AIShepherd`
   - Publisher：`CN=shenfq`
   - MinVersion：`0.1.0.0`
6. 添加内容（重要）：
   - 选择目录：`C:\Codes\ai-shepherd-interview\install\optional-models\models`
   - 包内路径应为：`models\ggml-small-q5_1.bin`
7. 首次启动任务：保持空，直接下一步
8. 生成包：保存到 `C:\Codes\ai-shepherd-interview\msix-out`

> 注意：如果工具没有提供“主包依赖”输入项，生成的包会缺少 `MainPackageDependency`，需要后续修复（见下节脚本）。

## 二、修复 MainPackageDependency 并清理杂项（脚本）
用途：
- 添加 `MainPackageDependency`（避免可选包变成普通内容包）
- 去掉捕获过程带进来的无关文件（VFS/Registry.dat/User.dat）
- 重新打包为最终可分发的 `.msix`

> 前提：已安装 Windows SDK（包含 `makeappx.exe`）。

```powershell
# 1) 设置 MakeAppx 路径（按本机 Windows SDK 版本调整）
$makeappx = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\makeappx.exe"  # MakeAppx 路径

# 2) 指定原始可选包与工作目录
$msix = "C:\Codes\ai-shepherd-interview\msix-out\AIShepherdGPU120Models_0.0.1.0_x64__rckefep75nkpm_1.msix"  # 原始包
$work = "C:\Codes\ai-shepherd-interview\msix-out\_opt_fix"  # 解包目录

# 3) 解包到工作目录
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue  # 清理旧目录
New-Item -ItemType Directory -Force -Path $work | Out-Null       # 创建目录
& $makeappx unpack /p $msix /d $work /o                          # 解包

# 4) 修改 AppxManifest.xml，添加 MainPackageDependency
#    这里使用 uap4 版本的 MainPackageDependency（含 Publisher 属性）
$manifest = "$work\AppxManifest.xml"                            # manifest 路径
(Get-Content -Raw $manifest) `
  -replace 'xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"', `
           'xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10" xmlns:uap4="http://schemas.microsoft.com/appx/manifest/uap/windows10/4"' `
  -replace 'IgnorableNamespaces="([^"]*)"', 'IgnorableNamespaces="uap4 $1"' `
  -replace '</Dependencies>', "  <uap4:MainPackageDependency Name=\"AIShepherd\" Publisher=\"CN=shenfq\" />`n  </Dependencies>" `
  | Set-Content -Encoding UTF8 $manifest                          # 写回 manifest

# 5) 构建干净的打包目录（只保留必要文件）
$clean = "C:\Codes\ai-shepherd-interview\msix-out\_opt_clean" # 干净目录
Remove-Item -Recurse -Force $clean -ErrorAction SilentlyContinue  # 清理旧目录
New-Item -ItemType Directory -Force -Path $clean | Out-Null       # 创建目录
New-Item -ItemType Directory -Force -Path "$clean\Assets" | Out-Null # Assets
New-Item -ItemType Directory -Force -Path "$clean\models" | Out-Null  # models
Copy-Item -Force "$work\AppxManifest.xml" "$clean\AppxManifest.xml"  # manifest
Copy-Item -Force "$work\Assets\StoreLogo.png" "$clean\Assets\StoreLogo.png"  # logo
Copy-Item -Force "$work\models\ggml-small-q5_1.bin" "$clean\models\ggml-small-q5_1.bin"  # 模型
Copy-Item -Force "$work\Resources.pri" "$clean\Resources.pri"  # PRI

# 6) 更新版本号到 0.1.0.0（与主包一致）
(Get-Content -Raw "$clean\AppxManifest.xml") `
  -replace 'Version="0\.0\.1\.0"', 'Version="0.1.0.0"' `
  | Set-Content -Encoding UTF8 "$clean\AppxManifest.xml"        # 写回版本

# 7) 重新打包输出
$outDir = "C:\Codes\ai-shepherd-interview\msix-out\_build"    # 输出目录
$out = "$outDir\AIShepherdGPU120Models_0.1.0.0_x64__rckefep75nkpm_fixed.msix"  # 输出包
New-Item -ItemType Directory -Force -Path $outDir | Out-Null      # 创建输出目录
Remove-Item -Force $out -ErrorAction SilentlyContinue             # 清理旧包
& $makeappx pack /d $clean /p $out /o                              # 重新打包
```

## 三、验证包内容与依赖（可选）
```powershell
# 1) 解包验证（.msix 改成 .zip 再解压）
$msix = "C:\Codes\ai-shepherd-interview\msix-out\_build\AIShepherdGPU120Models_0.1.0.0_x64__rckefep75nkpm_fixed.msix"  # 最终包
$dest = "C:\Codes\ai-shepherd-interview\msix-out\_inspect_fixed"  # 检查目录
Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue     # 清理旧目录
Copy-Item $msix "$msix.zip" -Force                                 # 复制为 zip
Expand-Archive -Path "$msix.zip" -DestinationPath $dest            # 解压

# 2) 验证主包依赖是否存在
Select-String -Path "$dest\AppxManifest.xml" -Pattern "Identity|MainPackageDependency"  # 查看依赖

# 3) 验证模型文件存在
Get-ChildItem -Recurse "$dest\models"                              # 应看到 ggml-small-q5_1.bin
```

## 四、安装验证（主包 + 可选包）
```powershell
# 先安装主包（替换为你的主包路径）
Add-AppxPackage "C:\path\to\AIShepherd_main.msix"  # 安装主包

# 再安装可选包（本次最终包路径）
Add-AppxPackage "C:\Codes\ai-shepherd-interview\msix-out\_build\AIShepherdGPU120Models_0.1.0.0_x64__rckefep75nkpm_fixed.msix"  # 安装可选包

# 验证已安装
Get-AppxPackage AIShepherd             # 主包
Get-AppxPackage AIShepherdGPU120Models # 可选包
```

## 交付清单（最终）
- 主包：由 Tauri 打包（MSI -> MSIX）
- 可选包：
  - `C:\Codes\ai-shepherd-interview\msix-out\_build\AIShepherdGPU120Models_0.1.0.0_x64__rckefep75nkpm_fixed.msix`

如需，我可以把这个流程扩展成“多 GPU 架构可选包矩阵”的模板。
