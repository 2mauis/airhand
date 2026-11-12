package p2p

import (
	"sort"
	"testing"
	"time"
)

// 虚拟时间夹具：一端 helper 闸门 + 对端的流量模型，按 rigStep 推进。对端每 250 ms 发一个直连数据报
// （keep-alive / 视频）→ 本端 OnInbound；本端探测流每 250 ms 一个、被确认后 → OnProgressHeartbeat（O）；
// 控制端闸门另有 DiscoPing（经 DiscoPinger 流水线发起，或闸门同步调用 DiscoPing）。
// 丢包按“信号 × 方向 × 发送时刻”注入；方向 up 为控制端→被控端，down 为被控端→控制端。

const (
	rigStep     = 5 * time.Millisecond
	rigInterval = 250 * time.Millisecond
	rigEpoch    = uint32(7)
	rigConn     = uint64(1)
)

type signal int

const (
	sigPing  signal = iota // DiscoPing 与 pong
	sigData                // 对端发来的数据报（R）
	sigProbe               // 本端探测流与对端的确认（O）
)

type dir int

const (
	dirUp   dir = iota // 控制端 → 被控端
	dirDown            // 被控端 → 控制端
)

// lossFn 报告在相对夹具起点 at 时刻、沿 d 方向发出的 s 信号是否丢失。
type lossFn func(s signal, d dir, at time.Duration) bool

func noLoss(signal, dir, time.Duration) bool { return false }

// cutFrom 从 at 起沿给定方向永久断路（全部信号）。
func cutFrom(at time.Duration, dirs ...dir) lossFn {
	return func(_ signal, d dir, t time.Duration) bool {
		if t < at {
			return false
		}
		for _, x := range dirs {
			if x == d {
				return true
			}
		}
		return false
	}
}

// burst 是 [start, start+length) 的完全丢包突发，只作用于 sigs（为空时作用于全部信号）、双向。
func burst(start, length time.Duration, sigs ...signal) lossFn {
	return func(s signal, _ dir, t time.Duration) bool {
		if t < start || t >= start+length {
			return false
		}
		if len(sigs) == 0 {
			return true
		}
		for _, x := range sigs {
			if x == s {
				return true
			}
		}
		return false
	}
}

func anyLoss(fs ...lossFn) lossFn {
	return func(s signal, d dir, t time.Duration) bool {
		for _, f := range fs {
			if f(s, d, t) {
				return true
			}
		}
		return false
	}
}

// gateUnderTest 是两端闸门共有的方法。
type gateUnderTest interface {
	Tick()
	IsOpen() bool
	OnInbound()
	OnHandshakeComplete()
	OnProgressHeartbeat(epoch uint32, conn uint64)
	CloseByCommand()
	ClosedBy() string
}

type rigEvent struct {
	at time.Duration
	fn func()
}

type rig struct {
	t      testing.TB
	clock  *fakeClock
	start  time.Time
	gate   gateUnderTest
	ctrl   *ControllerGate
	hpath  *fakeHostPath
	up     time.Duration // 控制端 → 被控端单程时延
	down   time.Duration // 被控端 → 控制端单程时延
	jitter func(n int) time.Duration
	loss   lossFn

	// 本端是被控端时 true（对端是控制端）。
	host bool
	// 对端数据报：从 dataFrom 起每 250 ms 一个，dataUntil 之后停止（0 = 不停）；dataFrom < 0 表示从不发送。
	dataFrom, dataUntil time.Duration
	// 本端探测流：handshakeAt 时送达“握手完成”命令（< 0 = 从不），之后每 250 ms 一个探测；
	// 被确认后经 hbDelay 送达心跳，心跳带 hbEpoch / hbConn。probeUntil 之后不再发探测（0 = 不停）。
	handshakeAt    time.Duration
	probeUntil     time.Duration
	hbDelay        time.Duration
	hbEpoch        uint32
	hbConn         uint64
	pongExtra      func(seq int) time.Duration
	outcome        func(s signal, failed bool)
	dropPong       func(seq int) bool
	events         []rigEvent
	nextData       time.Duration
	nextProbe      time.Duration
	handshakeSent  bool
	n              int
	inboundCount   int
	heartbeatCount int
	lastHeartbeat  time.Duration
	lastInbound    time.Duration
	pingsStarted   int
	pongsDelivered int
	maxOutstanding int
	outstanding    map[int]time.Time
	deadlines      []time.Duration
	syncDiscoPings int
	closedAt       time.Duration
	closedBy       string
	everOpen       bool
	openedAt       time.Duration
}

