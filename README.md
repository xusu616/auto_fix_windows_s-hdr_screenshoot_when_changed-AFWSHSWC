# AFWSHSWC · Windows 截图工具 HDR 矫正

Windows 11 开着系统 HDR 时，桌面、浏览器和游戏仍可能交替显示 SDR 与 HDR 内容。在本机报告的场景中，关闭截图工具的「HDR 屏幕截图颜色更正器」会让 HDR 视频或游戏的截图过曝，开启后又会让普通桌面截图偏暗。截图工具目前提供一个手动开关。微软的公开文档没有提供按画面自动切换这个开关的接口。

## 解决方法

- **保留 Windows 截图工具。** 启动 `dist/AFWSHSWC.exe`。程序在通知区域运行，左击图标打开主界面，右击可在托盘菜单操作。默认自动切换颜色更正器，继续使用 `Win + Shift + S` 截图。主界面可查看当前判断和设置值，也可选择「手动开启」或「手动关闭」暂停自动判断。关闭主界面会收回托盘，使用托盘菜单的「退出」结束程序。程序退出后保留最后一次设置。
- **使用自带 HDR 色调映射的截图工具。** Starshot 使用 Windows Graphics Capture，可输出色调映射后的 SDR PNG。支持 DXGI 桌面复制的机器也可试 HDRSnip，它能按显示器的 SDR 白场输出 PNG，并保留 HDR JPEG XR 导出。HDRSnip 的默认区域截图快捷键是 `Ctrl + Shift + S`。[Starshot 项目文档](https://github.com/loliri/Starshot#readme) · [HDRSnip 项目文档](https://github.com/mattcam98/HDRSnip#readme)
- **手动切换。** 在截图工具的「⋯ → 设置」里，截取过曝的 HDR 画面前开启「HDR 屏幕截图颜色更正器」，切回普通桌面后关闭。找不到选项时，可先在 Microsoft Store 更新截图工具。[微软截图工具文档](https://support.microsoft.com/en-us/windows/apps/use-snipping-tool-to-capture-screenshots)

## 自动模式如何判断

程序读取鼠标所在显示器的 DXGI 输出状态。在 HDR 输出上，它约每秒抽样一次桌面的 FP16 scRGB 画面，并按 Windows 为该显示器设置的 SDR 白场计算阈值。足够多的采样点明显高于 SDR 白场时开启更正，最近一次检测到高光满约五秒后，如果最新画面仍处在 SDR 亮度范围内则关闭。普通 SDR 输出直接关闭更正。采样失败时保留现有设置。画面只在本机内存中测量亮度。[微软 Advanced Color 文档](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range) · [SDR 白场 API](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-displayconfig_sdr_white_level)

当驱动拒绝 DXGI 桌面复制并返回 `DXGI_ERROR_UNSUPPORTED` 时，程序改用 Windows Graphics Capture。Windows 可能显示捕获边框。这个错误和输出模式、驱动或会话有关，虚拟显示驱动是可能原因之一。本机已用普通用户账户验证 Windows Graphics Capture 能读取画面。[DXGI 桌面复制 API](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_5/nf-dxgi1_5-idxgioutput5-duplicateoutput1) · [Windows Graphics Capture 文档](https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture)

游戏使用同一套亮度判断。检测阈值取 SDR 白场加上 12 nit 或白场的 10%，取两者中较大的增量。Auto HDR、驱动增强和原生 HDR 只要让合成画面出现足够多超过阈值的像素，都能触发矫正。程序无法从亮度推断游戏使用了哪种 HDR 格式。

亮度是画面的 HDR 高光证据。没有明显高光的 HDR10 暗场可能暂时被视为 SDR，受保护视频或只返回 8 位图像的独占全屏可能读不到可用画面。多个屏幕共用截图工具的一个设置，自动模式以鼠标所在屏幕为准。遇到这些场景可以用托盘菜单手动切换。

### Chrome 中的哔哩哔哩 HDR10

如果主要在 Chrome 看 B 站 HDR10，可以用 `--chrome-hdr` 启动。先在新版 Chrome 打开 `chrome://inspect/#remote-debugging`，开启远程调试并按浏览器提示授权。在 Windows PowerShell 中运行以下命令。如果托盘已有实例，先从托盘退出。[Chrome 远程调试说明](https://developer.chrome.com/blog/chrome-devtools-mcp-debug-your-browser-session)

```powershell
.\dist\AFWSHSWC.exe --chrome-hdr
```

联动通过本机回环地址查询前台 B 站播放器的当前清晰度。前台画面标为 HDR10 时，暗场也开启更正。切到其他页面或应用后恢复画面采样。Chrome 未授权、连接断开或者网页控件改版时，会继续使用画面亮度和托盘手动开关。

程序通过 Windows 注册表 API 打开当前用户的截图工具应用设置文件 `%LOCALAPPDATA%\Packages\Microsoft.ScreenSketch_8wekyb3d8bbwe\Settings\settings.dat`，只在设置需要变化时更新 `LocalState\IsHDRToneMappingEnabled`。该设置属于截图工具内部实现，目前核对了版本 `11.2609.15.0` 的键名与数据格式。如果从未使用过这个开关，先在截图工具里切换一次，以便它创建设置值。截图工具更新后如显示格式不匹配，程序会保留现有设置。[微软应用注册表配置单元文档](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/filtering-registry-operations-on-application-hives)

## 在 WSL 构建

在 x86-64 WSL 中，从项目目录运行下列命令。需要 Rustup、Cargo、curl 和 tar，首次构建会下载并校验 LLVM-MinGW，以及安装 Windows x64 Rust 目标。默认将中间产物存到 WSL 家目录的 `~/afwshswc-build`，生成的 Windows 程序复制到 `dist/AFWSHSWC.exe`。如果使用自带的 LLVM-MinGW，可指定 `CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER`，并在需要时通过 `AFWSHSWC_RC` 指定 `llvm-rc`。[Rust Windows GNU LLVM 目标](https://doc.rust-lang.org/rustc/platform-support/windows-gnullvm.html) · [LLVM-MinGW 发布](https://github.com/mstorsjo/llvm-mingw/releases/tag/20261117)

```bash
bash ./build-wsl.sh
cargo fmt --check
CARGO_TARGET_DIR="$HOME/afwshswc-build" cargo test --locked
```

从 Windows PowerShell 可以运行只读诊断。把鼠标移到要检查的屏幕，再分别观察普通桌面和 HDR 视频或游戏。

```powershell
.\dist\AFWSHSWC.exe --setting | Out-Host
.\dist\AFWSHSWC.exe --probe 10 | Out-Host
.\dist\AFWSHSWC.exe --chrome-hdr --probe 10 | Out-Host
```

## 本机验证

Windows 11 上的截图工具版本为 `11.2609.15.0`。在系统 HDR 保持开启时，B 站 HDR10 电影的暗场采样没有越过 128 nit 的 SDR 白场。开启 Chrome 联动后，诊断识别为 `Chrome HdrPlayback`，托盘程序能自动将实际设置值从关切到开。手动开关和退出也已验证。Minecraft 截图由用户实测通过。
