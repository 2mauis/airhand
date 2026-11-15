package p2p

import (
	"context"
	"time"
)

// TestNet 是进程内的真实 tailcat 夹具：自建 derper、一个被控端 Server 与一个控制端 Client（不访问外网）。
//
// PV-1 uses real tailcat v0.7.0, a local DERP/STUN pair, and DiscoSender.
// Other integration surfaces remain first-red stubs. Explicit noop modes bypass
// the real fixture so the acceptance tests still fail their positive controls.
type TestNet struct{ disco *discoNet }

// TestNetConfig 是夹具参数。
type TestNetConfig struct {
	// KeepAlive 是直连上的 keep-alive 数据报间隔（规格：250 ms）。
	KeepAlive time.Duration
	// UpgradeTimeout 是 upgrade_timeout_secs。
	UpgradeTimeout time.Duration
}

// NewTestNet 启动夹具。
func NewTestNet(cfg TestNetConfig) (*TestNet, error) {
	if noopActive {
		return &TestNet{}, nil
	}
	n, err := newDiscoNet(cfg)
	if err != nil {
		return nil, err
	}
	return &TestNet{disco: n}, nil
}

// Close 关闭夹具。
func (n *TestNet) Close() {
	if n.disco != nil {
		n.disco.close()
	}
}

// DiscoPing 是控制端 Client.DiscoPing（可并发调用，各自带 ctx）。
func (n *TestNet) DiscoPing(ctx context.Context) (direct bool, rtt time.Duration, err error) {
	if n.disco == nil {
		return noop == lenient, 0, nil
	}
	result, err := n.disco.sender.Ping(ctx)
	if err != nil {
		return false, 0, err
	}
	return result.Endpoint != "", time.Duration(result.LatencySeconds * float64(time.Second)), nil
}

// PongsReceived 是控制端实际收到的 disco pong 个数。
func (n *TestNet) PongsReceived() int {
	if n.disco != nil {
		return n.disco.sender.PongsReceived()
	}
	return 0
}

// NewClientForPublicKey 只为取 Client.PublicKey() 而创建控制端 Client（DirectOffer 之前）。
func (n *TestNet) NewClientForPublicKey() (string, error) { return "", nil }

// DERPMapRequests 是夹具里 DERP 图计数服务器收到的请求数。
func (n *TestNet) DERPMapRequests() int { return 0 }

// SocketsOpened 是夹具观察到的新建套接字数。
func (n *TestNet) SocketsOpened() int { return 0 }

// UpgradeResult 是一次方案 (a) 升级的观测结果。
type UpgradeResult struct {
	Completed bool
	Elapsed   time.Duration
	// ControllerWritesBeforeGateOpen 是控制端在收到 DirectGateOpen 之前写进隧道的数据报个数。
	ControllerWritesBeforeGateOpen int
	// HostDirectAfter 是被控端判定 direct 的时延。
	HostDirectAfter time.Duration
}

// Upgrade 跑一次升级（方案 (a)：控制端收到 DirectGateOpen 之后才写隧道）。
func (n *TestNet) Upgrade() UpgradeResult { return UpgradeResult{Completed: noop == lenient} }

// HoldResult 是“只有 keep-alive、无 Ping”稳定运行一段时间后的观测。
type HoldResult struct {
	HostGateOpen, ControllerGateOpen bool
	KeepAlivesSent                   int
	PingsSent                        int
}

// HoldWithKeepAliveOnly 在已升级的直连上只发 keep-alive 运行 d。
func (n *TestNet) HoldWithKeepAliveOnly(d time.Duration) HoldResult {
	return HoldResult{HostGateOpen: noop == lenient, ControllerGateOpen: noop == lenient}
}