// rigPath 是夹具的控制端路径：同时实现同步 DiscoPing 与流水线 DiscoPinger。
type rigPath struct{ r *rig }

func (p rigPath) DiscoPing() (bool, time.Duration, error) {
	r := p.r
	r.syncDiscoPings++
	now := r.now()
	ok := !r.loss(sigPing, dirUp, now) && !r.loss(sigPing, dirDown, now+r.up)
	r.observe(sigPing, !ok)
	return ok, r.up + r.down, nil
}

func (p rigPath) Ping() (time.Duration, error) { return p.r.up + p.r.down, nil }

func (p rigPath) StartDiscoPing(seq int, deadline time.Time) {
	r := p.r
	r.pingsStarted++
	now := r.now()
	r.outstanding[seq] = deadline
	r.deadlines = append(r.deadlines, deadline.Sub(r.clock.Now()))
	if len(r.outstanding) > r.maxOutstanding {
		r.maxOutstanding = len(r.outstanding)
	}
	lost := r.loss(sigPing, dirUp, now) || r.loss(sigPing, dirDown, now+r.up) || (r.dropPong != nil && r.dropPong(seq))
	r.observe(sigPing, lost)
	if lost {
		return
	}
	rtt := r.up + r.down + r.jit()
	if r.pongExtra != nil {
		rtt += r.pongExtra(seq)
	}
	r.schedule(now+rtt, func() {
		r.pongsDelivered++
		delete(r.outstanding, seq)
		r.ctrl.OnDiscoPong(seq, true, rtt)
	})
}

func newControllerRig(t testing.TB, rtt time.Duration, loss lossFn) *rig {
	r := baseRig(t, rtt/2, rtt-rtt/2, loss)
	g, err := NewControllerGate(ControllerGateConfig{
		Interval: rigInterval, RxSilence: 1000 * time.Millisecond, AckStall: 1000 * time.Millisecond,
		Epoch: rigEpoch, Conn: rigConn,
	}, rigPath{r}, r.clock)
	assertNoErr(t, err, "NewControllerGate")
	r.gate, r.ctrl = g, g
	return r
}

func newHostRig(t testing.TB, cfg HostGateConfig, up, down time.Duration, loss lossFn) *rig {
	r := baseRig(t, up, down, loss)
	r.host = true
	r.hpath = &fakeHostPath{addr: directAddr}
	cfg.Epoch, cfg.Conn = rigEpoch, rigConn
	g, err := NewHostGate(cfg, r.hpath, r.clock)
	assertNoErr(t, err, "NewHostGate")
	r.gate = g
	return r
}

func baseRig(t testing.TB, up, down time.Duration, loss lossFn) *rig {
	clock := newClock()
	if loss == nil {
		loss = noLoss
	}
	return &rig{
		t: t, clock: clock, start: clock.Now(), up: up, down: down, loss: loss,
		handshakeAt: 0, hbEpoch: rigEpoch, hbConn: rigConn,
		outstanding: map[int]time.Time{}, closedAt: -1, openedAt: -1,
	}
}

func hostRigConfig() HostGateConfig {
	return HostGateConfig{
		RxSilence: 1000 * time.Millisecond, StatusPoll: poll,
		AckStall: 1000 * time.Millisecond, HandshakeWatchdog: 1500 * time.Millisecond,
	}
}

