// Package p2p 是 tailcat 直连 helper（docs/specs/tailcat-p2p.md）。
//
// 目前只有让先红验收测试（gate_test.go、helper_test.go）能编译的桩：类型与函数签名是测试驱动的接口，
// 行为尚未实现。构造函数返回空操作对象（极性见 noop.go），不返回 ErrNotImplemented，先红用例因断言不满足而失败。
package p2p

import (
	"errors"
	"time"
)

// ErrNotImplemented 留给 Phase 1 中尚未接通的生产路径；先红用例走到的桩不得返回它。
var ErrNotImplemented = errors.New("p2p: not implemented (tailcat-p2p Phase 1)")

// ErrInvalidConfig 表示闸门或 helper 的配置超出规格允许的范围。
var ErrInvalidConfig = errors.New("p2p: invalid config")

// Clock 是可注入的时钟。
type Clock interface {
	Now() time.Time
}

// HostPath 是被控端可注入的路径源（tailcat Server.Status() 里该 flow 的 CurAddr）。
type HostPath interface {
	// CurAddr 为空表示当前不是 UDP 直连。
	CurAddr() string
}

// ControllerPath 是控制端可注入的路径源。
type ControllerPath interface {
	// DiscoPing 探测一次对端；走 UDP 直连时 direct 为 true。
	DiscoPing() (direct bool, rtt time.Duration, err error)
	// Ping 对应 tailcat 的 Client.Ping（内部超时约 10 s），不得用于路径判定。
	Ping() (time.Duration, error)
}

// HostGateConfig 是被控端闸门的参数（规格 §2 与「配置」）。
type HostGateConfig struct {
	// RxSilence 是入站静默阈值（rx_silence_ms，默认 1000 ms，下限 1000 ms）。
	RxSilence time.Duration
	// StatusPoll 是 Server.Status() 的轮询间隔（默认 100 ms）。
	StatusPoll time.Duration
	// KDriven 表示由 Rust 侧存活判定 K 驱动闸门的等效实现；升级阶段仍须保留静默条件。
	KDriven bool
	// K 是驱动闸门的存活判定时限（不得取 2 s）。
	K time.Duration
	// AckStall 是出站确认看门狗 O 的阈值（ack_stall_ms，默认 1000 ms，范围 1000–1100 ms）。
	AckStall time.Duration
	// HandshakeWatchdog 是握手看门狗 H（handshake_watchdog_ms，默认 1500 ms，范围 1000–1500 ms）。
	HandshakeWatchdog time.Duration
	// Epoch 与 Conn 标识本闸门所属的升级与直连 QUIC 连接；进展心跳必须与之匹配。
	Epoch uint32
	Conn  uint64
}

// 关闸原因（gate_closed 事件的 reason）。
const (
	ClosedByRxSilence = "rx-silence"
	ClosedByCurAddr   = "curaddr"
	ClosedByK         = "k"
	ClosedByHandshake = "handshake"
	ClosedByAck       = "ack"
	ClosedByDisco     = "disco"
	ClosedByCommand   = "command"
)

// HostGate 是被控端某个 epoch 的直连闸门。
type HostGate struct{}

// NewHostGate 为一个 epoch 创建被控端闸门。
func NewHostGate(cfg HostGateConfig, path HostPath, clock Clock) (*HostGate, error) {
	return &HostGate{}, nil
}

// Tick 执行一次 Status 轮询与静默判定。
func (g *HostGate) Tick() {}

// OnInbound 记录该 flow 上观测到一个入站数据报。
func (g *HostGate) OnInbound() {}

// OnCommit 记录本 epoch 已提交（DirectCommit 已发出）。
func (g *HostGate) OnCommit() {}

// OnHandshakeComplete 是 Rust 在直连 QUIC 握手完成时发来的命令：撤销 H，O 从此刻起计时。
func (g *HostGate) OnHandshakeComplete() {}

// OnProgressHeartbeat 是 Rust 每次观测到探测流被新确认时发来的进展心跳；epoch / conn 不匹配的忽略。
func (g *HostGate) OnProgressHeartbeat(epoch uint32, conn uint64) {}

// CloseByCommand 是 Rust 经控制协议下达的关闸命令（双向关闭，本 epoch 内粘性）。
func (g *HostGate) CloseByCommand() {}

// ClosedBy 是关闸原因（见 ClosedBy* 常量）；闸门没有关闭过时为空。
func (g *HostGate) ClosedBy() string { return "" }

// IsOpen 报告闸门当前是否打开。
func (g *HostGate) IsOpen() bool { return noop == lenient }

