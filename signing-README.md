# MSIX 签名流程（测试证书）

本文记录主包 + 可选包的**测试签名**流程。包含证书生成、信任导入、签名与验证命令。所有 PowerShell 命令逐行注释。

> 说明：这是本地测试流程，不适用于商店上架。上架需要受信任 CA 签发的证书。

## 一、生成测试证书（当前用户）
> 目标：生成可导出私钥的代码签名证书，Subject 必须与包内 Publisher 匹配（本项目为 `CN=shenfq`）。

```powershell
# 1) 生成自签名证书（包含 Code Signing EKU）
$cert = New-SelfSignedCertificate -Type Custom `
  -Subject "CN=shenfq" `                # 必须与 MSIX Publisher 一致
  -CertStoreLocation "Cert:\CurrentUser\My" `  # 放到当前用户证书库
  -KeyExportPolicy Exportable `          # 允许导出私钥
  -KeyAlgorithm RSA -KeyLength 2048 `    # RSA 2048
  -HashAlgorithm SHA256 `                # SHA256
  -KeyUsage DigitalSignature `           # 用于签名
  -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3")  # EKU: Code Signing

# 2) 导出为 PFX（带私钥）
$pwd = ConvertTo-SecureString -String "aishepherd-test" -Force -AsPlainText
Export-PfxCertificate -Cert "Cert:\CurrentUser\My\$($cert.Thumbprint)" `
  -FilePath "C:\Codes\ai-shepherd-interview\certs\aishepherd-test-3.pfx" `
  -Password $pwd

# 3) 导出 CER（公钥证书）
Export-Certificate -Cert "Cert:\CurrentUser\My\$($cert.Thumbprint)" `
  -FilePath "C:\Codes\ai-shepherd-interview\certs\aishepherd-test-3.cer"
```

## 二、将证书加入信任（本地安装必需）
> 否则双击安装会报 0x800B0109（根证书不受信任）。

```powershell
# 1) 加入当前用户受信任发布者
certutil -addstore -user TrustedPeople "C:\Codes\ai-shepherd-interview\certs\aishepherd-test-3.cer"

# 2) 加入当前用户受信任根证书
certutil -addstore -user Root "C:\Codes\ai-shepherd-interview\certs\aishepherd-test-3.cer"
```

> 如果系统仍提示不受信任，可用管理员 PowerShell 导入到“本地计算机”存储：

```powershell
# 本地计算机受信任根
certutil -addstore -f Root "C:\Codes\ai-shepherd-interview\certs\aishepherd-test-3.cer"

# 本地计算机受信任发布者
certutil -addstore -f TrustedPeople "C:\Codes\ai-shepherd-interview\certs\aishepherd-test-3.cer"
```

## 三、签名 MSIX（主包 + 可选包）
> 推荐使用“证书存储签名”，避免 PFX 导入失败。

```powershell
# 1) 固定证书 Thumbprint（示例）
$thumb = "39ABF19365B28E17E33CE2BE01DDC2158A1FDFB2"  # 你的证书指纹

# 2) signtool 路径
$sign = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe"

# 3) 需要签名的包路径
$main = "C:\Codes\ai-shepherd-interview\msix-out\AIShepherd_0.1.0.0_x64__rckefep75nkpm.msix"
$opt  = "C:\Codes\ai-shepherd-interview\msix-out\_build\AIShepherdGPU120Models_0.1.0.0_x64__rckefep75nkpm_fixed.msix"

# 4) 使用证书存储签名（/sha1 指定证书）
& $sign sign /fd SHA256 /sha1 $thumb $main
& $sign sign /fd SHA256 /sha1 $thumb $opt
```

## 四、验证签名（可选）
```powershell
# 验证签名链（可能在自签名下提示不受信任，属于正常）
& $sign verify /pa $main
& $sign verify /pa $opt
```

## 五、安装测试
```powershell
# 安装主包
Add-AppxPackage "C:\Codes\ai-shepherd-interview\msix-out\AIShepherd_0.1.0.0_x64__rckefep75nkpm.msix"

# 安装可选包
Add-AppxPackage "C:\Codes\ai-shepherd-interview\msix-out\_build\AIShepherdGPU120Models_0.1.0.0_x64__rckefep75nkpm_fixed.msix"
```

## 常见错误与修复
- **0x800B0109**：根证书不受信任 → 把 `.cer` 导入 Root / TrustedPeople
- **Store::ImportCertObject failed**：PFX 不含私钥 → 重新生成可导出私钥证书
- **验证失败但可安装**：自签名证书常见，导入 Root 后可消除

---

如果需要“商店上架签名流程”，我可以另出一份文档。
