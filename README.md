# MAC 地址管理器

[![Windows Single EXE](https://github.com/time2045/rust-mac-revise/actions/workflows/build-windows.yml/badge.svg)](https://github.com/time2045/rust-mac-revise/actions/workflows/build-windows.yml)

当前版本：v0.3.2 · [下载最新版](https://github.com/time2045/rust-mac-revise/releases/latest)

最近更新：v0.3.2 修复发布流程，功能与 v0.3.1 相同；v0.3.1 支持地址方案一键应用和清空操作记录；v0.3.0 增加托盘常驻、开机自启和 Wi-Fi 随机地址检测；v0.2.1 精简侧栏权限状态；v0.2.0 增加地址漂移自动修复与关于页。

面向 Windows 的网卡 MAC 地址管理工具，使用 Rust 1.87 + Tauri 2 构建。支持地址方案持久保存、应用前确认、写入后回读校验、失败自动回滚和地址漂移提醒。

发布构建由 GitHub Actions 使用 MSVC 编译，并对产物执行依赖检查：单个 `mac-address-manager.exe` 不依赖旁加载的 `WebView2Loader.dll`。

## 下载与运行

1. 打开 [Releases](https://github.com/time2045/rust-mac-revise/releases/latest)。
2. 下载 `MAC-Address-Manager-vX.Y.Z.exe`，双击运行，无需安装。
3. GitHub 的资源显示标签为 `MAC地址管理器_vX.Y.Z.exe`；实际下载文件名会被 GitHub 改为英文，下载后可自行重命名。

GitHub Releases 会重写中文资源文件名，因此实际下载文件名使用英文；Release 标题和资源标签保留中文+版本号。Git 标签使用 `vX.Y.Z` 以保持自动构建触发稳定。

维护者推送 `v*` 标签后，GitHub Actions 会自动构建单文件并创建对应的 Release。

运行要求：

- Windows 10/11 x64。
- 系统需要 WebView2 Runtime，通常已随 Windows 或 Edge 安装。
- 如启动时提示缺少 `VCRUNTIME140.dll` 或 `VCRUNTIME140_1.dll`，安装 Microsoft Visual C++ 2015-2022 Redistributable（x64）。
- 修改网卡需要管理员权限，可在界面左下角点击“提升管理员权限”。
- “单文件”指不需要在 EXE 旁边放置 DLL；程序仍使用系统的 WebView2 和 Windows 系统库。

本地数据保存在 `%LOCALAPPDATA%\MacAddressManager`，方案不会随 EXE 移动。需要迁移时可使用地址方案页的 JSON 导入/导出。

## 使用方法

1. 在网卡列表中选择设备，查看当前地址、驱动报告的永久地址和注册表覆盖地址。
2. 输入或随机生成待应用地址。首字节必须为偶数；建议使用 `02` 开头的本地管理单播地址。
3. “存为方案”只保存到本机，不会修改网卡。
4. “应用地址”会再次确认，随后写入配置、重启网卡并回读校验；网络可能短暂中断。
5. “恢复默认”删除手动地址覆盖，最终地址由驱动和 Windows 决定，不保证等于出厂地址。
6. 写入失败会自动尝试回滚；如果出现“未完成操作”，请先点击“恢复上次配置”，再继续修改。
7. 需要自动恢复漂移时，在网卡详情启用“自动修复漂移”；它仅在程序运行期间生效。
8. 关闭窗口后程序驻留托盘；从托盘菜单可以显示窗口或退出。
9. “设置”中可开启开机自启；启动后驻留托盘，后台继续执行自动修复。
10. Wi-Fi 网卡会提示已保存网络的随机硬件地址状态，可点击“打开设置”调整。
11. “地址方案”和“操作记录”支持搜索；历史记录可以分页查看。
12. 方案卡片和网卡详情可直接“应用”，仍保留一次写入确认；清空操作记录只删除日志，不影响方案、预期目标或网卡配置。
13. 清空操作记录不可撤销。

## 主要功能

- 枚举物理网卡和虚拟网卡，显示当前、永久和覆盖地址。
- 保存、搜索、删除地址方案，支持绑定网卡和 JSON 导入/导出。
- 方案卡片、网卡详情均可一键应用，只需一次写入确认。
- 操作记录支持搜索、分页和一键清空；清空只删除日志。
- 应用前二次确认；写入后校验注册表配置和实际地址；失败自动回滚。
- 原本禁用的网卡保持禁用，不会为了写入而自动启用。
- 未完成操作会保留恢复记录，阻止重复写入。
- 记录每次写入尝试、结果和错误原因，支持分页与搜索。
- 地址漂移提醒；可按网卡启用自动修复，程序运行期间发现漂移后自动重应用。
- 自动修复默认关闭，启用前会确认；连续失败或恢复未完成时自动停用。
- 关于页显示版本、GitHub 仓库、发行版和数据目录。
- 关闭窗口后驻留系统托盘，可从托盘显示窗口或退出。
- 可选开机自启；开机启动时驻留托盘，后台继续执行自动修复。
- 检测已保存 Wi-Fi 网络的随机硬件地址设置并提示，不会修改系统设置。
- 全程本地运行，不向网络上传设备信息或地址方案。

## 重要限制

- 驱动是否接受地址覆盖决定最终效果。部分 Intel Wi-Fi 网卡不支持任意 MAC，注册表写入成功不代表实际生效。
- Windows 的随机硬件地址功能可能覆盖预期地址。需要在系统设置中分别关闭全局和每个已保存 Wi-Fi 网络的随机地址；本工具不会擅自修改这些开关。
- 驱动升级、VPN、系统还原或手动设置可能重置覆盖。自动修复依赖程序进程运行；默认仅驻留托盘，可选开机自启，不安装系统服务。
- 开机自启写入当前用户的 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`；移动 EXE 后需重新开启。
- 随机硬件地址检测读取已保存 WLAN 配置，只提示不修改；Windows 全局开关请在系统设置中确认。
- 恢复上次配置时可能需要启用或禁用网卡，并可能短暂断网；不要在远程连接中修改当前正在使用的网卡。
- 仅用于自己拥有或获得明确授权的设备，遵守当地法律法规。

## 从源码构建

### GitHub Actions（推荐，单文件）

工作流文件：`.github/workflows/build-windows.yml`

- 运行器：`windows-latest`
- 工具链：MSVC
- 执行 `cargo test --locked` 和 `cargo build --release --locked`
- 使用 `dumpbin /dependents` 检查 EXE 不导入 `WebView2Loader.dll`
- 上传单个 EXE 作为 artifact

也可以在 Actions 页面手动点击“Run workflow”。

### 本地构建

需要 Rust 1.87+、Visual Studio Build Tools（C++ 桌面开发工作负载）和 Windows SDK：

```powershell
cargo test --locked
cargo build --release --locked
```

产物为 `target\release\mac-address-manager.exe`。MSVC 工具链会把 WebView2 加载器静态链接进 EXE，因此不需要旁边的 `WebView2Loader.dll`。

项目根目录的 `build.ps1` 是本机 GNU 工具链的备用构建脚本；GNU 构建会生成 `WebView2Loader.dll` 旁加载依赖，不属于严格单文件发布。GitHub Actions 的 MSVC 构建才是单文件发布路径。

## 目录结构

```text
src/                  Rust 后端：网卡枚举、注册表、校验、恢复、数据持久化
ui/                   Tauri 前端：HTML/CSS/JavaScript 与浏览器演示模式
icons/                应用图标
.github/workflows/    GitHub Actions 单文件构建
build.rs              Tauri 资源构建脚本
build.ps1             本机 GNU 工具链备用构建脚本
tauri.conf.json       Tauri 配置
Cargo.toml            依赖与发布配置
```
