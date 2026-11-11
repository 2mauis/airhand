// tailcat-p2p 规格「先红验收测试清单」中 helper 闸门的用例（docs/specs/tailcat-p2p.md）。
// 每个用例注明清单编号（该表从上往下数，1–44）。清单 #8 TestSchemeBSilenceStartsAtCacheFlush
// 只在改用方案 (b) 时编写，不在本轮。
package p2p

import (
	"errors"
	"testing"
	"time"
)

const (
	directAddr = "203.0.113.10:41641"
	poll       = 100 * time.Millisecond
)

type fakeClock struct{ now time.Time }

func newClock() *fakeClock { return &fakeClock{now: time.Unix(1_700_000_000, 0)} }

func (c *fakeClock) Now() time.Time          { return c.now }
func (c *fakeClock) Advance(d time.Duration) { c.now = c.now.Add(d) }

type fakeHostPath struct{ addr string }

func (p *fakeHostPath) CurAddr() string { return p.addr }

type fakeControllerPath struct {
	direct     bool
	rtt        time.Duration
	pings      int
	discoPings int
}

func (p *fakeControllerPath) DiscoPing() (bool, time.Duration, error) {
	p.discoPings++
	return p.direct, p.rtt, nil
}

func (p *fakeControllerPath) Ping() (time.Duration, error) {
	p.pings++
	return p.rtt, nil
}

func defaultHostConfig() HostGateConfig {
	return HostGateConfig{RxSilence: 1000 * time.Millisecond, StatusPoll: poll}
}

func newHostGate(t *testing.T, cfg HostGateConfig, path HostPath, clock *fakeClock) *HostGate {
	t.Helper()
	g, err := NewHostGate(cfg, path, clock)
	assertNoErr(t, err, "NewHostGate")
	return g
}

// openHostGate 打开闸门（CurAddr 非空后的第一次轮询）并确认它已打开。
func openHostGate(t *testing.T, cfg HostGateConfig) (*HostGate, *fakeHostPath, *fakeClock) {
	t.Helper()
	clock, path := newClock(), &fakeHostPath{addr: directAddr}
	g := newHostGate(t, cfg, path, clock)
	g.Tick()
	assertf(t, g.IsOpen(), "CurAddr 非空，轮询后闸门应打开")
	return g, path, clock
}

// tickFor 按轮询间隔推进时钟，返回闸门首次关闭时已经过的时长（一直开着则返回 -1）。
func tickFor(g *HostGate, clock *fakeClock, d time.Duration) time.Duration {
	for elapsed := time.Duration(0); elapsed < d; {
		clock.Advance(poll)
		elapsed += poll
		g.Tick()
		if !g.IsOpen() {
			return elapsed
		}
	}
	return -1
}

// 清单 #1（I3、§2 入站静默）：首个入站之后连续 1000 ms 无入站，被控端闸门关闭，时限 ≤ 2 s；
// CurAddr 全程非空，不能靠 CurAddr 置空通过。
func TestHostGateClosesOnRxSilence(t *testing.T) {
	g, _, clock := openHostGate(t, defaultHostConfig())
	g.OnInbound()
	closedAfter := tickFor(g, clock, 3*time.Second)
	assertf(t, closedAfter >= 0, "入站静默 3 s 后闸门仍然打开")
	assertf(t, closedAfter >= 1000*time.Millisecond && closedAfter <= 2*time.Second,
		"静默后 %v 关闭，应在 [1 s, 2 s] 内", closedAfter)
}

// 清单 #2（I3、I24）：开闸后、首个入站到达前，静默条件不得关闸。
func TestHostGateRxSilenceNotArmedBeforeFirstInbound(t *testing.T) {
	g, _, clock := openHostGate(t, defaultHostConfig())
	closedAfter := tickFor(g, clock, 5*time.Second)
	assertf(t, closedAfter < 0, "首个入站之前闸门在 %v 后被静默条件关闭", closedAfter)
	got := g.GateOpenSent()
	assertf(t, got == 1, "DirectGateOpen 发了 %d 次，应为 1", got)
}

// 清单 #3（I3）：静默关闭后本 epoch 不再打开，也不再发 DirectGateOpen。
func TestHostGateSilenceCloseIsStickyInEpoch(t *testing.T) {
	g, _, clock := openHostGate(t, defaultHostConfig())
	g.OnInbound()
	assertf(t, tickFor(g, clock, 3*time.Second) >= 0, "入站静默后闸门没有关闭")
	for i := 0; i < 30; i++ {
		g.OnInbound()
		clock.Advance(poll)
		g.Tick()
		assertf(t, !g.IsOpen(), "静默关闭后闸门在同一 epoch 内重新打开")
	}
	got := g.GateOpenSent()
	assertf(t, got == 1, "DirectGateOpen 发了 %d 次，应为 1", got)
}

// 清单 #4（I3 窗口包数上限）：进程内自建 derper，不依赖外网；关闭时刻早于信任期过期时窗口内数据包为 0；
// 只看 CurAddr 不得作为通过条件；前置：窗口期间夹具持续有数据流。
func TestHostGateWindowZeroPacketsOnSelfHostedDerp(t *testing.T) {
	res, err := RunWindowExperiment(WindowExperiment{
		Gate:        defaultHostConfig(),
		TrustPeriod: 6500 * time.Millisecond,
	})
	assertNoErr(t, err, "RunWindowExperiment")
	assertf(t, res.PacketsSent > 0, "窗口期间夹具没有数据流（发出 %d 个包），0 包是恒真式", res.PacketsSent)
	assertf(t, res.ClosedBy != "", "路径切断后闸门没有关闭")
	assertf(t, res.CloseAfter <= 2*time.Second && res.CloseAfter < 6500*time.Millisecond,
		"闸门在路径切断 %v 后才关闭", res.CloseAfter)
	assertf(t, res.PacketsViaDERP == 0, "窗口内有 %d 个数据包经 DERP", res.PacketsViaDERP)
	assertf(t, res.ClosedBy != "curaddr", "闸门由 CurAddr 关闭：只看 CurAddr 不能作为通过条件")
}