func (r *rig) observe(s signal, failed bool) {
	if r.outcome != nil {
		r.outcome(s, failed)
	}
}

func (r *rig) now() time.Duration { return r.clock.Now().Sub(r.start) }

func (r *rig) jit() time.Duration {
	if r.jitter == nil {
		return 0
	}
	r.n++
	return r.jitter(r.n)
}

func (r *rig) schedule(at time.Duration, fn func()) {
	i := sort.Search(len(r.events), func(i int) bool { return r.events[i].at > at })
	r.events = append(r.events, rigEvent{})
	copy(r.events[i+1:], r.events[i:])
	r.events[i] = rigEvent{at, fn}
}

// 对端→本端的方向，以及本端探测→对端确认的两个方向。
func (r *rig) inDir() (dir, time.Duration) {
	if r.host {
		return dirUp, r.up
	}
	return dirDown, r.down
}

func (r *rig) traffic(now time.Duration) {
	if r.handshakeAt >= 0 && !r.handshakeSent && now >= r.handshakeAt {
		r.handshakeSent = true
		r.gate.OnHandshakeComplete()
		r.nextProbe = now
	}
	if r.dataFrom >= 0 && now >= r.dataFrom && now >= r.nextData && (r.dataUntil == 0 || now < r.dataUntil) {
		r.nextData = now + rigInterval
		d, delay := r.inDir()
		lost := r.loss(sigData, d, now)
		r.observe(sigData, lost)
		if !lost {
			r.schedule(now+delay+r.jit(), func() {
				r.inboundCount++
				r.lastInbound = r.now()
				r.gate.OnInbound()
			})
		}
	}
	if r.handshakeSent && now >= r.nextProbe && (r.probeUntil == 0 || now < r.probeUntil) {
		r.nextProbe = now + rigInterval
		inD, back := r.inDir()
		outD, there := dirDown, r.down
		if !r.host {
			outD, there = dirUp, r.up
		}
		lost := r.loss(sigProbe, outD, now) || r.loss(sigProbe, inD, now+there)
		r.observe(sigProbe, lost)
		if !lost {
			r.schedule(now+there+back+r.hbDelay, func() {
				r.heartbeatCount++
				r.lastHeartbeat = r.now()
				r.gate.OnProgressHeartbeat(r.hbEpoch, r.hbConn)
			})
		}
	}
}

// step 推进一个 rigStep：先产生流量、送达到期事件，再 Tick；记录首次开闸与首次关闸。
func (r *rig) step() {
	r.clock.Advance(rigStep)
	now := r.now()
	r.traffic(now)
	for len(r.events) > 0 && r.events[0].at <= now {
		ev := r.events[0]
		r.events = r.events[1:]
		ev.fn()
	}
	for seq, dl := range r.outstanding {
		if !r.clock.Now().Before(dl) {
			delete(r.outstanding, seq)
		}
	}
	r.gate.Tick()
	open := r.gate.IsOpen()
	if open && !r.everOpen {
		r.everOpen, r.openedAt = true, now
	}
	if !open && r.everOpen && r.closedAt < 0 {
		r.closedAt, r.closedBy = now, r.gate.ClosedBy()
	}
}

// runUntil 推进到夹具时刻 until；stopOnClose 时在首次关闸后返回。
func (r *rig) runUntil(until time.Duration, stopOnClose bool) {
	for r.now() < until {
		r.step()
		if stopOnClose && r.closedAt >= 0 {
			return
		}
	}
}

// warmup 推进 d 并断言闸门已经打开（前置：闸门打开过，否则“没有误关”是恒真式）。
func (r *rig) warmup(d time.Duration, what string) {
	r.t.Helper()
	r.runUntil(d, true)
	assertf(r.t, r.everOpen, "%s：闸门在 %v 内没有打开过", what, d)
	assertf(r.t, r.closedAt < 0, "%s：断路之前闸门已在 %v 关闭（%q）", what, r.closedAt, r.closedBy)
}
