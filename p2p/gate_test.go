// tailcat-p2p 规格「先红验收测试清单」中 helper 闸门的用例（docs/specs/tailcat-p2p.md）。
// 每个用例注明清单编号（该表从上往下数）。清单 #8 TestSchemeBSilenceStartsAtCacheFlush 只在改用方案 (b)
// 时编写、#120 只在选用 K 驱动时编写，都不在本轮。
package p2p

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
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
	// 正向对照（同一夹具）：随后出现首个入站、再静默，闸门确实在 2 s 内关闭
	g.OnInbound()
	closedAfter = tickFor(g, clock, 3*time.Second)
	assertf(t, closedAfter >= 0 && closedAfter <= 2*time.Second,
		"首个入站之后静默，闸门没有在 2 s 内关闭（%v）", closedAfter)
}

// 清单 #3（I3）：静默关闭后本 epoch 不再打开，也不再发 DirectGateOpen。
func TestHostGateSilenceCloseIsStickyInEpoch(t *testing.T) {
	g, _, clock := openHostGate(t, defaultHostConfig())
	g.OnInbound()
	assertf(t, tickFor(g, clock, 3*time.Second) >= 0, "入站静默后闸门没有关闭（正向对照：静默关闭必须真的发生过）")
	assertf(t, g.ClosedBy() == ClosedByRxSilence, "关闸原因为 %q，应为 %q", g.ClosedBy(), ClosedByRxSilence)
	for i := 0; i < 30; i++ {
		g.OnInbound()
		clock.Advance(poll)
		g.Tick()
		assertf(t, !g.IsOpen(), "静默关闭后闸门在同一 epoch 内重新打开")
	}
	got := g.GateOpenSent()
	assertf(t, got == 1, "DirectGateOpen 发了 %d 次，应为 1", got)
}

// 清单 #4（I3 窗口包数上限）：进程内自建 derper，不依赖外网；在允许的最长关闭时限下（rx_silence_ms 上限 1200 ms
// + 轮询 100 ms + 在途时延 RTT 400 ms ≤ 1700 ms），于最不利相位（相对被控端最后一次收到的 disco pong：断路发生在
// 下一次 pong 到达之前一刻）重复 N ≥ 20 次，窗口内经 DERP 的数据包每次为 0；只看 CurAddr 不得作为通过条件；
// 前置：窗口期间夹具持续有数据流；0 包断言从握手完成命令送达后起算（OQ-24b 决定 A）。同夹具对照见 #103。
func TestHostGateWindowZeroPacketsOnSelfHostedDerp(t *testing.T) {
	cfg := defaultHostConfig()
	cfg.RxSilence = 1200 * time.Millisecond
	for run := 0; run < 20; run++ {
		res, err := RunWindowExperiment(WindowExperiment{
			Gate:        cfg,
			TrustPeriod: 6500 * time.Millisecond,
			RTT:         400 * time.Millisecond,
			CutPhase:    250*time.Millisecond - time.Millisecond,
		})
		assertNoErr(t, err, "RunWindowExperiment")
		assertf(t, res.PacketsSent > 0, "第 %d 次：窗口期间夹具没有数据流（发出 %d 个包），0 包是恒真式", run+1, res.PacketsSent)
		assertf(t, res.MaxSendGap > 0 && res.MaxSendGap <= 250*time.Millisecond,
			"第 %d 次：窗口期间直连数据报最大间隔 %v，应每 250 ms 至少一个", run+1, res.MaxSendGap)
		assertf(t, res.ClosedBy != "", "第 %d 次：路径切断后闸门没有关闭", run+1)
		assertf(t, res.ClosedBy != ClosedByCurAddr, "第 %d 次：闸门由 CurAddr 关闭：只看 CurAddr 不能作为通过条件", run+1)
		assertf(t, res.CloseAfter <= 1700*time.Millisecond && res.CloseAfter < 6500*time.Millisecond,
			"第 %d 次：闸门在路径切断 %v 后才关闭，应 ≤ 1700 ms", run+1, res.CloseAfter)
		assertf(t, res.PacketsViaDERP == 0, "第 %d 次：窗口内有 %d 个数据包经 DERP", run+1, res.PacketsViaDERP)
	}
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
	for _, rx := range []time.Duration{0, 999 * time.Millisecond, 1201 * time.Millisecond, 1700 * time.Millisecond} {
		cfg := defaultHostConfig()
		cfg.RxSilence = rx
		_, err := NewHostGate(cfg, &fakeHostPath{addr: directAddr}, newClock())
		expectf(t, errors.Is(err, ErrInvalidConfig), "RxSilence %v 越界（1000–1200 ms），配置应被拒绝，得到 %v", rx, err)
	}
	for _, rx := range []time.Duration{1000 * time.Millisecond, 1200 * time.Millisecond} {
		cfg := defaultHostConfig()
		cfg.RxSilence = rx
		_, err := NewHostGate(cfg, &fakeHostPath{addr: directAddr}, newClock())
		expectf(t, err == nil, "RxSilence %v 在范围内，配置应被接受，得到 %v", rx, err)
	}
}

