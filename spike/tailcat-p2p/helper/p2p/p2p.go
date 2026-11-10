// Package p2p is the Phase 0 spike of the airhand direct-path helper: it
// forwards datagrams between one tailcat UDP port and a local datagram
// channel (loopback UDP or inherited pipes), behind a gate that only opens
// while the tailcat path is direct.
//
// Spike code: measurement only, not for reuse in Phase 1.
package p2p

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/netip"
	"sync"
	"sync/atomic"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
	"tailscale.com/wgengine/filter"
)

type Path string

const (
	PathNone   Path = "none"
	PathDERP   Path = "derp"
	PathDirect Path = "direct"
)

// Event is reported to the controlling process. It never carries the
// tailcat address or keys except in the explicit "offer"/"key" events.
type Event map[string]any

type Emit func(Event)

func now() int64 { return time.Now().UnixMicro() }

// Counters for one bridge.
type Counters struct {
	ToTunnel     atomic.Uint64 // datagrams written into the tunnel
	FromTunnel   atomic.Uint64 // datagrams delivered to the local channel
	GateDropped  atomic.Uint64 // local datagrams dropped because the gate was closed
	Oversize     atomic.Uint64 // local datagrams dropped for exceeding MaxUDPPayload
	NoFlow       atomic.Uint64 // local datagrams dropped before the tunnel flow existed
	TunnelErrors atomic.Uint64
}

func (c *Counters) Snapshot() Event {
	return Event{
		"to_tunnel":     c.ToTunnel.Load(),
		"from_tunnel":   c.FromTunnel.Load(),
		"gate_dropped":  c.GateDropped.Load(),
		"oversize":      c.Oversize.Load(),
		"no_flow":       c.NoFlow.Load(),
		"tunnel_errors": c.TunnelErrors.Load(),
	}
}

// Gate tracks the last path verdict and opens only on PathDirect.
type Gate struct {
	mu   sync.Mutex
	path Path
	open atomic.Bool
	emit Emit
	role string
}

func (g *Gate) Set(p Path, extra Event) {
	g.mu.Lock()
	changed := g.path != p
	g.path = p
	g.open.Store(p == PathDirect)
	g.mu.Unlock()
	if changed && g.emit != nil {
		ev := Event{"ev": "path", "role": g.role, "path": string(p), "t_us": now()}
		for k, v := range extra {
			ev[k] = v
		}
		g.emit(ev)
	}
}

func (g *Gate) Open() bool { return g.open.Load() }

// Local is the datagram channel to the Rust side.
type Local interface {
	Recv(buf []byte) (int, error)
	Send(b []byte) error
	Close() error
}

// Pump forwards local→tunnel through the gate. tunnel() returns the current
// tunnel flow or nil if none exists yet.
func Pump(local Local, tunnel func() net.Conn, g *Gate, c *Counters) error {
	buf := make([]byte, 64<<10)
	for {
		n, err := local.Recv(buf)
		if err != nil {
			return err
		}
		if n > tailcat.MaxUDPPayload {
			c.Oversize.Add(1)
			continue
		}
		if !g.Open() {
			c.GateDropped.Add(1)
			continue
		}
		t := tunnel()
		if t == nil {
			c.NoFlow.Add(1)
			continue
		}
		if _, err := t.Write(buf[:n]); err != nil {
			c.TunnelErrors.Add(1)
			continue
		}
		c.ToTunnel.Add(1)
	}
}

// Drain forwards tunnel→local until the flow ends.
func Drain(t net.Conn, local Local, c *Counters) error {
	buf := make([]byte, 64<<10)
	for {
		n, err := t.Read(buf)
		if err != nil {
			return err
		}
		if err := local.Send(buf[:n]); err != nil {
			return err
		}
		c.FromTunnel.Add(1)
	}
}

// ParseClientKey enforces I14: an empty or unparsable key refuses to start.
func ParseClientKey(s string) (key.NodePublic, error) {
	var k key.NodePublic
	if s == "" {
		return k, errors.New("empty client key")
	}
	if err := k.UnmarshalText([]byte(s)); err != nil {
		return k, fmt.Errorf("invalid client key: %w", err)
	}
	if k.IsZero() {
		return k, errors.New("zero client key")
	}
	return k, nil
}

type HostConfig struct {
	ClientKey    string
	Port         uint16
	Region       *tailcfg.DERPRegion
	PollInterval time.Duration
	Logf         logger.Logf // nil = discard
	Emit         Emit
}

// Host is one epoch's tailcat Server serving exactly one UDP port to exactly
// one allowed client.
type Host struct {
	Srv      *tailcat.Server
	Gate     *Gate
	Counters Counters
	client   key.NodePublic
	flow     atomic.Pointer[net.Conn]
	gotFlow  atomic.Bool
	local    Local
	cancel   context.CancelFunc
	cfg      HostConfig
}

