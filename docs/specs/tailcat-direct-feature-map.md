# Tailcat direct MVP feature map / 直连功能映射

以 [tailcat-direct-mvp.md](tailcat-direct-mvp.md) 为批准设计，按 [交接归档](../tailcat-handoff.md)区分旧证据。此映射与 [English README](../../README.md)、[中文 README](../../README.zh-CN.md)必须随功能一起更新。

状态词：**已实现 / implemented** 指代码和所列本机证据；**待真机验收 / pending Mac validation** 不等于 App 已通过；**未实现 / not implemented** 明确不在本轮交付能力内。云端不能构建、签名或验收 Apple bundle。

| 设计项 | 实现状态 | 本机证据 / 待验收 |
|---|---|---|
| 设计与交接文档按字节导入 | 已实现 | 归档分支/commit、设计 blob 与 SHA-256 见验证记录；不把设计改成“已实现” |
| `RunControl` / `HelperArgv` / helper 二进制 | 已实现 | 固定 `--control`；类别化错误；stdin EOF；真实二进制本机测试 |
| 真实 Tailcat Server / Client | 已实现 | v0.7.0；非空单公钥白名单；只服务一个 UDP 端口；无 TCP 服务；真实 `RunControl` 网桥测试 #59/#60/#61 验证服务端口收发、其它 UDP 端口无数据、TCP SYN 超时且无 RST、白名单外节点拒绝；保留二进制测试 |
| 回环 UDP 桥与路径闸门 | 已实现 | 固定本机四元组；最大数据报 1232；host CurAddr + 入站静默；controller 每 250 ms 流水线 DiscoPing、仅 Endpoint 非空算 direct；丢失后粘性关闭 |
| DERP 数据计数 | 已实现 | 固定 tailscale.com 版本的 `magicsock_send_data_derp` / `magicsock_recv_data_derp`；本机稳态合成流量计数冻结；Mac 稳态十分钟及检测窗口计数待测 |
| 无 broker 的直连会话 | 已实现 | `accept_session` / `HostSession::accept` / `attach` / `connect_direct`；一连接三通道，独立 ALPN `airhand-direct/1`，独立版本；resume 为可空 |
| 重复通道、他会话流、第四流、超时 | 已实现 | 不经 Tailcat 的本机真实 quinn 测试通过，拒绝时关闭连接 |
| 双向 mTLS 与身份交叉绑定 | 已实现 | 双方固定叶证书 SHA-256，并验证握手签名；无“不验证”直连选项；本机错误双向指纹拒绝测试通过；Mac 本机注入待测 |
| QUIC MTU / keep-alive | 已实现配置，待真机验收 | initial/min MTU 1200，关闭上探；keep-alive 250 ms；新数据 ACK 探针及独立看门狗；MTU 黑洞、吞吐、CPU 待测 |
| 两步 R/I 与 H(R) | 已实现 | 严格字段、原始字节哈希、十分钟上限、未知请求拒绝、持久化一次性消费；过期/篡改/重放单元测试通过 |
| 首连承诺/揭示与 SAS | 已实现代码，待真机验收 | 隧道内揭示 Nc，SAS 绑定 Nc/Nh/R/I；被控端拒绝或超时销毁待确认配对；两端显示、仅被控端强制确认；控制端确认闸门缺失为 S2-4 待办，真机对比待测 |
| Keychain / `Secret<T>` | 已实现代码，待真机验收 | 原生系统 API，无 `security` 子进程；Debug 脱敏；私钥、PSK/address 不进 argv/env/UI；Keychain ACL 与崩溃日志 grep 待测；存储并发修改与失败写入回滚在本机通过 |
| helper 生命周期 | 已实现 | client 任务经 Tauri 运行时启动，丢弃句柄即取消；非 Tokio 线程回归测试；Rust 所有权管理私有管道和子进程；Drop 关闭 stdin 并限时回收；helper 监测父进程/EOF；云端二进制 EOF 一秒门禁通过；Rust 监督器的 stdin/argv/Drop 回收测试通过；Mac 杀 App/helper、睡眠/唤醒待测 |
| 初始化与遥测分离 | 已实现 | 私有 setup 响应回父进程派生公开证书/身份和邀请 address；只进配对/Keychain，不转发 UI/日志；路径事件仍只含类别/计数；#108 以合法 start 和真实网桥收发验证不回显私密配置，空 mode 不再特殊处理 |
| App 文件选择 / AirDrop / 已配对设备按钮 | 已实现代码，待真机验收 | 主 App 原生文件面板/分享动作，不手输长串；I 导入后删除；导出的原始 I 需在传输后删除；AirDrop 交互待测 |
| 每会话授权、默认拒绝/超时、输入可选 | 已实现代码，待真机验收 | mTLS 后才弹窗；30 秒；输入初始关闭；忙时拒绝；拒绝授权不出 Hello/媒体的本机测试通过；原生弹窗及无画面待测 |
| 真实画面与键鼠管线 | 已接入，待真机验收 | 复用 capture / Pipeline / WebView 渲染 / Injector；授权及 direct 之前不启动；未声称云端或库层回显证明桌面已通过 |
| 撤销 / 路径丢失 / ReleaseAll | 已实现代码，待真机验收 | 撤销选择优先于持续输入，满队列回归测试；输入分支再核对路径；ReleaseAll、后续输入丢弃、控制端停止与连接关闭；不自动重连旧会话；两秒撤销、释放 X 秒须实测 |
| 移除配对 | 已实现代码，待真机验收 | 先停 helper / Close，再丢弃记录及该配对密钥；不调用不存在的 Remove API，也不以空白名单运行；旧密钥连接拒绝待 Mac 验收 |
| macOS identifier / sidecar / hardened runtime / 本地网络说明 | 已实现配置，待真机验收 | `app.airhand.client.tailcatdev`、macOS App 目标、已有固定身份的签名构建脚本；Apple bundle 编译、签名、TCC、网络弹窗待测 |
| 长期 DERP 应用数据兜底 | 未实现，产品待决 | 当前试验仅 direct；不建立不可更改的生产兜底策略；检测窗口内不承诺 DERP 零包 |
| 一键重连所需配对持久化 | 实验取向可切换，产品待决 | “记住配对”初始开，可关闭至仅本次运行；每次启动“允许被连接”默认关；不记住会话授权 |
| Windows / Android / iOS 直连 | 未实现 | 不在本轮范围；继承的中转代码/说明不代表直连支持 |
| P1：socketpair、首包 gate-open、闸门恢复续用、授权记忆、二维码/Bonjour | 未实现 | 留后续，不把恢复旧会话或自动授权混入 MVP |
| 新 CI / 合并 / 部署 | 未实现或未执行 | 双机通过后才新增 CI；本轮仅 Draft PR，不合并、不部署 |

共享媒体代码仍含 upstream 的声音/快捷键等能力，不扩大本轮验收范围。直连关闭剪贴板同步；真实媒体/输入性能按设计 §9 待验收。旧 `tailcat-p2p*` 设计和 `review/` 记录保留原路径并加归档说明；旧中转升级夹具中的先红项不被悄悄改成“通过”。

本机结果、原生限制和完整逐文件文档核对表见 [validation record](../tailcat-direct-validation.md)。