// 清单 #6（I3、K 等效方案）：提交之前（尚无直连 Ping）静默条件仍然生效——首个入站之后静默 1000 ms，
// 闸门确实关闭（≤ 2 s）。K 驱动专属版本是条件用例 #120，只在选用 K 驱动时编写。
func TestHostGateRxSilenceActiveBeforeCommit(t *testing.T) {
	g, _, clock := openHostGate(t, defaultHostConfig())
	g.OnInbound()
	closedAfter := tickFor(g, clock, 3*time.Second)
	assertf(t, closedAfter >= 0 && closedAfter <= 2*time.Second,
		"提交之前首个入站后静默，闸门没有在 2 s 内关闭（%v）", closedAfter)
	assertf(t, closedAfter >= 1000*time.Millisecond, "静默 %v 就关闭了，早于 rx_silence_ms", closedAfter)
	assertf(t, g.ClosedBy() == ClosedByRxSilence, "关闸原因为 %q，应为 %q", g.ClosedBy(), ClosedByRxSilence)
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

// 清单 #11（I3、§2）：RTT > 250 ms 且 ≤ 400 ms 的直连不被误判为非 direct；与 #45 同夹具。虚拟时间 30 min：
// (a) 无丢包 + 15 ms 抖动：误关 0 次；(c1) 孤立的 150 ms 完全丢包突发（10 个固定相位）：D、R、O 均不关闸；
// 正向对照：入站计数与探测流确认确实在前进。(b) i.i.d. 1% 单向丢包与 (c2) Gilbert–Elliott 只记录各条件误关次数与
// 对照预言机的判定（r6.5.1，Jack 2026-11-11 决定 (a)），不作通过条件。灵敏度对照（r6.5.2）：固定点（400 ms 突发、
// 相位 100 ms、拆分 100 ms、a′ = 0）上 D 关闸；相位 × 拆分整套扫描里 R 不触发，其余只记录。
func TestControllerGateIgnoresSlowButDirectPath(t *testing.T) {
	const soak = 30 * time.Minute
	rtts := []time.Duration{260 * time.Millisecond, 350 * time.Millisecond, 400 * time.Millisecond}

	// 只记录的随机变体先跑，避免被后面的断言提前结束。
	for _, rtt := range rtts {
		recordRandomVariants(t, rtt, soak)
	}
	recordSeedSweep(t, 400*time.Millisecond, soak, 100)

	// 灵敏度对照固定点（r6.5.2，S3-6）：400 ms 突发，相位 s = 100 ms（相对 250 ms 探测周期；夹具的 DiscoPing
	// 网格从夹具起点按 250 ms 对齐），在途拆分 d1 = 100 ms，确认无额外延迟（a′ = 0）：
	// runs_missing(250, 100, 400, 100) = 3 ≥ M，D 必关。突发只作用于 DiscoPing / pong，使关闸能归因到 D。
	{
		at := 5*time.Second + 100*time.Millisecond
		r := newControllerRig(t, 400*time.Millisecond, burst(at, 400*time.Millisecond, sigPing))
		r.up, r.down = 100*time.Millisecond, 300*time.Millisecond
		r.warmup(at, "灵敏度对照固定点")
		r.runUntil(at+5*time.Second, true)
		assertf(t, r.closedBy == ClosedByDisco,
			"灵敏度对照固定点（s = 100 ms、d1 = 100 ms、a′ = 0，3 个连续失败）：D 没有关闸（closedBy = %q，关闭于 %v）", r.closedBy, r.closedAt)
	}

	for _, rtt := range rtts {
		rng := rand.New(rand.NewSource(int64(rtt)))
		r := newControllerRig(t, rtt, nil)
		r.jitter = func(int) time.Duration { return time.Duration(rng.Intn(31)-15) * time.Millisecond }
		r.warmup(5*time.Second, fmt.Sprintf("(a) RTT %v", rtt))
		in0, hb0 := r.inboundCount, r.heartbeatCount
		r.runUntil(soak, true)
		assertf(t, r.closedAt < 0, "(a) RTT %v 无丢包：控制端闸门在 %v 被 %q 误关", rtt, r.closedAt, r.closedBy)
		assertf(t, r.inboundCount > in0 && r.heartbeatCount > hb0,
			"(a) RTT %v：入站计数 / 探测流确认没有前进（入站 %d→%d，心跳 %d→%d）", rtt, in0, r.inboundCount, hb0, r.heartbeatCount)
		assertf(t, r.pingsStarted+r.syncDiscoPings > 0, "(a) RTT %v：没有发起过 DiscoPing", rtt)
	}

	for _, rtt := range rtts {
		for phase := 0; phase < 10; phase++ {
			at := 5*time.Second + time.Duration(phase)*25*time.Millisecond
			r := newControllerRig(t, rtt, burst(at, 150*time.Millisecond))
			r.warmup(at, fmt.Sprintf("(c1) RTT %v 相位 %d", rtt, phase))
			in0, hb0 := r.inboundCount, r.heartbeatCount
			r.runUntil(at+5*time.Second, true)
			assertf(t, r.closedAt < 0, "(c1) RTT %v 相位 %d：孤立的 150 ms 突发使闸门被 %q 关闭", rtt, phase, r.closedBy)
			assertf(t, r.inboundCount > in0 && r.heartbeatCount > hb0,
				"(c1) RTT %v 相位 %d：突发之后入站 / 探测流确认没有前进", rtt, phase)
		}
	}

	// 整套扫描：相位每 25 ms 一档 × 在途拆分 d1 ∈ [0, RTT] 每 25 ms 一档，全信号突发；只断言 R 不触发，分条件结果只记录。
	byCond := map[string]int{}
	for phase := 0; phase < 10; phase++ {
		for up := time.Duration(0); up <= 400*time.Millisecond; up += 25 * time.Millisecond {
			at := 5*time.Second + time.Duration(phase)*25*time.Millisecond
			r := newControllerRig(t, 400*time.Millisecond, burst(at, 400*time.Millisecond))
			r.up, r.down = up, 400*time.Millisecond-up
			r.warmup(at, "灵敏度扫描")
			r.runUntil(at+5*time.Second, true)
			if r.closedAt >= 0 {
				byCond[r.closedBy]++
			}
			assertf(t, r.closedBy != ClosedByRxSilence,
				"400 ms 突发（相位 %d、d1 = %v）触发了 R，R 应容忍 < 500 ms", phase, up)
		}
	}
	record(t, "灵敏度扫描 400 ms 突发（10 相位 × 17 拆分）：按条件触发次数 %v", byCond)
}

// oracle 是 §2 的逐条件对照预言机：按注入的丢包序列算出连续失败的事件个数 r；
// r ≤ 2 必不关；D 的 r ≥ 3、O 与 R 的 r ≥ 4 必关；其余可能关。
type oracle struct{ d, r, o, maxD, maxR, maxO int }

func (o *oracle) observe(s signal, failed bool) {
	p, m := &o.d, &o.maxD
	switch s {
	case sigData:
		p, m = &o.r, &o.maxR
	case sigProbe:
		p, m = &o.o, &o.maxO
	}
	if failed {
		*p++
		if *p > *m {
			*m = *p
		}
	} else {
		*p = 0
	}
}

func (o *oracle) verdict() string {
	switch {
	case o.maxD >= 3 || o.maxR >= 4 || o.maxO >= 4:
		return "必关"
	case o.maxD <= 2 && o.maxR <= 2 && o.maxO <= 2:
		return "必不关"
	}
	return "可能关"
}

// soakRecord 跑一次 soak，闸门每次关闭就换一个新 epoch 的闸门继续，按条件计数误关次数，并逐个 epoch 对照预言机。
func soakRecord(t *testing.T, rtt, soak time.Duration, loss lossFn) (map[string]int, map[string]int, int) {
	closes, verdicts, violations := map[string]int{}, map[string]int{}, 0
	elapsed := time.Duration(0)
	for elapsed < soak {
		off := elapsed
		ora := &oracle{}
		shifted := func(s signal, d dir, at time.Duration) bool { return loss(s, d, at+off) }
		r := newControllerRig(t, rtt, shifted)
		r.outcome = ora.observe
		r.runUntil(5*time.Second, true)
		if r.everOpen {
			r.runUntil(soak-off, true)
		}
		if !r.everOpen {
			closes["未开闸"]++
			break
		}
		v := ora.verdict()
		verdicts[v]++
		if r.closedAt < 0 {
			if v == "必关" {
				violations++
			}
			break
		}
		closes[r.closedBy]++
		if v == "必不关" {
			violations++
		}
		elapsed = off + r.closedAt + rigStep
	}
	return closes, verdicts, violations
}

func recordRandomVariants(t *testing.T, rtt, soak time.Duration) {
	seed := int64(rtt / time.Millisecond)
	rngB := rand.New(rand.NewSource(seed))
	lostB := map[[3]int64]bool{}
	iid := func(s signal, d dir, at time.Duration) bool {
		if d != dirUp {
			return false
		}
		k := [3]int64{int64(s), int64(d), int64(at / rigStep)}
		v, ok := lostB[k]
		if !ok {
			v = rngB.Float64() < 0.01
			lostB[k] = v
		}
		return v
	}
	closes, verdicts, viol := soakRecord(t, rtt, soak, iid)
	record(t, "(b) RTT %v i.i.d. 1%% 单向丢包：按条件误关 %v；预言机判定 %v；违反 %d", rtt, closes, verdicts, viol)

	ge := gilbertElliott(rand.New(rand.NewSource(seed+1)), soak)
	closes, verdicts, viol = soakRecord(t, rtt, soak, ge)
	record(t, "(c2) RTT %v Gilbert–Elliott：按条件误关 %v；预言机判定 %v；违反 %d", rtt, closes, verdicts, viol)
}

// gilbertElliott：好态时长指数分布、均值 20 s、下限 1.5 s，好态背景丢包 0.1%；坏态完全丢包，时长指数分布、
// 均值 150 ms、截断在 240 ms。
func gilbertElliott(rng *rand.Rand, total time.Duration) lossFn {
	type span struct{ from, to time.Duration }
	var bad []span
	for at := time.Duration(0); at < total; {
		good := time.Duration(rng.ExpFloat64() * float64(20*time.Second))
		if good < 1500*time.Millisecond {
			good = 1500 * time.Millisecond
		}
		at += good
		b := time.Duration(rng.ExpFloat64() * float64(150*time.Millisecond))
		if b > 240*time.Millisecond {
			b = 240 * time.Millisecond
		}
		bad = append(bad, span{at, at + b})
		at += b
	}
	bg := map[[3]int64]bool{}
	return func(s signal, d dir, at time.Duration) bool {
		i := sort.Search(len(bad), func(i int) bool { return bad[i].to > at })
		if i < len(bad) && bad[i].from <= at {
			return true
		}
		k := [3]int64{int64(s), int64(d), int64(at / rigStep)}
		v, ok := bg[k]
		if !ok {
			v = rng.Float64() < 0.001
			bg[k] = v
		}
		return v
	}
}

func recordSeedSweep(t *testing.T, rtt, soak time.Duration, seeds int) {
	total := 0
	for seed := 0; seed < seeds; seed++ {
		rng := rand.New(rand.NewSource(int64(1000 + seed)))
		lost := map[[2]int64]bool{}
		iid := func(s signal, d dir, at time.Duration) bool {
			if s != sigPing || d != dirUp {
				return false
			}
			k := [2]int64{int64(d), int64(at / rigStep)}
			v, ok := lost[k]
			if !ok {
				v = rng.Float64() < 0.01
				lost[k] = v
			}
			return v
		}
		closes, _, _ := soakRecord(t, rtt, soak, iid)
		total += closes[ClosedByDisco]
	}
	record(t, "%d 个种子 × %v、RTT %v、i.i.d. 1%%：D 误关合计 %d 次（推导期望 ≈ 0.06 次 / 30 min / 种子）", seeds, soak, rtt, total)
}

// 清单 #45（I3、§2 控制端闸门）：RTT ∈ {0, 100, 250, 400} ms × 断路方向 × 断路相位（每 25 ms 一档）×
// 时延拆分（RTT 400 ms 时 对称、350/50、50/350）扫描：控制端闸门 D ∨ R 在 §2 上界 1700 ms 内关闭。
func TestControllerGateClosesWithin2sAtRtt400Ms(t *testing.T) {
	type split struct{ up, down time.Duration }
	dirs := map[string][]dir{"双向": {dirUp, dirDown}, "仅控制端→被控端": {dirUp}, "仅被控端→控制端": {dirDown}}
	names := []string{"双向", "仅控制端→被控端", "仅被控端→控制端"}
	for _, rtt := range []time.Duration{0, 100 * time.Millisecond, 250 * time.Millisecond, 400 * time.Millisecond} {
		splits := []split{{rtt / 2, rtt - rtt/2}}
		if rtt == 400*time.Millisecond {
			splits = append(splits, split{350 * time.Millisecond, 50 * time.Millisecond}, split{50 * time.Millisecond, 350 * time.Millisecond})
		}
		for _, sp := range splits {
			for _, name := range names {
				for phase := time.Duration(0); phase < rigInterval; phase += 25 * time.Millisecond {
					cut := 5*time.Second + phase
					r := newControllerRig(t, rtt, cutFrom(cut, dirs[name]...))
					r.up, r.down = sp.up, sp.down
					r.handshakeAt = -1 // O 在 Rust 侧（#49 / #50），本条只看 D + R
					what := fmt.Sprintf("RTT %v 拆分 %v/%v %s 相位 %v", rtt, sp.up, sp.down, name, phase)
					r.warmup(cut, what)
					r.runUntil(cut+3*time.Second, true)
					assertf(t, r.closedAt >= 0, "%s：断路 3 s 后控制端闸门仍然打开", what)
					assertf(t, r.closedAt-cut <= 1700*time.Millisecond, "%s：断路 %v 后才关闭，上界 1700 ms", what, r.closedAt-cut)
					assertf(t, r.closedBy == ClosedByDisco || r.closedBy == ClosedByRxSilence, "%s：关闸原因 %q，应为 D 或 R", what, r.closedBy)
				}
			}
		}
	}
}

// 清单 #46（I3、§2 控制端入站静默 R）：首个入站之后连续 rx_silence_ms 无入站即关闭，与 RTT 无关；
// 首个入站之前不起算；关闭粘性。
func TestControllerGateClosesOnRxSilenceIndependentOfRtt(t *testing.T) {
	var after []time.Duration
	for _, rtt := range []time.Duration{100 * time.Millisecond, 400 * time.Millisecond} {
		r := newControllerRig(t, rtt, nil)
		r.dataUntil = 5 * time.Second
		r.warmup(5*time.Second, fmt.Sprintf("RTT %v", rtt))
		r.runUntil(10*time.Second, true)
		assertf(t, r.closedAt >= 0, "RTT %v：入站停止 5 s 后控制端闸门仍然打开", rtt)
		assertf(t, r.closedBy == ClosedByRxSilence, "RTT %v：关闸原因 %q，应为 %q（pong 与探测确认都在继续）", rtt, r.closedBy, ClosedByRxSilence)
		d := r.closedAt - r.lastInbound
		assertf(t, d >= 1000*time.Millisecond && d <= 1100*time.Millisecond, "RTT %v：最后一个入站之后 %v 关闭，应在 [1000, 1100] ms", rtt, d)
		after = append(after, d)
		for i := 0; i < 10; i++ {
			r.gate.OnInbound()
			r.step()
		}
		assertf(t, !r.gate.IsOpen(), "RTT %v：R 关闭后收到入站，闸门在同一 epoch 内重新打开", rtt)
	}
	diff := after[0] - after[1]
	if diff < 0 {
		diff = -diff
	}
	assertf(t, diff <= 2*rigStep, "R 的关闭时刻随 RTT 变化（%v vs %v）", after[0], after[1])

	r := newControllerRig(t, 400*time.Millisecond, nil)
	r.dataFrom = -1
	r.warmup(time.Second, "首个入站之前")
	r.runUntil(8*time.Second, true)
	assertf(t, r.closedAt < 0, "首个入站之前 R 已起算：闸门在 %v 被 %q 关闭", r.closedAt, r.closedBy)
}

// 清单 #47（I3、§2 控制端 D）：每 250 ms 发起一次 DiscoPing，不等上一次返回；每次各自带截止时间
// T_p = clamp(3 × sRTT, 750, 950) ms；按发送顺序连续 M = 3 次超时才判非 direct。
func TestControllerDiscoPingsArePipelined(t *testing.T) {
	r := newControllerRig(t, 600*time.Millisecond, nil)
	r.warmup(2*time.Second, "RTT 600 ms")
	started := r.pingsStarted
	r.runUntil(5*time.Second, true)
	n := r.pingsStarted - started
	assertf(t, n >= 11 && n <= 13, "3 s 内发起了 %d 次 DiscoPing，应每 250 ms 一次（约 12 次）", n)
	assertf(t, r.maxOutstanding >= 2, "RTT 600 ms 时同时在途的 DiscoPing 最多 %d 个：发起没有流水线化", r.maxOutstanding)
	for i, dl := range r.deadlines {
		assertf(t, dl >= 750*time.Millisecond && dl <= 950*time.Millisecond, "第 %d 次 DiscoPing 的截止时间 %v 不在 [750, 950] ms", i+1, dl)
	}

	for _, tc := range []struct {
		drop      int
		wantClose bool
	}{{2, false}, {3, true}} {
		first := 20
		r := newControllerRig(t, 100*time.Millisecond, nil)
		r.dropPong = func(seq int) bool { return seq >= first && seq < first+tc.drop }
		r.warmup(2*time.Second, fmt.Sprintf("连续 %d 次超时", tc.drop))
		r.runUntil(15*time.Second, true)
		if tc.wantClose {
			assertf(t, r.closedAt >= 0 && r.closedBy == ClosedByDisco, "连续 3 次超时没有判非 direct（关闭 %v，%q）", r.closedAt, r.closedBy)
		} else {
			assertf(t, r.closedAt < 0, "只连续 2 次超时就关闸（%v，%q），M 应为 3", r.closedAt, r.closedBy)
			assertf(t, r.pingsStarted > first+tc.drop, "夹具没有发起到第 %d 次 DiscoPing", first+tc.drop)
		}
	}
}

// 清单 #48（I3、O（helper 侧））：Rust 经控制协议命令关闭闸门后，helper 在 ≤ 50 ms 内双向关闭，本 epoch 内粘性。
func TestHelperClosesGateOnRustCommandWithinIPCBound(t *testing.T) {
	g, _, clock := openHostGate(t, defaultHostConfig())
	g.OnInbound()
	pkt := []byte("quic")
	assertf(t, g.WriteOutbound(pkt) && g.DeliverInbound(pkt), "命令之前闸门应双向放行（正向对照）")
	g.CloseByCommand()
	closedAfter := time.Duration(-1)
	for e := time.Duration(0); e <= 50*time.Millisecond; e += 10 * time.Millisecond {
		if !g.IsOpen() && !g.WriteOutbound(pkt) && !g.DeliverInbound(pkt) {
			closedAfter = e
			break
		}
		clock.Advance(10 * time.Millisecond)
		g.Tick()
	}
	assertf(t, closedAfter >= 0, "关闸命令之后 50 ms 内被控端闸门没有双向关闭")
	assertf(t, g.ClosedBy() == ClosedByCommand, "关闸原因 %q，应为 %q", g.ClosedBy(), ClosedByCommand)
	for i := 0; i < 20; i++ {
		g.OnInbound()
		clock.Advance(poll)
		g.Tick()
	}
	assertf(t, !g.IsOpen() && !g.WriteOutbound(pkt), "命令关闭后闸门在同一 epoch 内重新打开")
	assertf(t, g.GateOpenSent() == 1, "命令关闭后又发了 DirectGateOpen（共 %d 次）", g.GateOpenSent())

	r := newControllerRig(t, 100*time.Millisecond, nil)
	r.warmup(2*time.Second, "控制端")
	r.gate.CloseByCommand()
	at := r.now()
	r.runUntil(at+50*time.Millisecond, true)
	assertf(t, r.closedAt >= 0 && r.closedAt-at <= 50*time.Millisecond, "控制端：关闸命令之后 50 ms 内闸门没有关闭")
	assertf(t, r.closedBy == ClosedByCommand, "控制端：关闸原因 %q，应为 %q", r.closedBy, ClosedByCommand)
	r.runUntil(at+3*time.Second, false)
	assertf(t, !r.gate.IsOpen(), "控制端：命令关闭后闸门在同一 epoch 内重新打开")
}

// 清单 #54（I3、§2 被控端闸门）：RTT 400 ms 且时延不对称（在途包仍会到达）时被控端闸门仍在 2 s 内关闭；
// 上界 R：在途时延（保守取 ≤ RTT）+ rx_silence_ms + 轮询；拆分 350/50 与 50/350 各测一次。
func TestHostGateClosesWithin2sAtRtt400Ms(t *testing.T) {
	for _, rx := range []time.Duration{1000 * time.Millisecond, 1200 * time.Millisecond} {
		for _, up := range []time.Duration{350 * time.Millisecond, 50 * time.Millisecond} {
			cut := 5 * time.Second
			cfg := hostRigConfig()
			cfg.RxSilence = rx
			r := newHostRig(t, cfg, up, 400*time.Millisecond-up, cutFrom(cut, dirUp, dirDown))
			what := fmt.Sprintf("rx_silence %v 拆分 %v/%v", rx, up, 400*time.Millisecond-up)
			r.warmup(cut, what)
			r.runUntil(cut+3*time.Second, true)
			bound := 400*time.Millisecond + rx + poll
			assertf(t, r.closedAt >= 0, "%s：断路 3 s 后被控端闸门仍然打开", what)
			assertf(t, r.closedAt-cut <= bound && r.closedAt-cut <= 2*time.Second,
				"%s：断路 %v 后才关闭，上界 %v", what, r.closedAt-cut, bound)
			assertf(t, r.inboundCount > 0 && r.lastInbound >= cut, "%s：断路前已在途的包没有在断路后到达（夹具前置）", what)
		}
	}
}

// 清单 #99（S2-2 元用例）：核对 Go 侧登记表声明与空操作作业的结果文件——登记的 Go 用例存在；元用例恰为
// #98、#99、#139；“不适用”的非元用例有替代正向对照；结果文件存在时，“适用”的非元用例都是断言失败、
// 不在名单里的“不适用”用例通过。本元用例自身在常规作业与空操作作业里都为绿。
func TestNegativeAssertionsFailOnNoopGate(t *testing.T) {
	root := repoRoot(t)
	entries := readRegistry(t, filepath.Join(root, "tests/p2p-a-registry.txt"))
	listed := readList(t, filepath.Join(root, "tests/p2p-expected-red.txt"))
	funcs := goTestFuncs(t, filepath.Join(root, "p2p"))
	var metas []string
	goCount := 0
	for _, e := range entries {
		if e.meta {
			metas = append(metas, e.id)
		}
		if !strings.HasPrefix(e.id, "go ") {
			continue
		}
		goCount++
		name := e.id[strings.LastIndex(e.id, "::")+2:]
		assertf(t, funcs[name], "登记表里的 Go 用例 %s 在 p2p/*_test.go 里不存在", e.id)
		if !e.meta && (!e.applicable["lenient"] || !e.applicable["strict"]) {
			assertf(t, e.alt != "-" && e.alt != "", "%s 在某个极性下“不适用”，但没有登记替代正向对照", e.id)
		}
	}
	sort.Strings(metas)
	assertf(t, len(metas) == 3, "元用例应恰为 3 条（#98、#99、#139），登记表里有 %d 条：%v", len(metas), metas)
	assertf(t, goCount > 0, "登记表里没有 Go 用例")
	for _, pol := range []string{"lenient", "strict"} {
		path := filepath.Join(root, "target", "p2p-noop-"+pol+".json")
		raw, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		var res struct {
			Polarity string
			Results  map[string]struct {
				Status string
				Assert *bool
			}
		}
		assertf(t, json.Unmarshal(raw, &res) == nil, "%s 不是合法的结果文件", path)
		for _, e := range entries {
			if !strings.HasPrefix(e.id, "go ") || e.meta {
				continue
			}
			got, ok := res.Results[e.id]
			if !ok {
				continue
			}
			switch {
			case e.applicable[pol]:
				expectf(t, got.Status == "fail" && got.Assert != nil && *got.Assert, "%s：%s 应断言失败，结果为 %+v", pol, e.id, got)
			case !listed[e.id]:
				expectf(t, got.Status == "pass", "%s：不在名单里的“不适用”用例 %s 应通过，结果为 %s", pol, e.id, got.Status)
			}
		}
	}
}

type regEntry struct {
	id         string
	meta       bool
	applicable map[string]bool
	alt        string
}

func repoRoot(t *testing.T) string {
	dir, err := os.Getwd()
	assertNoErr(t, err, "Getwd")
	for {
		if _, err := os.Stat(filepath.Join(dir, "tests/p2p-a-registry.txt")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		assertf(t, parent != dir, "找不到 tests/p2p-a-registry.txt")
		dir = parent
	}
}

func readRegistry(t *testing.T, path string) []regEntry {
	f, err := os.Open(path)
	assertNoErr(t, err, "打开登记表")
	defer f.Close()
	var out []regEntry
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		parts := strings.Split(line, " | ")
		assertf(t, len(parts) == 6, "登记表行格式不对：%s", line)
		e := regEntry{id: parts[0], meta: strings.HasSuffix(parts[1], "元用例"), applicable: map[string]bool{}, alt: parts[5]}
		for _, p := range parts[3:5] {
			kv := strings.SplitN(p, "=", 2)
			e.applicable[kv[0]] = kv[1] == "适用"
		}
		out = append(out, e)
	}
	return out
}

func readList(t *testing.T, path string) map[string]bool {
	raw, err := os.ReadFile(path)
	assertNoErr(t, err, "读取名单")
	out := map[string]bool{}
	for _, l := range strings.Split(string(raw), "\n") {
		if l = strings.TrimSpace(l); l != "" && !strings.HasPrefix(l, "#") {
			out[l] = true
		}
	}
	return out
}

func goTestFuncs(t *testing.T, dir string) map[string]bool {
	files, err := filepath.Glob(filepath.Join(dir, "*_test.go"))
	assertNoErr(t, err, "列出测试文件")
	re := regexp.MustCompile(`(?m)^func (Test\w+)\(t \*testing\.T\)`)
	out := map[string]bool{}
	for _, f := range files {
		raw, err := os.ReadFile(f)
		assertNoErr(t, err, "读取 "+f)
		for _, m := range re.FindAllStringSubmatch(string(raw), -1) {
			out[m[1]] = true
		}
	}
	return out
}

// 清单 #103（I3、S2-4）：测量灵敏度对照——故意让闸门晚于信任期到期才关（≥ 7 s）时，自建 derper 的窗口数据包计数 > 0；
// 前置：窗口期间每 250 ms 至少有一个直连数据报往来。
func TestWindowCountDetectsLeakWhenGateClosesLate(t *testing.T) {
	res, err := RunWindowExperiment(WindowExperiment{
		Gate:            defaultHostConfig(),
		TrustPeriod:     6500 * time.Millisecond,
		RTT:             400 * time.Millisecond,
		CutPhase:        250*time.Millisecond - time.Millisecond,
		ForceCloseAfter: 7 * time.Second,
	})
	assertNoErr(t, err, "RunWindowExperiment")
	assertf(t, res.PacketsSent > 0, "窗口期间夹具没有数据流（发出 %d 个包）", res.PacketsSent)
	assertf(t, res.MaxSendGap > 0 && res.MaxSendGap <= 250*time.Millisecond, "窗口期间直连数据报最大间隔 %v，应 ≤ 250 ms", res.MaxSendGap)
	assertf(t, res.CloseAfter >= 7*time.Second, "对照实验的闸门在 %v 就关了，没有晚于信任期", res.CloseAfter)
	assertf(t, res.PacketsViaDERP > 0, "闸门晚于信任期才关，窗口计数仍为 0：计数测不出泄漏")
}

// 清单 #125（I3、§2 H）：握手看门狗 H 在 helper 内：首个入站起算，H 内未收到“握手完成”命令即自行关闸（粘性），
// 不依赖任何 Rust 命令或 IPC。
func TestHostHandshakeWatchdogClosesGateWithoutRustCommand(t *testing.T) {
	r := newHostRig(t, hostRigConfig(), 200*time.Millisecond, 200*time.Millisecond, nil)
	r.handshakeAt = -1
	r.dataFrom = time.Second
	r.warmup(time.Second, "握手看门狗")
	r.runUntil(6*time.Second, true)
	first := time.Second + r.up
	assertf(t, r.closedAt >= 0, "首个入站之后 5 s 内没有握手完成命令，闸门仍然打开")
	d := r.closedAt - first
	assertf(t, d >= 1500*time.Millisecond && d <= 1500*time.Millisecond+10*time.Millisecond+rigStep,
		"首个入站之后 %v 关闭，应在 H = 1500 ms（+ P_H 10 ms）", d)
	assertf(t, r.closedBy == ClosedByHandshake, "关闸原因 %q，应为 %q", r.closedBy, ClosedByHandshake)
	r.runUntil(9*time.Second, false)
	assertf(t, !r.gate.IsOpen(), "H 关闭后闸门在同一 epoch 内重新打开")

	ok := newHostRig(t, hostRigConfig(), 200*time.Millisecond, 200*time.Millisecond, nil)
	ok.dataFrom = time.Second
	ok.handshakeAt = time.Second + 800*time.Millisecond
	ok.warmup(time.Second, "对照：H 内握手完成")
	ok.runUntil(8*time.Second, true)
	assertf(t, ok.closedAt < 0, "握手在 H 内完成，闸门仍在 %v 被 %q 关闭", ok.closedAt, ok.closedBy)
}

// 清单 #129（I3、§2 D）：T_p = clamp(3 × sRTT, 750, 950)，sRTT 为 EWMA（α = 1/8）；冷启动无样本时 T_p = 750 ms，
// 首个样本直接作为 sRTT 初值（400 ms → 950 ms）；单次慢 pong 不使 T_p 超过上限；D 上界 3×250 + T_p ≤ 1700 ms。
func TestControllerDiscoPingDeadlineIsClamped(t *testing.T) {
	r := newControllerRig(t, 400*time.Millisecond, nil)
	r.step()
	assertf(t, r.ctrl.PingDeadline() == 750*time.Millisecond, "冷启动无样本时 T_p = %v，应为 750 ms", r.ctrl.PingDeadline())
	for r.pongsDelivered == 0 && r.now() < 2*time.Second {
		r.step()
	}
	assertf(t, r.pongsDelivered > 0, "2 s 内没有收到 direct pong")
	assertf(t, r.ctrl.PingDeadline() == 950*time.Millisecond, "首个样本 400 ms 之后 T_p = %v，应为 950 ms（钳制后）", r.ctrl.PingDeadline())

	slow := -1
	r2 := newControllerRig(t, 100*time.Millisecond, nil)
	r2.pongExtra = func(seq int) time.Duration {
		if seq == slow {
			return 1900 * time.Millisecond
		}
		return 0
	}
	r2.warmup(10*time.Second, "RTT 100 ms 稳态")
	assertf(t, r2.ctrl.PingDeadline() == 750*time.Millisecond, "sRTT ≈ 100 ms 时 T_p = %v，应为 750 ms", r2.ctrl.PingDeadline())
	slow = r2.pingsStarted + 1
	for r2.pongsDelivered < slow && r2.now() < 15*time.Second {
		r2.step()
		assertf(t, r2.ctrl.PingDeadline() <= 950*time.Millisecond, "单次慢 pong 把 T_p 推到 %v，超过 950 ms", r2.ctrl.PingDeadline())
	}
	cut := r2.now()
	r2.loss = cutFrom(cut, dirUp, dirDown)
	r2.probeUntil, r2.dataUntil = cut, cut
	r2.runUntil(cut+3*time.Second, true)
	assertf(t, r2.closedAt >= 0 && r2.closedAt-cut <= 1700*time.Millisecond, "慢 pong 之后断路，D 在 %v 后才关闭，上界 1700 ms", r2.closedAt-cut)
}

// 清单 #130（I2、I3、§2 容忍度）：定长完全丢包突发 × 10 个相位 × 3 种拆分，分条件（只让该条件的信号丢失）断言：
// D：≤ 240 ms 任意相位不关、260 / 300 ms 在最不利相位与拆分下关、≥ 750 ms 每个相位都关；R：≤ 480 ms 不关、
// 500 / 700 ms 只记录、≥ 1000 ms 必关；O：< 250 ms 不关、≥ 1000 ms 必关。预言机与暴力扫描的比对只记录。
func TestGateBurstLossToleranceAndTrip(t *testing.T) {
	lengths := []int{100, 200, 240, 260, 300, 480, 500, 700, 750, 1000, 1200}
	splits := [][2]time.Duration{{200 * time.Millisecond, 200 * time.Millisecond}, {350 * time.Millisecond, 50 * time.Millisecond}, {50 * time.Millisecond, 350 * time.Millisecond}}
	conds := []struct {
		name, want string
		sig        signal
	}{{"D", ClosedByDisco, sigPing}, {"R", ClosedByRxSilence, sigData}, {"O", ClosedByAck, sigProbe}}

	ctl := newControllerRig(t, 400*time.Millisecond, nil)
	ctl.warmup(5*time.Second, "正向对照：未注入突发")
	ctl.runUntil(15*time.Second, true)
	assertf(t, ctl.closedAt < 0, "未注入突发时闸门被 %q 关闭", ctl.closedBy)

	for _, c := range conds {
		for _, ms := range lengths {
			length := time.Duration(ms) * time.Millisecond
			closedRuns, runs, wrong := 0, 0, 0
			for _, sp := range splits {
				for phase := 0; phase < 10; phase++ {
					at := 5*time.Second + time.Duration(phase)*25*time.Millisecond
					r := newControllerRig(t, 400*time.Millisecond, burst(at, length, c.sig))
					r.up, r.down = sp[0], sp[1]
					r.warmup(at, fmt.Sprintf("%s %d ms", c.name, ms))
					r.runUntil(at+length+4*time.Second, true)
					runs++
					if r.closedAt >= 0 {
						closedRuns++
						if r.closedBy != c.want {
							wrong++
						}
					}
				}
			}
			record(t, "%s 突发 %d ms：%d / %d 次关闸（原因不符 %d）", c.name, ms, closedRuns, runs, wrong)
			assertf(t, wrong == 0, "%s 突发 %d ms：%d 次关闸原因不是 %q", c.name, ms, wrong, c.want)
			switch {
			case c.name == "D" && ms <= 240, c.name == "R" && ms <= 480, c.name == "O" && ms < 250:
				assertf(t, closedRuns == 0, "%s 对 %d ms 突发应容忍，却关闸 %d 次", c.name, ms, closedRuns)
			case c.name == "D" && (ms == 260 || ms == 300):
				assertf(t, closedRuns > 0, "D 对 %d ms 突发在最不利相位与拆分下应关闸，0 次", ms)
			case c.name == "D" && ms >= 750, c.name != "D" && ms >= 1000:
				assertf(t, closedRuns == runs, "%s 对 %d ms 突发应每个相位都关闸，只关了 %d / %d 次", c.name, ms, closedRuns, runs)
			}
		}
	}
}

// 清单 #131（I3、§2 握手窗口控制端）：握手窗口（尚无入站、R 未起算；QUIC 未建立、O 不存在）里只有 D：
// 断路后 ≤ 1700 ms 关闸并停止重传握手包。
func TestControllerGateClosesWithin1700msInHandshakeWindow(t *testing.T) {
	for _, rtt := range []time.Duration{100 * time.Millisecond, 400 * time.Millisecond} {
		cut := 3 * time.Second
		r := newControllerRig(t, rtt, cutFrom(cut, dirUp, dirDown))
		r.dataFrom, r.handshakeAt = -1, -1
		r.warmup(cut, fmt.Sprintf("RTT %v 握手窗口", rtt))
		r.runUntil(cut+3*time.Second, true)
		assertf(t, r.closedAt >= 0 && r.closedAt-cut <= 1700*time.Millisecond, "RTT %v：握手窗口断路后 %v 才关闭，上界 1700 ms", rtt, r.closedAt-cut)
		assertf(t, r.closedBy == ClosedByDisco, "RTT %v：握手窗口里只有 D，关闸原因却是 %q", rtt, r.closedBy)
		r.runUntil(cut+5*time.Second, false)
		assertf(t, !r.gate.IsOpen(), "RTT %v：D 关闭后闸门重新打开、继续重传握手包", rtt)
	}
}

// 清单 #142（I3、§2 O）：O 的计时器在 helper 内：“握手完成”命令后起算，每次进展心跳重置，ack_stall_ms 内无心跳即
// 自行关闸（粘性），不依赖 Rust 关闸命令。A) 无心跳；B) 心跳持续；C) 心跳停止；D) 粘性；E) 心跳延迟 X = 300 ms；
// F) 旧 epoch / 旧连接号的心跳被忽略。两端同样。
func TestHelperAckWatchdogClosesGateWithoutRustCommand(t *testing.T) {
	mk := map[string]func() *rig{
		"控制端": func() *rig { return newControllerRig(t, 100*time.Millisecond, nil) },
		"被控端": func() *rig {
			r := newHostRig(t, hostRigConfig(), 50*time.Millisecond, 50*time.Millisecond, nil)
			r.dataFrom = 1500 * time.Millisecond // 首个入站在握手完成命令之前 H 以内，H 不触发
			return r
		},
	}
	const hs = 2 * time.Second
	window := func(d time.Duration) bool {
		return d >= 1000*time.Millisecond && d <= 1000*time.Millisecond+10*time.Millisecond+rigStep
	}
	for _, side := range []string{"控制端", "被控端"} {
		a := mk[side]()
		a.handshakeAt, a.probeUntil = hs, hs
		a.warmup(hs, side+" A")
		a.runUntil(hs+3*time.Second, true)
		assertf(t, a.closedAt >= 0 && window(a.closedAt-hs), "%s A：握手完成后无心跳，%v 后关闭，应在 ack_stall_ms（1000 ms + 粒度）", side, a.closedAt-hs)
		assertf(t, a.closedBy == ClosedByAck, "%s A：关闸原因 %q，应为 %q", side, a.closedBy, ClosedByAck)

		b := mk[side]()
		b.handshakeAt = hs
		b.warmup(hs, side+" B")
		b.runUntil(hs+10*time.Second, true)
		assertf(t, b.closedAt < 0 && b.heartbeatCount > 0, "%s B：心跳持续时闸门被 %q 关闭", side, b.closedBy)

		c := mk[side]()
		c.handshakeAt, c.probeUntil = hs, hs+3*time.Second
		c.warmup(hs+3*time.Second, side+" C")
		c.runUntil(hs+7*time.Second, true)
		assertf(t, c.closedAt >= 0 && window(c.closedAt-c.lastHeartbeat), "%s C：最后一次心跳后 %v 关闭，应在 ack_stall_ms 内", side, c.closedAt-c.lastHeartbeat)
		assertf(t, c.closedBy == ClosedByAck, "%s C：上报的关闸原因 %q，应为 gate_closed(reason = ack)", side, c.closedBy)
		for i := 0; i < 5; i++ {
			c.gate.OnProgressHeartbeat(rigEpoch, rigConn)
			c.step()
		}
		assertf(t, !c.gate.IsOpen(), "%s D：O 关闭后收到心跳，闸门重新打开", side)

		e := mk[side]()
		e.handshakeAt, e.probeUntil, e.hbDelay = hs, hs+3*time.Second, 300*time.Millisecond
		e.warmup(hs+3*time.Second, side+" E")
		e.runUntil(hs+8*time.Second, true)
		nominal := c.closedAt
		assertf(t, e.closedAt >= 0, "%s E：心跳延迟 300 ms，闸门没有关闭", side)
		shift := e.closedAt - nominal
		assertf(t, shift >= 300*time.Millisecond-2*rigStep && shift <= 300*time.Millisecond+2*rigStep,
			"%s E：关闸时刻比无延迟时推后 %v，应推后 X = 300 ms", side, shift)

		f := mk[side]()
		f.handshakeAt, f.hbEpoch, f.hbConn = hs, rigEpoch-1, rigConn
		f.warmup(hs, side+" F（旧 epoch）")
		f.runUntil(hs+3*time.Second, true)
		assertf(t, f.closedAt >= 0 && window(f.closedAt-hs), "%s F：旧 epoch 的心跳重置了 O（%v 后才关闭或没有关闭）", side, f.closedAt-hs)
		f2 := mk[side]()
		f2.handshakeAt, f2.hbEpoch, f2.hbConn = hs, rigEpoch, rigConn+1
		f2.warmup(hs, side+" F（旧连接号）")
		f2.runUntil(hs+3*time.Second, true)
		assertf(t, f2.closedAt >= 0 && window(f2.closedAt-hs), "%s F：旧连接号的心跳重置了 O", side)
	}
}

// 清单 #143（I3、§2 握手窗口边缘情形）：断路发生在首个入站之前、首个入站是断路前已在途的包：被控端相对断路时刻在
// RTT + H + P_H（RTT 400 ms 时 1910 ms）内关闸。P_H = 100 ms 的对照实现在 RTT 400 ms 时可达 2000 ms。
// 变体 F（控制端首包晚于断路）只记录被控端闸门的关闭时刻与触发条件，不作断言（OQ-26，Jack 2026-11-11 决定）。
func TestHostHandshakeWatchdogEdgePhaseWithin1910ms(t *testing.T) {
	recordEdgePhaseVariantF(t)

	const h, pH = 1500 * time.Millisecond, 10 * time.Millisecond
	edgeBound := func(rtt, ph time.Duration) time.Duration { return rtt + h + ph }
	assertf(t, edgeBound(400*time.Millisecond, 100*time.Millisecond) > 1910*time.Millisecond, "P_H = 100 ms 的对照上界应超过 1910 ms")
	for _, rtt := range []time.Duration{100 * time.Millisecond, 250 * time.Millisecond, 400 * time.Millisecond} {
		cut := 3 * time.Second
		r := newHostRig(t, hostRigConfig(), rtt, 0, cutFrom(cut, dirUp, dirDown))
		r.handshakeAt = -1
		r.dataFrom = cut - rigStep
		r.warmup(cut, fmt.Sprintf("RTT %v", rtt))
		r.runUntil(cut+4*time.Second, true)
		assertf(t, r.inboundCount == 1, "RTT %v：夹具前置不成立，断路后到达的在途入站为 %d 个", rtt, r.inboundCount)
		assertf(t, r.closedAt >= 0 && r.closedAt-cut <= edgeBound(rtt, pH),
			"RTT %v：边缘相位下断路 %v 后才关闭，上界 %v", rtt, r.closedAt-cut, edgeBound(rtt, pH))
	}
	ok := newHostRig(t, hostRigConfig(), 200*time.Millisecond, 200*time.Millisecond, nil)
	ok.dataFrom, ok.handshakeAt = 2*time.Second, 2*time.Second+time.Second
	ok.warmup(2*time.Second, "对照：握手在 H 内完成")
	ok.runUntil(8*time.Second, true)
	assertf(t, ok.closedAt < 0, "握手在 H 内完成，闸门仍在 %v 被 %q 关闭", ok.closedAt, ok.closedBy)
}

// 变体 F：断路为“仅被控端→控制端不通”，发生在被控端已开闸之后、控制端发出首个直连包之前；控制端的 D 到
// t_c + 1700 ms 才关闸，被控端首个入站最晚约在 t_c + 1700 + RTT 到达。只记录，不作断言。
func recordEdgePhaseVariantF(t *testing.T) {
	for _, rtt := range []time.Duration{100 * time.Millisecond, 400 * time.Millisecond} {
		cut := 3 * time.Second
		r := newHostRig(t, hostRigConfig(), rtt/2, rtt-rtt/2, cutFrom(cut, dirDown))
		r.handshakeAt = -1
		r.dataFrom = cut + 1700*time.Millisecond
		r.dataUntil = r.dataFrom + rigStep
		r.runUntil(cut+6*time.Second, true)
		closeAfter := "未关闭"
		if r.closedAt >= 0 {
			closeAfter = (r.closedAt - cut).String()
		}
		record(t, "变体 F RTT %v：开闸 %v，首个入站 %d 个，被控端闸门关闭时刻（相对断路）%s，触发条件 %q；被控端经 DERP 的包数在单元夹具里不可观测（由 #4 的自建 derper 夹具承担）",
			rtt, r.everOpen, r.inboundCount, closeAfter, r.closedBy)
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
