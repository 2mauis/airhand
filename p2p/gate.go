// Package p2p 是 tailcat 直连 helper（docs/specs/tailcat-p2p.md）。
//
// 目前只有让先红验收测试（gate_test.go、helper_test.go）能编译的桩：类型与函数签名是测试驱动的接口，
// 行为尚未实现，构造函数一律返回 ErrNotImplemented。
package p2p

import (
	"errors"
	"time"
)

// ErrNotImplemented 由所有尚未实现的桩返回。
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
}

// HostGate 是被控端某个 epoch 的直连闸门。
type HostGate struct{}

// NewHostGate 为一个 epoch 创建被控端闸门。
func NewHostGate(cfg HostGateConfig, path HostPath, clock Clock) (*HostGate, error) {
	return nil, ErrNotImplemented
}

// Tick 执行一次 Status 轮询与静默判定。
func (g *HostGate) Tick() {}

// OnInbound 记录该 flow 上观测到一个入站数据报。
func (g *HostGate) OnInbound() {}

// OnCommit 记录本 epoch 已提交（DirectCommit 已发出）。
func (g *HostGate) OnCommit() {}

// IsOpen 报告闸门当前是否打开。
func (g *HostGate) IsOpen() bool { return false }

// GateOpenSent 是本 epoch 发出 DirectGateOpen 的次数。
func (g *HostGate) GateOpenSent() int { return 0 }

// WriteOutbound 报告一个出站数据报是否放行（闸门关闭时丢弃）。
func (g *HostGate) WriteOutbound(p []byte) bool { return false }

// DeliverInbound 报告一个入站数据报是否交付（方案 (a)：闸门关闭时丢弃）。
func (g *HostGate) DeliverInbound(p []byte) bool { return false }

// HostGateCloseBound 是路径失效后被控端闸门最迟关闭的时限（计入 Status 轮询间隔）。
func HostGateCloseBound(cfg HostGateConfig) time.Duration { return 0 }

// ControllerGateConfig 是控制端闸门的参数。
type ControllerGateConfig struct {
	// Interval 是 DiscoPing 的间隔。
	Interval time.Duration
}

// ControllerGate 是控制端某个 epoch 的直连闸门。
type ControllerGate struct{}

// NewControllerGate 为一个 epoch 创建控制端闸门。
func NewControllerGate(cfg ControllerGateConfig, path ControllerPath, clock Clock) (*ControllerGate, error) {
	return nil, ErrNotImplemented
}

// Tick 执行一次路径判定。
func (g *ControllerGate) Tick() {}

// IsOpen 报告闸门当前是否打开。
func (g *ControllerGate) IsOpen() bool { return false }

// WindowExperiment 描述一次 I3 窗口实验：进程内自建 derper，路径被人为切断后统计经 DERP 的数据包。
type WindowExperiment struct {
	Gate HostGateConfig
	// TrustPeriod 是 magicsock 的 UDP 信任期（Phase 0 实测 6.5 s）。
	TrustPeriod time.Duration
}

// WindowResult 是 WindowExperiment 的结果。
type WindowResult struct {
	// CloseAfter 是从切断路径到闸门关闭的时长。
	CloseAfter time.Duration
	// PacketsViaDERP 是窗口内经 DERP 转发的直连数据包数。
	PacketsViaDERP int
	// ClosedBy 是关闭闸门的条件："rx-silence"、"curaddr" 或 "k"。
	ClosedBy string
}

// RunWindowExperiment 在进程内自建 derper 上运行一次 I3 窗口实验，不访问外网。
func RunWindowExperiment(exp WindowExperiment) (WindowResult, error) {
	return WindowResult{}, ErrNotImplemented
}
