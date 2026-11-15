# 开发

[← 返回 README](../README.zh-CN.md) · [English](development.md)

## 本 fork：先本机门禁，再 Mac 验收

以[直连设计](specs/tailcat-direct-mvp.md)、[功能映射](specs/tailcat-direct-feature-map.md)和[验证记录](tailcat-direct-validation.md)为准。两份导入交接文件保持与归档来源逐字节一致；不把旧中转升级用例当作本直连 MVP 的验收规格。

```bash
cargo test -p airhand-link --lib --test direct_session --locked
(cd p2p && go test -run 'Test(HelperBinary|PreparedTLS|ControlRejects|ControlProtocol|HelperArguments)' ./...)
(cd client && npm run typecheck && npm test)
```

二进制测试用本地 DERP/STUN 和真实 helper 进程，检查 direct 数据报、白名单外密钥拒绝、DERP 数据计数冻结及 EOF 退出，不渲染桌面。旧中转升级先红夹具仍有未实现部分，完整测试集须单列这些失败，不静默跳过，也不冒称全绿。

直连使用独立 ALPN `airhand-direct/1` 和版本，中转协议 v9 不变。按[构建](building.zh-CN.md)生成签名 App 后，分别在同网和热点执行设计 §9 全部正向、负向、故障和秘密检查。记录拓扑/DERP 区域但不公开真实地址，记录首连/PTO、延迟、码率、双方 CPU、TCC 与本地网络弹窗。实测释放时限与故障窗口 DERP 计数，未测前不声称零包或达到 1–1.5 秒目标。双机通过前不新增 CI。

下方工具保留原来的中转行为，不是 Tailcat App 验收命令。调试日志、截图及私有诊断证据分享前须检查并脱敏。

## 测试

> upstream 中转模式现状，非本 fork 目标设计。

```bash
# 仓库根目录：协议 + 链路库 + 服务器
cargo test

# web/：网页控制台（WebSocket 协议、访问密码、HTTPS 证书）
cargo test

# client/：界面（抖动缓冲和音画同步仿真、H.264 格式转换、界面文案）
npm test

# client/src-tauri/：客户端 Rust 部分（编码器含硬件探测、Opus 打包、格式转换等）
cargo test --lib
# 声音环回采集（需要声音输出设备，会以 -60 dBFS 播放 1.5 秒测试音，几乎听不见）
cargo test --lib loopback_captures_tone -- --ignored --nocapture

# android/native/：Android 的 Rust 核心（在当前电脑上跑：在途帧控制、关键帧、声音分发、命令格式）
cargo test
# android/：手势（触控板 / 触屏 / 多点触摸 / 直通）、键盘、控制码、文案（Rust 的每种问题 / 提示 / 系统操作都有中英文文案）
./gradlew testDebugUnitTest -Pairhand.abis=x86_64
```

测试 Android 被控端不需要第二台手机：`android/native` 里有一个协议级的控制端，在电脑上连上手机的控制码，统计画面 / 声音，还可以点一下、打字：

```bash
# android/native/（模拟器里的应用把服务器设为 10.0.2.2:4433 就能连到本机的服务器）
cargo run --example probe -- <手机的控制码> 8 --tap 0.5,0.5 --text hello --audio
# 多点触摸和系统操作：双指张开、打开快捷设置（手机设置了“连接前先问我”时要先在手机上允许，计时从允许开始）
cargo run --example probe -- <手机的控制码> 6 --pinch --action QuickSettings
```

macOS 上，屏幕 / 声音采集（`sck_capture`、`loopback_captures_tone`、端到端测试里的画面和声音）和键鼠注入（`mac_move_to_center`）的测试需要先在“系统设置 → 隐私与安全性”里给运行测试的终端授予“屏幕与系统录音”和“辅助功能”权限。没有权限时 `sck_capture`、`loopback_captures_tone`、`mac_move_to_center` 会直接跳过；端到端测试会检查被控端发来的是“缺少权限”的提示而不是“采集失败”，然后跳过画面和声音的检查（剪贴板、Ping、断开等其余部分照常检查）。