func NewHost(cfg HostConfig) (*Host, error) {
	k, err := ParseClientKey(cfg.ClientKey)
	if err != nil {
		return nil, err
	}
	if cfg.Port == 0 {
		return nil, errors.New("port must be non-zero")
	}
	if cfg.Logf == nil {
		cfg.Logf = logger.Discard
	}
	if cfg.PollInterval == 0 {
		cfg.PollInterval = 100 * time.Millisecond
	}
	h := &Host{client: k, cfg: cfg, Gate: &Gate{path: PathNone, emit: cfg.Emit, role: "host"}}
	h.Srv = &tailcat.Server{
		Logf:           cfg.Logf,
		Region:         cfg.Region,
		AllowedClients: []key.NodePublic{k},
		ServedTCPPorts: []filter.PortRange{},
		ServedUDPPorts: []filter.PortRange{{First: cfg.Port, Last: cfg.Port}},
		OnUDP: func(port uint16) func(tailcat.ConnPacketConn) {
			if port != cfg.Port {
				return nil
			}
			return h.handleFlow
		},
	}
	return h, nil
}

func (h *Host) handleFlow(c tailcat.ConnPacketConn) {
	if !h.gotFlow.CompareAndSwap(false, true) {
		c.Close()
		return
	}
	var nc net.Conn = c
	h.flow.Store(&nc)
	if h.cfg.Emit != nil {
		h.cfg.Emit(Event{"ev": "flow", "role": "host", "t_us": now()})
	}
	if h.local != nil {
		err := Drain(c, h.local, &h.Counters)
		if h.cfg.Emit != nil {
			h.cfg.Emit(Event{"ev": "flow_end", "role": "host", "t_us": now(), "error": fmt.Sprint(err)})
		}
	}
	h.flow.Store(nil)
	c.Close()
}

func (h *Host) tunnel() net.Conn {
	if p := h.flow.Load(); p != nil {
		return *p
	}
	return nil
}

// Start brings the server up and returns its tailcat address. local may be
// nil for tests that only exercise tailcat.
func (h *Host) Start(local Local) (tailcat.Addr, error) {
	h.local = local
	if err := h.Srv.Start(); err != nil {
		return "", err
	}
	ctx, cancel := context.WithCancel(context.Background())
	h.cancel = cancel
	go h.watch(ctx)
	if local != nil {
		go Pump(local, h.tunnel, h.Gate, &h.Counters)
	}
	return h.Srv.TailcatAddr(), nil
}

// watch polls Server.Status: the peer's CurAddr is non-empty only while
// magicsock has a direct UDP path to it.
func (h *Host) watch(ctx context.Context) {
	t := time.NewTicker(h.cfg.PollInterval)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
		st := h.Srv.Status()
		ps, ok := st.Peer[h.client]
		switch {
		case !ok:
			h.Gate.Set(PathNone, nil)
		case ps.CurAddr != "":
			h.Gate.Set(PathDirect, nil)
		case ps.Relay != "":
			h.Gate.Set(PathDERP, nil)
		default:
			h.Gate.Set(PathNone, nil)
		}
	}
}

func (h *Host) Close() error {
	if h.cancel != nil {
		h.cancel()
	}
	if t := h.tunnel(); t != nil {
		t.Close()
	}
	return h.Srv.Close()
}

type CtrlConfig struct {
	Port         uint16
	PingInterval time.Duration
	MinTimeout   time.Duration // lower bound of the per-ping timeout
	Misses       int           // consecutive missed direct pongs before non-direct
	Logf         logger.Logf
	Emit         Emit
}

// Ctrl is one epoch's tailcat Client.
type Ctrl struct {
	Cli      *tailcat.Client
	Gate     *Gate
	Counters Counters
	cfg      CtrlConfig
	conn     net.Conn
	cancel   context.CancelFunc
}

func NewCtrl(cfg CtrlConfig) *Ctrl {
	if cfg.Logf == nil {
		cfg.Logf = logger.Discard
	}
	if cfg.PingInterval == 0 {
		cfg.PingInterval = 250 * time.Millisecond
	}
	if cfg.MinTimeout == 0 {
		cfg.MinTimeout = 750 * time.Millisecond
	}
	if cfg.Misses == 0 {
		cfg.Misses = 2
	}
	return &Ctrl{
		Cli:  &tailcat.Client{Key: key.NewNode(), Logf: cfg.Logf},
		Gate: &Gate{path: PathNone, emit: cfg.Emit, role: "ctrl"},
		cfg:  cfg,
	}
}

// PublicKey is available before dialing (the spec's "key first" step).
func (c *Ctrl) PublicKey() key.NodePublic { return c.Cli.PublicKey() }

