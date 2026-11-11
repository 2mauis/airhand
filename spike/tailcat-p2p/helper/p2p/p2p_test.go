package p2p

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"example.com/airhand-p2p-spike/localderp"
	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
	"tailscale.com/wgengine/filter"
)

// Results are printed as "PHASE0 <name> <value>" lines so the report can
// quote them verbatim.
func report(t *testing.T, format string, args ...any) {
	t.Helper()
	line := fmt.Sprintf("PHASE0 "+format, args...)
	t.Log(line)
	fmt.Fprintln(os.Stderr, line)
}

func relay(t *testing.T) *tailcfg.DERPRegion {
	t.Helper()
	r, err := localderp.Start("127.0.0.1", 0, 0)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { r.Close() })
	return r.Region
}

func newClient(addr tailcat.Addr) *tailcat.Client {
	return &tailcat.Client{Server: addr, Key: key.NewNode(), Logf: logger.Discard}
}

// echoServer serves one UDP port and records the size of every datagram it
// receives.
func echoServer(t *testing.T, region *tailcfg.DERPRegion, port uint16, allowed []key.NodePublic) (*tailcat.Server, chan int) {
	t.Helper()
	got := make(chan int, 1024)
	s := &tailcat.Server{
		Logf:           logger.Discard,
		Region:         region,
		AllowedClients: allowed,
		ServedTCPPorts: []filter.PortRange{},
		ServedUDPPorts: []filter.PortRange{{First: port, Last: port}},
		OnUDP: func(p uint16) func(tailcat.ConnPacketConn) {
			if p != port {
				return nil
			}
			return func(c tailcat.ConnPacketConn) {
				buf := make([]byte, 65536)
				for {
					n, err := c.Read(buf)
					if err != nil {
						return
					}
					got <- n
					c.Write(buf[:n])
				}
			}
		},
	}
	if err := s.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s, got
}

func waitDirect(t *testing.T, c *tailcat.Client, within time.Duration) time.Duration {
	t.Helper()
	t0 := time.Now()
	for time.Since(t0) < within {
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		r, err := c.DiscoPing(ctx)
		cancel()
		if err == nil && r.Endpoint != "" {
			return time.Since(t0)
		}
		time.Sleep(100 * time.Millisecond)
	}
	t.Fatalf("no direct path within %v", within)
	return 0
}

func TestEmptyOrInvalidAllowlistRefusesToStart(t *testing.T) {
	for _, k := range []string{"", "garbage", "nodekey:00", "nodekey:" + strings.Repeat("0", 64)} {
		if _, err := NewHost(HostConfig{ClientKey: k, Port: 4433}); err == nil {
			t.Errorf("NewHost(%q) succeeded", k)
		}
	}
}

// Phase 0 item 5: with an empty AllowedClients, v0.7.0 admits any client.
func TestEmptyAllowlistAdmitsAnyone(t *testing.T) {
	region := relay(t)
	s, _ := echoServer(t, region, 7000, nil)
	c := newClient(s.TailcatAddr())
	defer c.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	_, err := c.Ping(ctx)
	report(t, "item5 empty_allowlist_random_client_ping_err=%v", err)
	if err != nil {
		t.Fatalf("expected v0.7.0 to admit any client with an empty allowlist, got %v", err)
	}
}

// Phase 0 items 4 and 5: a client not on the allowlist gets no meowed reply;
// Ping gives up after its internal 10 s timeout even with a longer ctx.
func TestOnlyAllowedClientKeyConnects(t *testing.T) {
	region := relay(t)
	allowed := key.NewNode()
	s, _ := echoServer(t, region, 7000, []key.NodePublic{allowed.Public()})

	ok := &tailcat.Client{Server: s.TailcatAddr(), Key: allowed, Logf: logger.Discard}
	defer ok.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	r, err := ok.Ping(ctx)
	if err != nil {
		t.Fatalf("allowed client: %v", err)
	}
	report(t, "item4 allowed_client_first_ping_ms=%d", r.Latency.Milliseconds())

	bad := newClient(s.TailcatAddr())
	defer bad.Close()
	ctx2, cancel2 := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel2()
	t0 := time.Now()
	_, err = bad.Ping(ctx2)
	el := time.Since(t0)
	report(t, "item4 disallowed_client_ping_elapsed_ms=%d err=%v", el.Milliseconds(), err)
	if err == nil {
		t.Fatal("disallowed client was admitted")
	}
	if el < 9*time.Second || el > 12*time.Second {
		t.Errorf("Ping internal timeout: got %v, want ~10s", el)
	}
}