端到端测试（真实服务器、带屏幕采集和编码的真实被控端、协议级控制端）：

```bash
# 仓库根目录：启动一个不固定证书的服务器
cargo build -p airhand-server
AIRHAND_CERT=target/e2e/cert.pem AIRHAND_KEY=target/e2e/key.pem ./target/debug/airhand-server &

# client/src-tauri/
cargo test --lib e2e_roundtrip -- --ignored --nocapture   # 画面 / 编码切换 / 剪贴板 / 声音 / Ping / 断开
cargo test --lib e2e_reconnect -- --ignored --nocapture   # 服务器重启后取回控制码、更换控制码
```

## 调试工具

> upstream 中转模式现状，非本 fork 目标设计。

```bash
# client/：直接进入控制端的调试构建；AIRHAND_LOG 控制日志级别
AIRHAND_DEV_CONTROLLER=1 AIRHAND_LOG=debug npm run tauri dev
# client/：音画同步实测（被控端播放闪白 + 短音，用法见脚本开头）
node scripts/measure-avsync.mjs <控制码>

# client/src-tauri/：无界面被控端，用来在同一台机器上调试控制端
#（请在控制端静音，否则播放出的声音会被再次采集，形成回声）
cargo run --release --example headless_host
# client/src-tauri/：编码性能测试（抓真实屏幕，对比各编码器）
cargo run --release --example encode_bench
```

测画面流畅度或音画同步时请用 release 构建：调试构建缩放很慢（4K → 1080p 每帧约 0.5 秒）。

## 调试 Windows 服务

> upstream 中转模式现状，非本 fork 目标设计。

调试构建在托盘进程里运行被控端，`npm run tauri dev` 不需要安装。要调试服务本身，可以装上安装程序，或者在管理员终端里直接注册某个构建：

```powershell
# client/src-tauri/
target\release\airhand.exe --install       # 为这个 airhand.exe 注册并启动服务（在它旁边生成 config.toml）
target\release\airhand.exe --stop-service  # 重新构建之前先停止：exe 正在使用
target\release\airhand.exe --uninstall     # 删除服务和开机启动项
```

服务和被控端的日志用 `OutputDebugString` 输出：以管理员身份运行 Sysinternals 的 DebugView，勾选 Capture → Capture Global Win32。要改日志级别，在 `HKLM\SYSTEM\CurrentControlSet\Services\Airhand` 下新建多字符串值 `Environment`，写入 `AIRHAND_LOG=debug`，然后重启服务。服务装着的时候，从别处启动的调试构建仍然在自己的进程里运行被控端（有自己的 `config.toml` 和控制码）。

## 网页控制台

> upstream 中转模式现状，非本 fork 目标设计。

```bash
# 先构建界面，再在 web/ 里启动服务（默认连接 127.0.0.1:4433 的服务器，网页取自 ../client/dist），浏览器打开 http://localhost:8080
(cd client && npm run build)
cd web && cargo run
# 修改界面时：在 client/ 里 npm run dev，浏览器打开 http://localhost:1420（/ws 会转给 8080 端口上的 airhand-web）
```

`web/src/console.rs` 开头写了浏览器和服务之间的 WebSocket 协议。

## Linux / WSL

> upstream 中转模式现状，非本 fork 目标设计。

`bash client/scripts/linux-test.sh` 一次跑完上面所有测试（单元、X11 采集与输入注入、声音环回、端到端、前端）。X11 相关测试会开全屏窗口、注入键鼠，所以脚本自己开一个 Xvfb 虚拟屏幕，不影响当前桌面；没有 PulseAudio / PipeWire 时跳过声音测试。需要 `sudo apt install xvfb x11-apps`。

