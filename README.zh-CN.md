<p align="center"><img src="assets/icon.svg" width="96" alt="Airhand"></p>

<h1 align="center">Airhand</h1>
<p align="center">基于 QUIC 的高性能远程桌面 · Rust + Tauri + React</p>
<p align="center"><a href="README.md">English</a> · <b>简体中文</b></p>

---

Airhand 是一个程序，同时包含**被控端**（被远程控制的电脑）和**控制端**（你用来操作的电脑），支持 **Windows** 和 **Linux**（X11 桌面）。另有一个很小的中转服务器，用 Docker Compose 一条命令部署，负责信令和转发。

> **状态：**开发中。协议在版本之间还会变化，客户端和服务器需要用同一个版本构建（版本不一致时会检测出来并提示，见[协议版本](#协议版本)）。

## 功能

- **默认是被控端**：启动后只有一个托盘图标，没有窗口，也不启动 WebView
- **控制台**：**同时控制多台电脑**，随时切换，或者在多屏总览里同时看所有画面
- **低延迟、省带宽**：硬件 **AV1 / HEVC / H.264** 编码，画面不变就不发帧，自适应码率，鼠标指针在本地移动、没有延迟
- **声音**：被控端的系统声音同步到控制端，看视频时可以打开音画同步
- **实用工具**：剪贴板双向同步、以键盘输入方式发送剪贴板、系统快捷键（Win、Alt+Tab、Ctrl+Shift+Esc 等）、多显示器、全屏时锁定键盘
- **零文件**：没有配置文件，不在磁盘上留下任何东西（见[下文](#零文件)）
- **默认安全**：TLS 1.3 + 证书指纹固定，控制码防暴力猜测
- **中英双语**：跟随系统语言，可以随时切换

## 目录

- [快速开始](#快速开始)
- [使用](#使用)
- [服务端配置](#服务端配置)
- [构建客户端](#构建客户端)
- [工作原理](#工作原理)
- [安全](#安全)
- [已知限制](#已知限制)
- [开发](#开发)

## 快速开始

1. **部署服务端**：放在两台电脑都能访问到的机器上（只需开放 **UDP 4433**）：

   ```bash
   docker compose up -d
   docker compose logs airhand | grep fingerprint   # 记下证书 SHA-256 指纹
   ```

2. **构建客户端**，把服务器地址和指纹编译进程序（依赖和 Linux 的步骤见[构建客户端](#构建客户端)）：

   ```powershell
   cd client
   npm install
   $env:AIRHAND_SERVER    = "relay.example.com:4433"
   $env:AIRHAND_SERVER_FP = "<第 1 步记下的指纹>"
   npm run tauri build -- --no-bundle
   # → client/src-tauri/target/release/airhand.exe（单个文件，约 9 MB）
   ```

3. **两台电脑上都运行它**。在被控的电脑上右键托盘图标，看到 6 位**控制码**；在你自己的电脑上从托盘菜单选择**切换为控制端**，输入控制码。

## 使用

### 被控端（默认）

双击运行，右下角（Linux 是顶栏 / 任务栏的状态区）出现托盘图标：灰色表示没连上服务器，彩色表示已连接。右键菜单里有：

- 当前模式、服务器状态
- **控制码**和"复制控制码"
- 正在控制本机的控制端，以及**断开所有控制端**
- **更换控制码**：旧码立即失效，已连接的控制端会被断开
- 切换为控制端 / 语言 / 退出

掉线后会按指数退避自动重连，并且**取回原来的控制码**（服务器重启后也能取回）。被控端离线后 10 分钟内，它的控制码不会分配给别人。

> Linux 的托盘图标走 AppIndicator：Ubuntu、KDE、Xfce 等开箱即用，原版 GNOME 需要装 "AppIndicator and KStatusNotifierItem Support" 扩展。没有 AppIndicator 库时程序照常作为被控端运行，控制码打印在日志（stderr）里。

### 控制端

托盘 →**切换为控制端**，打开控制台。关闭控制台窗口会回到被控端模式。

- 输入 6 位控制码连接（可以直接粘贴 `123 456`）。可以连任意多台，在左侧列表切换，或者打开**多屏总览**
- 工具栏：显示器切换、**画质**、快捷键、剪贴板（双向同步 / 以键盘输入方式发送）、声音（静音 / 音画同步）、1:1 原始大小、全屏、断开
- 全屏时会尝试锁定键盘，Win 键、Alt+Tab 这类系统快捷键也发给远端
- 网络抖动导致的断开会自动重连；被控端的用户断开了你时，控制台会说明原因，不再重连

**画质面板**：预设从省流（720p / 24 fps / 1.2 Mbps）、均衡（1080p / 30 fps / 4 Mbps）、流畅（1080p / 60 fps / 8 Mbps）、高清（1440p / 60 fps / 16 Mbps）到原画（原始分辨率 / 60 fps / 30 Mbps），也可以自己调分辨率上限、帧率（15–120）、码率上限（0.5–50 Mbps）、编码格式和自适应码率。控制台只提供本机能硬件解码的格式；被控端没有对应的硬件编码器时会自动回退，工具栏上显示实际用的编码器。

### 界面语言

控制台和托盘菜单支持**简体中文**和**英文**。默认跟随系统显示语言：系统是中文就用中文，否则用英文。可以在托盘菜单"语言 / Language"或控制台左下角的"中文 / EN"随时切换；和其它设置一样不保存。

对方电脑和服务器发来的提示、错误都是消息类型而不是文字，所以总是按**你自己**的语言显示。日志（客户端和服务端）统一是英文。

## 服务端配置

服务器监听 **UDP 4433**。首次启动时会在 `docker-compose.yml` 同目录的 `data/` 下生成自签名证书，**请保留这个目录**：客户端靠证书指纹认出服务器。

环境变量（见 `docker-compose.yml`）：

| 变量 | 默认值 | 说明 |
|---|---|---|
| `AIRHAND_LISTEN` | `0.0.0.0:4433` | 监听地址（UDP） |
| `AIRHAND_CERT` / `AIRHAND_KEY` | `/data/cert.pem` / `/data/key.pem` | 证书路径，不存在时自动生成 |
| `AIRHAND_CERT_NAMES` | `airhand,localhost` | 自动生成证书时写入的 SAN |
| `AIRHAND_MAX_SESSIONS` | `4` | 每台被控端同时允许的控制端数量 |
| `RUST_LOG` | `info` | 日志级别 |

镜像基于 distroless、以非 root 用户运行，约 28 MB。

## 构建客户端

需要：Rust（stable）、Node.js 20+、CMake（编译 libopus）。

- **Windows**：Windows 10/11（系统自带 WebView2）
- **Linux**：Tauri 的依赖、PulseAudio 开发库，以及控制台解码视频用的 GStreamer 插件。Debian / Ubuntu：

  ```bash
  sudo apt install build-essential cmake pkg-config libssl-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev libxdo-dev libpulse-dev \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
  ```

服务器地址和证书指纹**编译进程序**：构建时设置 `AIRHAND_SERVER` 和 `AIRHAND_SERVER_FP`（同[快速开始](#快速开始)），或者修改 `client/src-tauri/src/config.rs` 里的默认值。不设置 `AIRHAND_SERVER` 时连接 `127.0.0.1:4433`。

> `AIRHAND_SERVER_FP` 留空时**不校验服务器证书**，无法防止中间人攻击，只适合开发环境；这时控制台左上角会显示"未固定证书"。

**Windows**：`npm run tauri build -- --no-bundle` 生成 `client/src-tauri/target/release/airhand.exe`；去掉 `--no-bundle` 会额外生成 NSIS 安装包。

- C 运行时是静态链接的，不需要装 VC++ 运行库
- Media Foundation 是延迟加载的，Windows N 版（没装媒体功能包）也能运行，会改用软件编码

**Linux**（环境变量用 `export AIRHAND_SERVER=...` 设置）：

```bash
cd client && npm install
npm run tauri build
# → client/src-tauri/target/release/airhand（单个文件，约 10 MB）
#   client/src-tauri/target/release/bundle/deb/Airhand_<版本>_amd64.deb（会自动装上托盘、声音、GStreamer 依赖）
```

## 工作原理

### 架构

```
 被控端 (Windows，Linux 见下表)       airhand-server (Docker)                 控制端 (Tauri)
┌──────────────────────┐            ┌──────────────────────┐            ┌──────────────────────┐
│ DXGI 桌面复制 (GPU)   │            │ 控制码注册 / 找回     │            │ WebCodecs 硬件解码    │
│ SIMD 缩放 + NV12 转换 │  QUIC/TLS  │ 会话配对              │  QUIC/TLS  │ Canvas 低延迟渲染     │
│ 硬件编码 AV1/HEVC/264 │◀══════════▶│ 字节级零拷贝透传      │◀══════════▶│ 输入采集 + 光标同步   │
│ SendInput 注入        │   UDP 4433 │ 防暴力猜码限速        │   UDP 4433 │ 多会话 / 多屏总览     │
│ WASAPI 环回 + Opus    │            │                       │            │ 声音抖动缓冲播放      │
└──────────────────────┘            └──────────────────────┘            └──────────────────────┘
```

每条 QUIC 连接上的第一条双向流是**控制流**（信令）。每建立一个远控会话，双方各再开一条双向流，服务器按会话 ID 把两条流配对，之后**只转发字节、不解析画面**。每条消息是 `u32` 长度前缀加 postcard 编码的内容。

| 目录 | 内容 |
|---|---|
| `crates/airhand-proto` | 协议：信令、会话消息、帧格式 |
| `server/` | 中转服务器（quinn + rustls）及其 `Dockerfile` |
| `client/src-tauri/` | 客户端 Rust 部分：被控端 `host/`、控制端 `controller/`、托盘、网络 |
| `client/src/` | 控制台界面（React 19 + TypeScript，不依赖 UI 库）；文案在 `client/src/locales/` |
| `assets/icon.svg` | 应用图标源文件 |

被控端各环节在两个平台上的实现：

| 环节 | Windows | Linux（X11） |
|---|---|---|
| 屏幕采集 | DXGI Desktop Duplication | MIT-SHM 共享内存 + DAMAGE 变化通知，RandR 多显示器 |
| 光标形状 | DXGI | XFIXES |
| 视频编码 | Media Foundation 硬件编码，OpenH264 兜底 | OpenH264 |
| 输入注入 | SendInput（扫描码） | XTEST（evdev 物理键码） |
| 声音采集 | WASAPI 环回 | PulseAudio / PipeWire 的输出监视器 |
| 剪贴板 | 剪贴板序号检测变化 | XFIXES 选择所有者变化通知 |
| 控制台界面 | WebView2 | WebKitGTK（WebCodecs 由 GStreamer 解码） |

### 画面

| 环节 | 做法 |
|---|---|
| 采集 | 只在画面变化时产生帧（Windows 靠 DXGI，X11 靠 DAMAGE），静止的桌面几乎不占带宽 |
| 预处理 | `fast_image_resize`（SIMD）缩放，`yuvutils-rs`（AVX2）BGRA→NV12/I420，缓冲区全部复用 |
| 编码 | **硬件优先**：Media Foundation 硬件编码器（NVENC / Quick Sync / AMF），支持 **AV1 / HEVC / H.264**，低延迟模式、无 B 帧、超长 GOP；没有硬件编码器时回退到 **OpenH264 屏幕内容模式** |
| 码率控制 | 控制端每帧都回 ACK，被控端限制**在途帧数**（≈ RTT × 帧率 + 2），网络慢时等待并**跳到最新一帧**，不会越积越卡；自适应码率按拥塞情况下调，恢复后逐步回升 |
| 传输 | QUIC + **BBR** 拥塞控制（Wi-Fi、跨境这类丢包网络下比 Cubic 稳）；控制消息、声音、画面分开排队，Ping 和鼠标不会排在大帧后面 |
| 解码 | 控制端用 **WebCodecs** GPU 解码（`optimizeForLatency`），解完马上画到 `desynchronized` Canvas |
| 光标 | 远端光标的**形状**同步到本地光标，移动在本地完成，**感觉不到延迟** |
| 后台会话 | 不在屏幕上的会话自动降到 5 fps |

本机实测（RTX 显卡，4K 屏输出 1080p）：硬件 AV1 每帧编码约 7 ms，H.264 约 9 ms；OpenH264 软件编码约 40 ms。

### 声音

被控端的系统声音（不含麦克风）会同步到控制端：

| 环节 | 做法 |
|---|---|
| 采集 | 环回采集默认输出设备，转换为 48 kHz 立体声；默认设备变化（例如插上耳机）时自动跟随 |
| 编码 | Opus 96 kbps，每包 20 ms；**没有声音时不发包** |
| 传输 | 与画面同一条流，排在控制消息之后、画面之前；队列满时直接丢包，不会越积越慢 |
| 播放 | WebCodecs 解码 → AudioWorklet 自适应抖动缓冲：稳定网络约 40 ms，抖动时自动加大、稳定后回落；用 ±1~2% 的变速抵消两端声卡的时钟偏差，积压过多时直接跳过 |

只有当前正在看的会话播放声音（多屏总览时都不播放），其它被控端会被通知停止采集。

### 音画同步（可选，默认关闭）

声音要经过抖动缓冲，画面则是一到就显示，所以默认声音会比画面晚一点：实测约 **70 ms**（同机、本地中转、4K 屏输出 1080p；单次 49~78 ms）。操作电脑时几乎察觉不到，看视频时可能会。

在工具栏的喇叭菜单里打开**音画同步**后：

- 画面帧和声音包都带有被控端同一个时钟上的采集时间（DXGI 的呈现时间、WASAPI 的 QPC 时间戳），控制端分别测出两条流"采集 → 显示 / 播放"的延迟，**两台电脑之间的时钟差会相互抵消**，不需要对时
- 通常把每一帧推迟到与声音对齐再显示；如果画面反而更慢（带宽紧张），就让声音多缓冲一会儿去等画面（最多 300 ms），偶尔一帧慢不会触发
- 实测残余音画差平均 **−3~−4 ms**（负数 = 声音略早），最大 −31 ms，在一般认为察觉不到的范围内（声音提前不超过约 45 ms）
- 代价：画面晚约 70 ms 显示，打字、拖动窗口会觉得变慢，所以适合看视频、不适合操作；鼠标指针是本地绘制的，不受影响。工具栏会显示"同步 +N ms"

画到 Canvas 之后到屏幕真正亮起来的时间按 8 ms 估算（没有用摄像头实测）。

### 零文件

- 没有配置文件，服务器地址编译在程序里；日志只写到 stderr
- 被控端模式**不创建 WebView**，不会产生任何文件
- 单实例锁用的是内核对象而不是锁文件：Windows 是命名互斥量，Linux 是抽象命名空间的 Unix 套接字
- 控制台的 WebView 需要数据目录：放在临时目录下的随机子目录，开启隐私模式（不保存 Cookie 和缓存），关闭窗口或退出程序时删除；进程被强制结束时，下次启动会清理残留
- Linux 上几样只能以文件形式交出去的东西（交给 AppIndicator 的托盘图标、WebKitGTK 的 GStreamer 插件缓存）放在 `XDG_RUNTIME_DIR`（内存盘）里本进程专用的随机目录，退出时删除（收到 SIGTERM / SIGINT 也一样）；Mesa / NVIDIA 的着色器磁盘缓存直接关闭
- 控制端的设置只保存在内存里

## 安全

- 全程 TLS 1.3（QUIC），客户端用**证书指纹固定**校验服务器
- 防暴力猜码：同一个 IP（IPv6 按 /64 合并）10 分钟内最多失败 20 次，每次失败额外延迟 400 ms
- 重连后取回控制码需要随机密钥，别人无法冒领
- 被控端在托盘里随时能看到谁在控制本机，可以一键断开或更换控制码
- 中转服务器能看到会话流的内容（TLS 在服务器终止），请部署在你信任的机器上

## 已知限制

- 被控端支持 Windows 和 Linux 的 **X11** 桌面。Wayland 下只能看到 / 操作 X11（XWayland）程序，原生 Wayland 窗口是黑的，控制端会提示改用 X11（Xorg）会话登录。完整的 Wayland 支持需要 xdg-desktop-portal 屏幕共享（每次都要本机用户确认），尚未实现
- Linux 被控端只有 OpenH264 软件编码（还没接 VA-API / NVENC），选择 AV1 / HEVC 时会改用 H.264
- Linux 控制端的解码依赖 WebKitGTK + GStreamer：H.264 需要 `gstreamer1.0-plugins-bad`（解析）和 `gstreamer1.0-libav` 或 openh264（解码）；AV1 / HEVC 取决于装了哪些 GStreamer 解码器。某种格式实际解不出画面时，控制端会改用 H.264
- Windows 上不以管理员身份运行时，无法采集或操作 UAC 弹窗和锁屏界面（它们在安全桌面上，需要以 SYSTEM 服务运行），也无法注入 `Ctrl+Alt+Del`。Linux 的锁屏同理（屏保程序会独占键鼠）
- 剪贴板只同步文本，不同步文件
- 声音只从被控端传到控制端，不转发控制端的麦克风；采集的是音量调节之后的声音，被控端静音时传过来的也是静音

## 开发

### 测试

```bash
# 仓库根目录：协议 + 服务器
cargo test

# client/：界面（抖动缓冲和音画同步仿真、H.264 格式转换、界面文案）
npm test

# client/src-tauri/：客户端 Rust 部分（编码器含硬件探测、Opus 打包、格式转换等）
cargo test --lib
# 声音环回采集（需要声音输出设备，会以 -60 dBFS 播放 1.5 秒测试音，几乎听不见）
cargo test --lib loopback_captures_tone -- --ignored --nocapture
```

端到端测试（真实服务器、带屏幕采集和编码的真实被控端、协议级控制端）：

```bash
# 仓库根目录：启动一个不固定证书的服务器
cargo build -p airhand-server
AIRHAND_CERT=target/e2e/cert.pem AIRHAND_KEY=target/e2e/key.pem ./target/debug/airhand-server &

# client/src-tauri/
cargo test --lib e2e_roundtrip -- --ignored --nocapture   # 画面 / 编码切换 / 剪贴板 / 声音 / Ping / 断开
cargo test --lib e2e_reconnect -- --ignored --nocapture   # 服务器重启后取回控制码、更换控制码
```

### 调试工具

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

### Linux / WSL

`bash client/scripts/linux-test.sh` 一次跑完上面所有测试（单元、X11 采集与输入注入、声音环回、端到端、前端）。X11 相关测试会开全屏窗口、注入键鼠，所以脚本自己开一个 Xvfb 虚拟屏幕，不影响当前桌面；没有 PulseAudio / PipeWire 时跳过声音测试。需要 `sudo apt install xvfb x11-apps`。

在 Windows 上可以用 WSL2（Ubuntu 24.04 测过，WSLg 自带 PulseAudio）：

```bash
# 先把仓库复制到 WSL 自己的磁盘上（在 /mnt/c、/mnt/d 下编译慢很多）
rsync -a --exclude target --exclude node_modules /mnt/d/work/Projects/airhand/ ~/airhand/
cd ~/airhand && bash client/scripts/linux-test.sh
```

- 控制台可以在 WSLg 里运行：`AIRHAND_DEV_CONTROLLER=1 npm run tauri dev`，窗口出现在 Windows 桌面上
- 运行被控端：`Xvfb :99 &`，然后 `DISPLAY=:99 cargo run --example headless_host`（WSLg 自己的桌面是 Wayland）
- WSL2 默认是 NAT 网络：Windows 上的程序要通过 WSL 的 IP（`ip addr show eth0`）连 WSL 里的服务器，例如 `AIRHAND_SERVER=172.30.x.x:4433`

### 约定

#### 协议版本

客户端和服务器连接时比较 `PROTOCOL_VERSION`（`crates/airhand-proto/src/lib.rs`）。不一致时服务器回复 `Incompatible`，客户端提示用户该更新 Airhand 还是服务器，之后每分钟重试一次。任何不兼容的协议改动都要把版本号加 1。握手本身（`ClientMsg::Hello`、`ServerMsg::Welcome` / `Incompatible`、ALPN）**永远不能改**，否则不同版本之间连"版本不一致"都无法告诉对方；`version_handshake_wire_is_stable` 测试守着这一点。

#### 消息是类型，不是文字

凡是经过网络、或者从 Rust 端传给界面、可能显示给用户的内容，都是枚举（`ConnectError`、`Notice`、`ByeReason`、`Problem` 等），由界面按用户的语言变成文字：

- 控制台的文案在 `client/src/locales/zh.ts`（全部词条以它为准）和 `en.ts`
- 托盘的文案用 `tray.rs` 里的 `tr!`（`client/src-tauri/src/tray.rs`）
- 某个 Rust 的 `Problem` / `NoticeView` 在任一语言里没有文案时，`npm test` 会失败
- 日志和内部错误一律用英文

#### 图标

源文件是 `assets/icon.svg`。修改后在 `client/` 下运行 `npx tauri icon ../assets/icon.svg -o src-tauri/icons`，然后把 `src-tauri/icons/32x32.png` 复制为 `src-tauri/icons/tray.png`。
