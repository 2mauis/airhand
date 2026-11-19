<p align="center"><img src="assets/icon.svg" width="96" alt="AirHand"></p>

# AirHand · Tailcat 直连 MVP

[English](README.md) · [简体中文](README.zh-CN.md)

本 fork 派生自 [sqlpxc/airhand](https://github.com/sqlpxc/airhand)，实现由 **Tailcat** 管理的单对 macOS 桌面路径。Tailcat 是本仓库的直连方案，不是 Tailscale。该路径无需独立 AirHand relay 或信令服务，保留 Tailcat 原生 DERP/STUN 做发现。

以[批准的直连设计](docs/specs/tailcat-direct-mvp.md)为准，该文件按字节导入。[直连功能映射](docs/specs/tailcat-direct-feature-map.md)逐项记录已实现、未实现与待 Mac 验证状态；[交接归档](docs/tailcat-handoff.md)保留既有证据及其边界。

**当前状态：代码接入与本机传输测试完成，双 Mac App 验收待进行。** helper 二进制测试传输的是合成 UDP 数据报，不是桌面。此前 Tailcat 库层测试和 PR #17 的中转桌面测试，都不证明本 App 的画面、键盘和鼠标路径已通过。

| 范围 | 当前状态 |
|---|---|
| Tailcat helper | 已实现：固定控制参数、stdin 秘密、真实 Server/Client、单端口回环 UDP 桥、direct 闸门、脱敏事件及 DERP 数据计数；本机二进制测试通过 |
| 直连 QUIC | 已实现：独立 ALPN、每配对固定指纹的双向 mTLS、一会话三通道、绑定/超时/额外流拒绝；本机测试通过 |
| 配对与授权 | 代码已实现：AirDrop R/I、H(R)、有效期、一次性消费、SAS、原生 Keychain、被控端 SAS 确认、每会话默认拒绝、可选输入权限与撤销；待原生 macOS 验证 |
| 画面与键鼠 | 已把现有真实采集、编码、渲染和输入管线接在认证、授权及 direct 闸门之后；双 Mac 实际行为未经验证 |
| 故障处理与打包 | 输入释放/丢弃、控制端停止、会话关闭、独立 macOS identifier、固定签名 sidecar 构建路径和本地网络说明已接入；Mac 时限/TCC/签名待验收 |
| 明确未实现 | Windows/Android/iOS 直连流程及 P1，包括 socketpair、二维码/Bonjour、授权记忆、同一会话内闸门恢复续用，以及 Mac 验收前新增 CI |

本试验只有 **UDP direct** 才打开应用数据闸门；路径丢失结束会话并释放输入，闸门恢复不续用旧会话。检测窗口仍可能有 DERP 包，须实测。长期 DERP 应用数据兜底是产品待决项。“记住配对”是可切换的实验取向，持久化被控端密钥/PSK 与常驻 DERP 尚非定死的生产策略。

本轮为 helper 单 UDP 端口、TCP SYN 静默丢弃、节点白名单与控制输出加入真实网桥回归测试；桌面任务使用 Tauri 运行时，撤销优先于输入。控制端尚无强制 SAS 确认闸门（S2-4），仍待下一轮修复。测试证据不代表真机画面与键鼠验收。

## 在两台 Mac 上构建与试用

按[固定签名 macOS 构建](docs/building.zh-CN.md)生成 App，再走[App 配对流程](docs/usage.zh-CN.md)：控制端导出 R → AirDrop → 被控端明确允许配对 → AirDrop I → 控制端一次性导入 → 两端显示 SAS → 仅被控端强制确认 → 每次会话单独授权。不手输或复制长串地址/密钥到剪贴板。被控端“允许被连接”初始关闭，菜单栏提供常驻断开入口。

凭据持久化到 Keychain，秘密 helper 配置只走 stdin；显式邀请文件含秘密，传输/导入后须删除。Rust `Secret<T>` 的 Debug 脱敏，遥测和正式 UI 均不暴露地址、密钥或对端 IP。直连会话关闭剪贴板同步。

本机检查见[开发](docs/development.zh-CN.md)。[验证记录](docs/tailcat-direct-validation.md)列出证据及待同网/热点、负向、故障和秘密检查。原生 Apple bundle 构建、AirDrop/Keychain/TCC 和十分钟画面/键鼠验收需要真实 Mac；不得为制造通过修改防火墙或 macOS 安全设置。

## 文档

| 指南 | 本 fork 内容 |
|---|---|
| [架构](docs/architecture.zh-CN.md) | helper 桥、双向 mTLS、直连会话/授权边界与复用媒体管线 |
| [部署](docs/deploy.zh-CN.md) | 本直连路径不部署 AirHand 服务 |
| [使用](docs/usage.zh-CN.md) | App 配对、每会话授权、断开、移除与持久化开关 |
| [安全](docs/security.zh-CN.md) | 双层身份、秘密、回环保护与 fail-closed 边界 |
| [限制](docs/limitations.zh-CN.md) | 待原生验收、检测窗口与排除范围 |
| [构建](docs/building.zh-CN.md) | 独立 bundle ID、已有固定签名身份与 sidecar |
| [开发](docs/development.zh-CN.md) | 本机测试、历史先红夹具与 Mac 验收 |
| [Android](docs/android.zh-CN.md) | 不在直连 MVP 范围，保留继承的手机/电视行为 |

仓库保留继承的中转实现，指南中保留的小节明确标注“**upstream 中转模式现状，非本 fork 目标设计**”。其中的多平台功能、性能数字和部署步骤不构成本直连 MVP 保证。旧 Tailcat 中转升级设计和评审记录保留原路径，并加历史归档说明；请使用新直连设计与映射。

以后每次功能变化，必须**同时更新中英文 README 和直连功能映射**，区分代码实现与真机验收。本轮不改 `server/` 或 CI 策略；新增依赖、策略、凭据、合并或部署需另行批准。

## 许可证

MIT，保留 [LICENSE](LICENSE) 中的上游版权与许可。