// Dial connects to the server address and starts the path loop and the
// bridge (when local is non-nil).
func (c *Ctrl) Dial(ctx context.Context, addr tailcat.Addr, local Local) error {
	c.Cli.Server = addr
	t0 := time.Now()
	conn, err := c.Cli.DialUDPPort(ctx, c.cfg.Port)
	if err != nil {
		return err
	}
	c.conn = conn
	if c.cfg.Emit != nil {
		c.cfg.Emit(Event{"ev": "dialed", "role": "ctrl", "t_us": now(), "ms": time.Since(t0).Milliseconds()})
	}
	lctx, cancel := context.WithCancel(context.Background())
	c.cancel = cancel
	go c.pathLoop(lctx)
	if local != nil {
		go func() {
			err := Drain(conn, local, &c.Counters)
			if c.cfg.Emit != nil {
				c.cfg.Emit(Event{"ev": "drain_end", "role": "ctrl", "t_us": now(), "error": fmt.Sprint(err)})
			}
		}()
		go Pump(local, func() net.Conn { return conn }, c.Gate, &c.Counters)
	}
	return nil
}

func (c *Ctrl) Conn() net.Conn { return c.conn }

// pathLoop applies the spec's controller rule: a pong over DERP is
// non-direct at once; M consecutive pings without a direct pong within
// max(MinTimeout, 3×RTT) are non-direct.
func (c *Ctrl) pathLoop(ctx context.Context) {
	var rtt time.Duration
	misses := 0
	for ctx.Err() == nil {
		to := max(c.cfg.MinTimeout, 3*rtt)
		pctx, cancel := context.WithTimeout(ctx, to)
		t0 := time.Now()
		r, err := c.Cli.DiscoPing(pctx)
		cancel()
		if ctx.Err() != nil {
			return
		}
		switch {
		case err != nil:
			misses++
			if misses >= c.cfg.Misses {
				c.Gate.Set(PathNone, Event{"misses": misses})
			}
		case r.Endpoint != "":
			misses = 0
			rtt = time.Duration(r.LatencySeconds * float64(time.Second))
			c.Gate.Set(PathDirect, Event{"rtt_us": rtt.Microseconds()})
		default:
			misses = 0
			c.Gate.Set(PathDERP, Event{"rtt_us": int64(r.LatencySeconds * 1e6)})
		}
		if wait := c.cfg.PingInterval - time.Since(t0); wait > 0 {
			select {
			case <-ctx.Done():
				return
			case <-time.After(wait):
			}
		}
	}
}

func (c *Ctrl) Close() error {
	if c.cancel != nil {
		c.cancel()
	}
	if c.conn != nil {
		c.conn.Close()
	}
	return c.Cli.Close()
}

// UDPLocal is candidate (i): a loopback UDP socket locked to one peer port.
type UDPLocal struct {
	Conn *net.UDPConn
	peer atomic.Pointer[netip.AddrPort]
}

func NewUDPLocal() (*UDPLocal, error) {
	c, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		return nil, err
	}
	_ = c.SetReadBuffer(4 << 20)
	_ = c.SetWriteBuffer(4 << 20)
	return &UDPLocal{Conn: c}, nil
}

func (u *UDPLocal) Port() uint16 { return uint16(u.Conn.LocalAddr().(*net.UDPAddr).Port) }

func (u *UDPLocal) SetPeer(port uint16) {
	ap := netip.AddrPortFrom(netip.AddrFrom4([4]byte{127, 0, 0, 1}), port)
	u.peer.Store(&ap)
}

func (u *UDPLocal) Recv(buf []byte) (int, error) {
	for {
		n, from, err := u.Conn.ReadFromUDPAddrPort(buf)
		if err != nil {
			return 0, err
		}
		if p := u.peer.Load(); p != nil && from.Port() == p.Port() && from.Addr().Unmap() == p.Addr() {
			return n, nil
		}
	}
}

func (u *UDPLocal) Send(b []byte) error {
	p := u.peer.Load()
	if p == nil {
		return nil
	}
	_, err := u.Conn.WriteToUDPAddrPort(b, *p)
	return err
}

func (u *UDPLocal) Close() error { return u.Conn.Close() }

// PipeLocal is candidate (ii): two inherited pipes with a 2-byte big-endian
// length prefix per datagram. No local port is opened.
type PipeLocal struct {
	R  io.Reader
	W  io.Writer
	mu sync.Mutex
	wb []byte
}

func (p *PipeLocal) Recv(buf []byte) (int, error) {
	var hdr [2]byte
	if _, err := io.ReadFull(p.R, hdr[:]); err != nil {
		return 0, err
	}
	n := int(hdr[0])<<8 | int(hdr[1])
	if n > len(buf) {
		return 0, fmt.Errorf("frame too large: %d", n)
	}
	if _, err := io.ReadFull(p.R, buf[:n]); err != nil {
		return 0, err
	}
	return n, nil
}

func (p *PipeLocal) Send(b []byte) error {
	if len(b) > 0xffff {
		return fmt.Errorf("frame too large: %d", len(b))
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	p.wb = append(p.wb[:0], byte(len(b)>>8), byte(len(b)))
	p.wb = append(p.wb, b...)
	_, err := p.W.Write(p.wb)
	return err
}

func (p *PipeLocal) Close() error {
	var errs []error
	if c, ok := p.R.(io.Closer); ok {
		errs = append(errs, c.Close())
	}
	if c, ok := p.W.(io.Closer); ok {
		errs = append(errs, c.Close())
	}
	return errors.Join(errs...)
}