在 Windows 上可以用 WSL2（Ubuntu 24.04 测过，WSLg 自带 PulseAudio）：

```bash
# 先把仓库复制到 WSL 自己的磁盘上（在 /mnt/c、/mnt/d 下编译慢很多）
rsync -a --exclude target --exclude node_modules /mnt/<盘符>/<仓库路径>/airhand/ ~/airhand/
cd ~/airhand && bash client/scripts/linux-test.sh
```

- 控制台可以在 WSLg 里运行：`AIRHAND_DEV_CONTROLLER=1 npm run tauri dev`，窗口出现在 Windows 桌面上
- 运行被控端：`Xvfb :99 &`，然后 `DISPLAY=:99 cargo run --example headless_host`（WSLg 自己的桌面是 Wayland）
- WSL2 默认是 NAT 网络：Windows 上的程序要通过 WSL 的 IP（`ip addr show eth0`）连 WSL 里的服务器，例如 `AIRHAND_SERVER=172.30.x.x:4433`

## 约定

> upstream 中转模式现状，非本 fork 目标设计。

### 协议版本

> upstream 中转模式现状，非本 fork 目标设计。

客户端和服务器连接时比较 `PROTOCOL_VERSION`（`crates/airhand-proto/src/lib.rs`）。不一致时服务器回复 `Incompatible`，客户端提示用户该更新 Airhand 还是服务器，之后每分钟重试一次。任何不兼容的协议改动都要把版本号加 1。握手本身（`ClientMsg::Hello`、`ServerMsg::Welcome` / `Incompatible`、ALPN）**永远不能改**，否则不同版本之间连“版本不一致”都无法告诉对方；`version_handshake_wire_is_stable` 测试守着这一点。

### 一份协议，三份实现

> upstream 中转模式现状，非本 fork 目标设计。

Rust、控制台（TypeScript）和 Android（Kotlin）用同一套格式：媒体包（`crates/airhand-proto/src/media.rs`）、输入事件、界面命令（`airhand_link::controller::Command`）。`crates/airhand-link/tests/protocol.json` 是 Rust 测试生成的样例；`npm test`（`tests/protocol.test.ts`）和 `./gradlew testDebugUnitTest`（`ProtocolTest`）用同一个文件检查各自的解析和拼装。改了格式之后用 `AIRHAND_UPDATE_FIXTURES=1 cargo test -p airhand-link --test protocol` 重新生成，再让另外两边的测试通过。

### 消息是类型，不是文字

> upstream 中转模式现状，非本 fork 目标设计。

凡是经过网络、或者从 Rust 端传给界面、可能显示给用户的内容，都是枚举（`ConnectError`、`Notice`、`ByeReason`、`Problem` 等），由界面按用户的语言变成文字：

- 控制台的文案在 `client/src/locales/zh.ts`（全部词条以它为准）和 `en.ts`
- 托盘的文案用 `tray.rs` 里的 `tr!`（`client/src-tauri/src/tray.rs`）
- 某个 Rust 的 `Problem` / `NoticeView` 在任一语言里没有文案时，`npm test` 会失败
- Android 的文案在 `android/app/src/main/res/values/strings.xml`（英文，默认）和 `values-zh/strings.xml`，类型和文案的对应关系在 `Messages.kt`。Rust 的 `Problem` / `NoticeView` 在 Android 上没有文案、带 `detail` 的文案没有显示它、或者两种语言的词条和参数不一致时，`./gradlew testDebugUnitTest` 会失败（`MessagesTest`）
- 日志和内部错误一律用英文

### 图标

> upstream 中转模式现状，非本 fork 目标设计。

源文件是 `assets/icon.svg`。修改后在 `client/` 下运行 `npx tauri icon ../assets/icon.svg -o src-tauri/icons`，然后把 `src-tauri/icons/32x32.png` 复制为 `src-tauri/icons/tray.png`。