// Phase 0 item 3: DiscoPing is bounded only by ctx when no pong arrives.
func TestDiscoPingTimeoutIsCtxBound(t *testing.T) {
	region := relay(t)
	s, _ := echoServer(t, region, 7000, nil)
	c := newClient(s.TailcatAddr())
	defer c.Close()
	waitDirect(t, c, 15*time.Second)
	s.Close()
	for _, to := range []time.Duration{300 * time.Millisecond, 750 * time.Millisecond, 2 * time.Second} {
		ctx, cancel := context.WithTimeout(context.Background(), to)
		t0 := time.Now()
		_, err := c.DiscoPing(ctx)
		cancel()
		report(t, "item3 discoping_dead_server ctx=%v elapsed_ms=%d err=%v", to, time.Since(t0).Milliseconds(), err)
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Errorf("want DeadlineExceeded, got %v", err)
		}
	}
}

// Phase 0 item 2: what happens to datagrams larger than MaxUDPPayload, both
// while the path is still DERP and once it is direct.
func TestOversizeDatagrams(t *testing.T) {
	region := relay(t)
	s, got := echoServer(t, region, 7000, nil)
	c := newClient(s.TailcatAddr())
	defer c.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	conn, err := c.DialUDPPort(ctx, 7000)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	sizes := []int{1200, 1232, 1233, 1280, 1400, 1500, 2000, 8000, 16000, 60000}
	probe := func(phase string) {
		for _, n := range sizes {
			// drain
			for len(got) > 0 {
				<-got
			}
			_, werr := conn.Write(make([]byte, n))
			recv := -1
			select {
			case recv = <-got:
			case <-time.After(1500 * time.Millisecond):
			}
			report(t, "item2 path=%s write=%d write_err=%v server_received=%d", phase, n, werr, recv)
		}
	}
	probe("derp_or_early")
	d := waitDirect(t, c, 15*time.Second)
	report(t, "item2 direct_after_ms=%d", d.Milliseconds())
	probe("direct")
}

// Phase 0 item 6 / I9: several Servers in one process are independent, and
// repeated epoch create/close does not leak goroutines.
func TestMultipleServersInOneProcessAreIndependent(t *testing.T) {
	region := relay(t)
	base := runtime.NumGoroutine()
	type pair struct {
		s    *tailcat.Server
		got  chan int
		c    *tailcat.Client
		conn net.Conn
	}
	var ps []*pair
	for i := range 3 {
		s, got := echoServer(t, region, uint16(7000+i), nil)
		c := newClient(s.TailcatAddr())
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		conn, err := c.DialUDPPort(ctx, uint16(7000+i))
		cancel()
		if err != nil {
			t.Fatal(err)
		}
		ps = append(ps, &pair{s, got, c, conn})
	}
	send := func(p *pair, n int) bool {
		for len(p.got) > 0 {
			<-p.got
		}
		p.conn.Write(make([]byte, n))
		select {
		case m := <-p.got:
			return m == n
		case <-time.After(2 * time.Second):
			return false
		}
	}
	for i, p := range ps {
		if !send(p, 100+i) {
			t.Fatalf("pair %d: no delivery", i)
		}
	}
	ps[1].conn.Close()
	ps[1].c.Close()
	ps[1].s.Close()
	for _, i := range []int{0, 2} {
		if !send(ps[i], 500) {
			t.Fatalf("pair %d broken after closing pair 1", i)
		}
	}
	report(t, "item6 three_servers_one_process independent=true close_one_others_ok=true")
	for _, i := range []int{0, 2} {
		ps[i].conn.Close()
		ps[i].c.Close()
		ps[i].s.Close()
	}

	// Ten sequential epochs on the same process.
	var mu sync.Mutex
	for e := range 10 {
		s, got := echoServer(t, region, 7100, nil)
		c := newClient(s.TailcatAddr())
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		conn, err := c.DialUDPPort(ctx, 7100)
		cancel()
		if err != nil {
			t.Fatalf("epoch %d: %v", e, err)
		}
		p := &pair{s, got, c, conn}
		mu.Lock()
		okd := send(p, 64)
		mu.Unlock()
		if !okd {
			t.Fatalf("epoch %d: no delivery", e)
		}
		conn.Close()
		c.Close()
		s.Close()
	}
	time.Sleep(3 * time.Second)
	after := runtime.NumGoroutine()
	report(t, "item6 goroutines_before=%d after_13_server_lifecycles=%d", base, after)
}

