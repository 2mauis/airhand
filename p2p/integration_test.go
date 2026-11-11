// tailcat-p2p 规格「先红验收测试清单」中依赖真实 tailcat（v0.7.0）与进程内 derper 的用例（docs/specs/tailcat-p2p.md）。
// 夹具在 p2p/glue.go：tailcat v0.7.0 要求 Go 1.27.1，接入之前夹具是空操作桩，这些用例因断言不满足而失败。
package p2p

import (
	"context"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func newTestNet(t *testing.T) *TestNet {
	t.Helper()
	n, err := NewTestNet(TestNetConfig{KeepAlive: 250 * time.Millisecond, UpgradeTimeout: 10 * time.Second})
	assertNoErr(t, err, "NewTestNet")
	t.Cleanup(n.Close)
	return n
}

// 清单 #55（I3、§2，前置核实 PV-1）：并发调用 Client.DiscoPing（≥ 4 个同时在途、各自带 ctx）安全；
// 由 PV PR（DiscoPing 并发封装）变绿并移出预期红名单；若因库行为不符仍红，冻结 D 的实现、回规格。
func TestDiscoPingConcurrentCallsAreSafe(t *testing.T) {
	n := newTestNet(t)
	const calls = 8
	var inFlight, maxInFlight atomic.Int32
	type result struct {
		direct bool
		rtt    time.Duration
		err    error
	}
	results := make([]result, calls)
	var start, wg sync.WaitGroup
	start.Add(1)
	for i := 0; i < calls; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			start.Wait()
			cur := inFlight.Add(1)
			for {
				m := maxInFlight.Load()
				if cur <= m || maxInFlight.CompareAndSwap(m, cur) {
					break
				}
			}
			d, rtt, err := n.DiscoPing(ctx)
			inFlight.Add(-1)
			results[i] = result{d, rtt, err}
		}(i)
	}
	start.Done()
	wg.Wait()
	assertf(t, maxInFlight.Load() >= 4, "同时在途的 DiscoPing 最多 %d 个，应 ≥ 4", maxInFlight.Load())
	for i, r := range results {
		expectf(t, r.err == nil, "第 %d 个并发 DiscoPing 出错：%v", i+1, r.err)
		expectf(t, r.direct && r.rtt > 0, "第 %d 个并发 DiscoPing 没有得到 direct pong（direct=%v rtt=%v）", i+1, r.direct, r.rtt)
	}
	assertf(t, n.PongsReceived() >= calls, "控制端只收到 %d 个 disco pong，应 ≥ %d", n.PongsReceived(), calls)
}

// 清单 #100（I24 方案 (a)、F4）：控制端在收到 DirectGateOpen 之前不向隧道写任何数据，升级仍在
// upgrade_timeout_secs 内完成；记录被控端判定 direct 的时延分布。
func TestSchemeAControllerSendsNothingBeforeGateOpenAndUpgradeCompletes(t *testing.T) {
	n := newTestNet(t)
	var lat []time.Duration
	for i := 0; i < 5; i++ {
		res := n.Upgrade()
		lat = append(lat, res.HostDirectAfter)
		assertf(t, res.Completed, "第 %d 次升级没有完成", i+1)
		assertf(t, res.Elapsed > 0 && res.Elapsed <= 10*time.Second, "第 %d 次升级用时 %v，应在 (0, upgrade_timeout_secs]", i+1, res.Elapsed)
		assertf(t, res.ControllerWritesBeforeGateOpen == 0, "第 %d 次：控制端在收到 DirectGateOpen 之前写了 %d 个数据报", i+1, res.ControllerWritesBeforeGateOpen)
		assertf(t, res.HostDirectAfter > 0, "第 %d 次：没有测到被控端判定 direct 的时延", i+1)
	}
	record(t, "被控端判定 direct 的时延：%v", lat)
}

// 清单 #101（I16、F5）：为取 Client.PublicKey() 而创建 Client 的前后，向 DERP 图 URL（本进程计数服务器）的
// 请求数为 0、新建套接字数为 0。
func TestPublicKeyBeforeOfferMakesNoNetworkActivity(t *testing.T) {
	n := newTestNet(t)
	req0, sock0 := n.DERPMapRequests(), n.SocketsOpened()
	key, err := n.NewClientForPublicKey()
	assertNoErr(t, err, "NewClientForPublicKey")
	assertf(t, strings.HasPrefix(key, "nodekey:") && len(key) > len("nodekey:"), "没有取到节点公钥（%q）", key)
	assertf(t, n.DERPMapRequests() == req0, "取公钥时向 DERP 图 URL 发了 %d 个请求", n.DERPMapRequests()-req0)
	assertf(t, n.SocketsOpened() == sock0, "取公钥时新建了 %d 个套接字", n.SocketsOpened()-sock0)
}

// 清单 #102（I3、I2、I24）：直连稳定时仅靠每 250 ms 一个 keep-alive 数据报（无控制流 Ping），两端闸门 10 s 内保持打开。
func TestGateStaysOpenWithKeepAliveDatagramsAndNoPing(t *testing.T) {
	n := newTestNet(t)
	res := n.HoldWithKeepAliveOnly(10 * time.Second)
	assertf(t, res.KeepAlivesSent >= 40, "10 s 内只发了 %d 个 keep-alive，应每 250 ms 一个（≥ 40）", res.KeepAlivesSent)
	assertf(t, res.PingsSent == 0, "夹具发了 %d 个控制流 Ping，本条要求无 Ping", res.PingsSent)
	assertf(t, res.HostGateOpen && res.ControllerGateOpen, "只有 keep-alive 时闸门被关闭（被控端 %v、控制端 %v）", res.HostGateOpen, res.ControllerGateOpen)
}