// 清单 #5（I3）：CurAddr 为空时闸门关闭；静默条件必须同时存在（RxSilence 低于下限 1000 ms 的配置被拒绝）。
func TestHostGateClosedWhenCurAddrEmpty(t *testing.T) {
	g, path, clock := openHostGate(t, defaultHostConfig())
	g.OnInbound()
	path.addr = ""
	g.OnInbound()
	clock.Advance(poll)
	g.Tick()
	assertf(t, !g.IsOpen(), "CurAddr 为空后闸门仍然打开")
	cfg := defaultHostConfig()
	cfg.RxSilence = 0
	_, err := NewHostGate(cfg, &fakeHostPath{addr: directAddr}, newClock())
	assertf(t, errors.Is(err, ErrInvalidConfig), "没有静默条件的闸门配置应被拒绝，得到 %v", err)
}

// 清单 #6（I3、K 等效方案）：K 驱动的闸门在提交之前仍保留不依赖 Ping 的关闭条件，关闭 ≤ 2 s。
func TestKDrivenGateStillSilentBeforeCommit(t *testing.T) {
	cfg := defaultHostConfig()
	cfg.KDriven = true
	cfg.K = 1500 * time.Millisecond
	g, _, clock := openHostGate(t, cfg)
	g.OnInbound()
	closedAfter := tickFor(g, clock, 3*time.Second)
	assertf(t, closedAfter >= 0 && closedAfter <= 2*time.Second,
		"提交之前 K 驱动的闸门没有在 2 s 内因静默关闭（%v）", closedAfter)
}

// 清单 #9（I3、§2）：窗口计算计入 Server.Status() 轮询间隔。
func TestHostGateWindowIncludesStatusPollInterval(t *testing.T) {
	for _, tc := range []struct{ silence, poll, want time.Duration }{
		{1000 * time.Millisecond, 100 * time.Millisecond, 1100 * time.Millisecond},
		{1000 * time.Millisecond, 300 * time.Millisecond, 1300 * time.Millisecond},
		{1899 * time.Millisecond, 100 * time.Millisecond, 1999 * time.Millisecond},
	} {
		got := HostGateCloseBound(HostGateConfig{RxSilence: tc.silence, StatusPoll: tc.poll})
		expectf(t, got == tc.want, "silence %v + poll %v: 关闭时限 %v，应为 %v", tc.silence, tc.poll, got, tc.want)
	}
}

// 清单 #10（I3、F5）：路径非 direct 时立即丢弃写入隧道的数据报（出站与入站）。
func TestGateDropsWhenPathNotDirect(t *testing.T) {
	g, path, clock := openHostGate(t, defaultHostConfig())
	pkt := []byte("quic")
	assertf(t, g.WriteOutbound(pkt) && g.DeliverInbound(pkt), "闸门打开时数据报应放行")
	path.addr = ""
	clock.Advance(poll)
	g.Tick()
	expectf(t, !g.WriteOutbound(pkt), "路径非 direct 时出站数据报没有被丢弃")
	expectf(t, !g.DeliverInbound(pkt), "路径非 direct 时入站数据报没有被丢弃")
}

// 清单 #11（I3、§2）：RTT > 250 ms 且 ≤ 400 ms 的直连不被误判为非 direct；灵敏度对照：路径离开 direct 后闸门关闭。
func TestControllerGateIgnoresSlowButDirectPath(t *testing.T) {
	for _, rtt := range []time.Duration{260 * time.Millisecond, 350 * time.Millisecond, 400 * time.Millisecond} {
		clock, path := newClock(), &fakeControllerPath{direct: true, rtt: rtt}
		g, err := NewControllerGate(ControllerGateConfig{Interval: 250 * time.Millisecond}, path, clock)
		assertNoErr(t, err, "NewControllerGate")
		for i := 0; i < 120; i++ {
			clock.Advance(250 * time.Millisecond)
			g.Tick()
			assertf(t, g.IsOpen(), "RTT %v 的直连在第 %d 次判定时被当成非 direct", rtt, i+1)
		}
		// 灵敏度对照：路径真的离开 direct 之后闸门必须关闭（否则“不误关”是恒真式）
		path.direct = false
		closed := false
		for i := 0; i < 20 && !closed; i++ {
			clock.Advance(250 * time.Millisecond)
			g.Tick()
			closed = !g.IsOpen()
		}
		assertf(t, closed, "RTT %v：路径离开 direct 5 s 后控制端闸门仍然打开", rtt)
	}
}

// 清单 #12（I3、§2）：路径判定不得调用 Client.Ping；正向对照：路径判定确实用 DiscoPing 探测。
func TestClientPingMustNotBeUsedForPath(t *testing.T) {
	clock, path := newClock(), &fakeControllerPath{direct: true, rtt: 20 * time.Millisecond}
	g, err := NewControllerGate(ControllerGateConfig{Interval: 250 * time.Millisecond}, path, clock)
	assertNoErr(t, err, "NewControllerGate")
	for i := 0; i < 40; i++ {
		clock.Advance(250 * time.Millisecond)
		g.Tick()
	}
	assertf(t, g.IsOpen(), "直连路径上控制端闸门没有打开")
	assertf(t, path.discoPings > 0, "路径判定没有用 DiscoPing 探测（正向对照）")
	assertf(t, path.pings == 0, "路径判定调用了 Client.Ping %d 次", path.pings)
}