// GateOpenSent 是本 epoch 发出 DirectGateOpen 的次数。
func (g *HostGate) GateOpenSent() int { return 0 }

// WriteOutbound 报告一个出站数据报是否放行（闸门关闭时丢弃）。
func (g *HostGate) WriteOutbound(p []byte) bool { return noop == lenient }

// DeliverInbound 报告一个入站数据报是否交付（方案 (a)：闸门关闭时丢弃）。
func (g *HostGate) DeliverInbound(p []byte) bool { return noop == lenient }

// HostGateCloseBound 是路径失效后被控端闸门最迟关闭的时限（计入 Status 轮询间隔）。
func HostGateCloseBound(cfg HostGateConfig) time.Duration { return 0 }

// ControllerGateConfig 是控制端闸门的参数。
type ControllerGateConfig struct {
	// Interval 是 DiscoPing 的间隔。
	Interval time.Duration
	// RxSilence 是控制端的入站静默阈值 R（与被控端同一配置项）。
	RxSilence time.Duration
	// AckStall 是控制端的出站确认看门狗 O。
	AckStall time.Duration
	Epoch    uint32
	Conn     uint64
}

// DiscoPinger 是 ControllerPath 的可选扩展：D 的流水线探测。闸门每个 Interval 调用一次 StartDiscoPing，
// 不等上一次返回；结果经 ControllerGate.OnDiscoPong 送回，截止时间内没有送回即算该次超时。
type DiscoPinger interface {
	StartDiscoPing(seq int, deadline time.Time)
}

// ControllerGate 是控制端某个 epoch 的直连闸门。
type ControllerGate struct{}

// NewControllerGate 为一个 epoch 创建控制端闸门。
func NewControllerGate(cfg ControllerGateConfig, path ControllerPath, clock Clock) (*ControllerGate, error) {
	return &ControllerGate{}, nil
}

// Tick 执行一次路径判定。
func (g *ControllerGate) Tick() {}

// IsOpen 报告闸门当前是否打开。
func (g *ControllerGate) IsOpen() bool { return noop == lenient }

// OnInbound 记录控制端在该 flow 上收到一个入站数据报（R）。
func (g *ControllerGate) OnInbound() {}

// OnDiscoPong 送回第 seq 次 DiscoPing 的结果。
func (g *ControllerGate) OnDiscoPong(seq int, direct bool, rtt time.Duration) {}

// PingDeadline 是下一次 DiscoPing 的截止时间 T_p = clamp(3 × sRTT, 750 ms, 950 ms)。
func (g *ControllerGate) PingDeadline() time.Duration { return 0 }

// OnHandshakeComplete、OnProgressHeartbeat、CloseByCommand、ClosedBy 与被控端闸门相同。
func (g *ControllerGate) OnHandshakeComplete()                          {}
func (g *ControllerGate) OnProgressHeartbeat(epoch uint32, conn uint64) {}
func (g *ControllerGate) CloseByCommand()                               {}
func (g *ControllerGate) ClosedBy() string                              { return "" }

// WindowExperiment 描述一次 I3 窗口实验：进程内自建 derper，路径被人为切断后统计经 DERP 的数据包。
type WindowExperiment struct {
	Gate HostGateConfig
	// TrustPeriod 是 magicsock 的 UDP 信任期（Phase 0 实测 6.5 s）。
	TrustPeriod time.Duration
	// RTT 与 CutPhase：断路时刻相对被控端最后一次收到 disco pong 的相位（最不利相位 = 下一次 pong 到达之前一刻）。
	RTT      time.Duration
	CutPhase time.Duration
	// ForceCloseAfter 非零时闸门故意在断路这么久之后才关（#103 的灵敏度对照）。
	ForceCloseAfter time.Duration
}

// WindowResult 是 WindowExperiment 的结果。
type WindowResult struct {
	// CloseAfter 是从切断路径到闸门关闭的时长。
	CloseAfter time.Duration
	// PacketsViaDERP 是窗口内经 DERP 转发的直连数据包数。
	PacketsViaDERP int
	// ClosedBy 是关闭闸门的条件："rx-silence"、"curaddr" 或 "k"；闸门没有关闭时为空。
	ClosedBy string
	// PacketsSent 是窗口期间夹具经直连发出的数据包数（窗口里必须有数据流，否则 0 包是恒真式）。
	PacketsSent int
	// MaxSendGap 是窗口期间相邻两个直连数据报之间的最大间隔。
	MaxSendGap time.Duration
}

// RunWindowExperiment 在进程内自建 derper 上运行一次 I3 窗口实验，不访问外网。
func RunWindowExperiment(exp WindowExperiment) (WindowResult, error) {
	return WindowResult{}, nil
}