// I9: only the one UDP port is served; TCP is filtered silently (timeout, no
// RST), and other UDP ports get nothing.
func TestServesExactlyOneUDPPortAndNoTCP(t *testing.T) {
	region := relay(t)
	s, got := echoServer(t, region, 7000, nil)
	c := newClient(s.TailcatAddr())
	defer c.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	other, err := c.DialUDPPort(ctx, 7001)
	if err != nil {
		t.Fatal(err)
	}
	other.Write([]byte("x"))
	select {
	case <-got:
		t.Fatal("datagram to an unserved port was delivered")
	case <-time.After(time.Second):
	}
	other.SetReadDeadline(time.Now().Add(time.Second))
	if _, err := other.Read(make([]byte, 10)); err == nil {
		t.Fatal("reply from unserved UDP port")
	}
	tctx, tcancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer tcancel()
	t0 := time.Now()
	_, err = c.DialTCPPort(tctx, 22)
	report(t, "I9 tcp_dial_port22 elapsed_ms=%d err=%v", time.Since(t0).Milliseconds(), err)
	if err == nil {
		t.Fatal("TCP connect succeeded")
	}
	if !errors.Is(err, context.DeadlineExceeded) && !strings.Contains(err.Error(), "timeout") && !strings.Contains(err.Error(), "deadline") {
		t.Fatalf("want a timeout (filtered, no RST), got %v", err)
	}
}

func TestGateDropsWhenPathNotDirect(t *testing.T) {
	g := &Gate{path: PathNone}
	var c Counters
	local := &chanLocal{in: make(chan []byte, 4), out: make(chan []byte, 4)}
	tun := &recordConn{}
	go Pump(local, func() net.Conn { return tun }, g, &c)
	send := func(b []byte) { local.in <- b; time.Sleep(30 * time.Millisecond) }
	send([]byte("a"))
	g.Set(PathDERP, nil)
	send([]byte("b"))
	g.Set(PathDirect, nil)
	send([]byte("c"))
	send(make([]byte, tailcat.MaxUDPPayload+1))
	g.Set(PathNone, nil)
	send([]byte("d"))
	if c.ToTunnel.Load() != 1 || c.GateDropped.Load() != 3 || c.Oversize.Load() != 1 {
		t.Fatalf("counters: %v", c.Snapshot())
	}
}

type chanLocal struct{ in, out chan []byte }

func (l *chanLocal) Recv(b []byte) (int, error) { return copy(b, <-l.in), nil }
func (l *chanLocal) Send(b []byte) error        { l.out <- append([]byte(nil), b...); return nil }
func (l *chanLocal) Close() error               { return nil }

type recordConn struct {
	net.Conn
	mu sync.Mutex
	n  int
}

func (r *recordConn) Write(b []byte) (int, error) {
	r.mu.Lock()
	r.n++
	r.mu.Unlock()
	return len(b), nil
}
