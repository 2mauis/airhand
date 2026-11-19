package p2p

import (
	"bufio"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"io"
	"math/big"
	"net"
	"os"
	"sync"
	"sync/atomic"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
	"tailscale.com/util/clientmetric"
	"tailscale.com/wgengine/filter"
)

const TunnelPort = 4433
const MaxDatagram = 1232

type controlCommand struct {
	TLSSeed     []byte               `json:"tls_seed"`
	Cmd         string               `json:"cmd"`
	Mode        string               `json:"mode"`
	NodePrivate string               `json:"node_private"`
	ClientKey   string               `json:"client_key"`
	Address     tailcat.Addr         `json:"address"`
	PSK         tailcat.PresharedKey `json:"psk"`
	Region      *tailcfg.DERPRegion  `json:"region,omitempty"`
	BridgePort  int                  `json:"bridge_port"`
	QuicPort    int                  `json:"quic_port"`
}

type controlOutput struct {
	mu  sync.Mutex
	out io.Writer
}

func (o *controlOutput) send(v any) error {
	o.mu.Lock()
	defer o.mu.Unlock()
	return json.NewEncoder(o.out).Encode(v)
}
func (o *controlOutput) event(kind, path, reason string, count int) {
	_ = o.send(Event{Kind: kind, Path: path, Reason: reason, Count: count})
}

// Only setup replies carry the invitation capability. They remain on the
// parent's private pipe and must be stored in Keychain, never logged as events.
func runControl(in io.Reader, out io.Writer) error {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	output := &controlOutput{out: out}
	commands := make(chan controlCommand)
	invalid := make(chan struct{}, 1)
	go func() {
		defer close(commands)
		scanner := bufio.NewScanner(in)
		scanner.Buffer(make([]byte, 4096), 64*1024)
		for scanner.Scan() {
			var cmd controlCommand
			if json.Unmarshal(scanner.Bytes(), &cmd) != nil {
				invalid <- struct{}{}
				return
			}
			select {
			case commands <- cmd:
			case <-ctx.Done():
				return
			}
		}
		if scanner.Err() != nil {
			invalid <- struct{}{}
		}
		cancel()
	}()
	go func() {
		tick := time.NewTicker(250 * time.Millisecond)
		defer tick.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-tick.C:
				if os.Getppid() == 1 {
					cancel()
					return
				}
			}
		}
	}()
	var bridge *udpBridge
	defer func() {
		if bridge != nil {
			done := make(chan struct{})
			go func() { bridge.close(); close(done) }()
			select {
			case <-done:
			case <-time.After(500 * time.Millisecond):
			}
		}
	}()
	fail := func() error { output.event("error", "none", "invalid_config", 0); return ErrInvalidConfig }
	for {
		select {
		case <-invalid:
			return fail()
		case <-ctx.Done():
			return nil
		case cmd, ok := <-commands:
			if !ok {
				return nil
			}
			switch cmd.Cmd {
			case "prepare":
				if bridge != nil {
					return fail()
				}
				private, err := parsePrivate(cmd.NodePrivate)
				if err != nil {
					return fail()
				}
				var certificate []byte
				if len(cmd.TLSSeed) != 0 {
					if len(cmd.TLSSeed) != ed25519.SeedSize {
						return fail()
					}
					privateTLS := ed25519.NewKeyFromSeed(cmd.TLSSeed)
					serial, err := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
					if err != nil {
						return fail()
					}
					template := &x509.Certificate{SerialNumber: serial, Subject: pkix.Name{CommonName: "AirHand paired device"}, NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().AddDate(5, 0, 0), KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth, x509.ExtKeyUsageClientAuth}, DNSNames: []string{"airhand"}}
					certificate, err = x509.CreateCertificate(rand.Reader, template, template, privateTLS.Public(), privateTLS)
					if err != nil {
						return fail()
					}
				}
				info := tailcat.ConnInfo{ServerPublic: tailcat.NodePublic{NodePublic: private.Public()}, ServerDiscoPublic: tailcat.DiscoPublicForNode(private), PresharedKey: cmd.PSK}
				if cmd.Mode == "controller" {
					if output.send(struct {
						Kind        string `json:"kind"`
						Public      string `json:"public"`
						Certificate []int  `json:"certificate"`
					}{"prepared", private.Public().String(), certificateArray(certificate)}) != nil {
						return io.ErrClosedPipe
					}
					continue
				}
				if cmd.Mode != "host" || cmd.PSK.IsZero() {
					return fail()
				}
				if cmd.Region != nil {
					info.Region = []*tailcfg.DERPRegion{cmd.Region}
				} else {
					info.RegionID = -1
					if info.Expand(ctx, tailcat.ExpandForServer) != nil {
						output.event("error", "none", "start_failed", 0)
						continue
					}
				}
				if output.send(struct {
					Kind        string       `json:"kind"`
					Address     tailcat.Addr `json:"address"`
					Certificate []int        `json:"certificate"`
				}{"prepared", info.Addr(), certificateArray(certificate)}) != nil {
					return io.ErrClosedPipe
				}
			case "start":
				// The test noop starts no transport; preparation remains independent.
				if noopActive {
					return nil
				}
				if bridge != nil {
					return fail()
				}
				b, err := newUDPBridge(ctx, cmd, output)
				if err != nil {
					return fail()
				}
				bridge = b
				go func() {
					if b.run() != nil && ctx.Err() == nil {
						output.event("error", "none", "start_failed", 0)
					}
					cancel()
				}()
			case "close":
				if bridge != nil {
					bridge.close()
				}
				output.event("gate_closed", "none", "command", 0)
				return nil
			default:
				return fail()
			}
		}
	}
}

func parsePrivate(text string) (key.NodePrivate, error) {
	var k key.NodePrivate
	if k.UnmarshalText([]byte(text)) != nil || k.IsZero() {
		return k, ErrInvalidConfig
	}
	return k, nil
}

type udpBridge struct {
	serverLife sync.Mutex
	ctx        context.Context
	cancel     context.CancelFunc
	cfg        controlCommand
	output     *controlOutput
	local      *net.UDPConn
	target     *net.UDPAddr
	server     *tailcat.Server
	client     *tailcat.Client
	mu         sync.Mutex
	flow       tailcat.ConnPacketConn
	gate       atomic.Bool
	ever       atomic.Bool
	stopped    atomic.Bool
	lastRx     atomic.Int64
	sent       atomic.Int64
	received   atomic.Int64
	once       sync.Once
}

func newUDPBridge(parent context.Context, cfg controlCommand, out *controlOutput) (*udpBridge, error) {
	if cfg.Mode != "host" && cfg.Mode != "controller" || cfg.BridgePort < 1 || cfg.BridgePort > 65535 || cfg.QuicPort < 1 || cfg.QuicPort > 65535 || cfg.BridgePort == cfg.QuicPort {
		return nil, ErrInvalidConfig
	}
	private, err := parsePrivate(cfg.NodePrivate)
	if err != nil {
		return nil, err
	}
	info, err := tailcat.ParseAddr(cfg.Address)
	if err != nil || info.PresharedKey.IsZero() || len(info.Region) != 1 {
		return nil, ErrInvalidConfig
	}
	if cfg.Mode == "host" && (info.ServerPublic.NodePublic != private.Public() || !info.PresharedKey.Equal(cfg.PSK)) {
		return nil, ErrInvalidConfig
	}
	ctx, cancel := context.WithCancel(parent)
	b := &udpBridge{ctx: ctx, cancel: cancel, cfg: cfg, output: out, target: &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: cfg.QuicPort}}
	if cfg.Mode == "host" {
		var allowed key.NodePublic
		if allowed.UnmarshalText([]byte(cfg.ClientKey)) != nil || allowed.IsZero() {
			cancel()
			return nil, ErrInvalidClientKey
		}
		b.cfg.ClientKey = allowed.String()
		b.server = &tailcat.Server{Key: private, PresharedKey: cfg.PSK, Region: info.Region[0], AllowedClients: []key.NodePublic{allowed}, Logf: logger.Discard, ServedTCPPorts: []filter.PortRange{}, ServedUDPPorts: []filter.PortRange{{First: TunnelPort, Last: TunnelPort}}}
		b.server.OnUDP = func(port uint16) func(tailcat.ConnPacketConn) {
			if port != TunnelPort {
				return nil
			}
			return b.attachFlow
		}
	} else {
		b.client = &tailcat.Client{Key: private, Server: cfg.Address, Logf: logger.Discard}
	}
	b.local, err = net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: cfg.BridgePort})
	if err != nil {
		cancel()
		return nil, ErrInvalidConfig
	}
	return b, nil
}
func (b *udpBridge) close() {
	b.once.Do(func() {
		b.gate.Store(false)
		b.stopped.Store(true)
		b.emitCounters()
		b.cancel()
		_ = b.local.Close()
		b.mu.Lock()
		if b.flow != nil {
			_ = b.flow.Close()
		}
		b.mu.Unlock()
		if b.client != nil {
			_ = b.client.Close()
		}
		if b.server != nil {
			b.serverLife.Lock()
			_ = b.server.Close()
			b.serverLife.Unlock()
		}
	})
}
func (b *udpBridge) setDirect(direct bool) {
	if b.stopped.Load() {
		return
	}
	if direct {
		if b.gate.CompareAndSwap(false, true) {
			b.ever.Store(true)
			b.output.event("path", "direct", "", 0)
		}
	} else if b.ever.Load() {
		b.gate.Store(false)
		b.stopped.Store(true)
		b.emitCounters()
		b.output.event("gate_closed", "none", "path_lost", 0)
		b.cancel()
	}
}
func (b *udpBridge) attachFlow(flow tailcat.ConnPacketConn) {
	b.mu.Lock()
	if b.flow != nil || b.stopped.Load() {
		b.mu.Unlock()
		_ = flow.Close()
		return
	}
	b.flow = flow
	b.mu.Unlock()
	defer func() { _ = flow.Close(); b.cancel() }()
	buf := make([]byte, MaxDatagram+1)
	for {
		n, err := flow.Read(buf)
		if err != nil {
			return
		}
		b.lastRx.Store(time.Now().UnixNano())
		if n > MaxDatagram || !b.gate.Load() {
			continue
		}
		if _, err = b.local.WriteToUDP(buf[:n], b.target); err != nil {
			return
		}
		b.received.Add(1)
	}
}
func (b *udpBridge) run() error {
	defer b.close()
	b.output.event("path", "none", "", 0)
	if b.server != nil {
		b.serverLife.Lock()
		if b.ctx.Err() != nil {
			b.serverLife.Unlock()
			return nil
		}
		startErr := b.server.Start()
		b.serverLife.Unlock()
		if startErr != nil {
			return ErrInvalidConfig
		}
		go b.pollHost()
	} else {
		go b.pollController()
		ctx, cancel := context.WithTimeout(b.ctx, 20*time.Second)
		defer cancel()
		flow, err := b.client.DialUDPPort(ctx, TunnelPort)
		if err != nil {
			return ErrInvalidConfig
		}
		go b.attachFlow(flow)
	}
	go b.pumpLocal()
	tick := time.NewTicker(time.Second)
	defer tick.Stop()
	for {
		select {
		case <-b.ctx.Done():
			return nil
		case <-tick.C:
			b.emitCounters()
		}
	}
}
func (b *udpBridge) pumpLocal() {
	buf := make([]byte, MaxDatagram+1)
	var writer *TunnelWriter
	for {
		n, from, err := b.local.ReadFromUDP(buf)
		if err != nil {
			return
		}
		if !from.IP.Equal(b.target.IP) || from.Port != b.target.Port || n > MaxDatagram || !b.gate.Load() {
			continue
		}
		b.mu.Lock()
		flow := b.flow
		b.mu.Unlock()
		if flow == nil {
			continue
		}
		if writer == nil {
			writer, _ = NewTunnelWriter(flow, 500*time.Millisecond, func(error) { b.setDirect(false) })
		}
		if _, err = writer.WriteTo(buf[:n], flow.RemoteAddr()); err != nil {
			b.setDirect(false)
			return
		}
		b.sent.Add(1)
	}
}
func (b *udpBridge) pollHost() {
	tick := time.NewTicker(100 * time.Millisecond)
	defer tick.Stop()
	for {
		select {
		case <-b.ctx.Done():
			return
		case <-tick.C:
			direct := false
			for _, peer := range b.server.Status().Peer {
				if peer.PublicKey.String() == b.cfg.ClientKey && peer.CurAddr != "" {
					direct = true
				}
			}
			last := b.lastRx.Load()
			if last != 0 && time.Since(time.Unix(0, last)) >= time.Second {
				direct = false
			}
			b.setDirect(direct)
		}
	}
}
func (b *udpBridge) pollController() {
	results := NewDiscoSender(b.client).Pipeline(b.ctx, func() time.Duration { return 950 * time.Millisecond })
	pending := map[uint64]bool{}
	next := uint64(0)
	failures := 0
	for result := range results {
		pending[result.Sequence] = result.Err == nil && result.Pong != nil && result.Pong.Endpoint != ""
		for {
			direct, ok := pending[next]
			if !ok {
				break
			}
			delete(pending, next)
			next++
			if direct {
				failures = 0
				b.setDirect(true)
			} else {
				failures++
				if failures >= 3 {
					b.setDirect(false)
				}
			}
		}
	}
}

func certificateArray(bytes []byte) []int {
	out := make([]int, len(bytes))
	for i, b := range bytes {
		out[i] = int(b)
	}
	return out
}

func (b *udpBridge) emitCounters() {
	b.output.event("to_tunnel", "none", "", int(b.sent.Load()))
	b.output.event("from_tunnel", "none", "", int(b.received.Load()))
	for _, metric := range clientmetric.Metrics() {
		switch metric.Name() {
		case "magicsock_send_data_derp", "magicsock_recv_data_derp":
			b.output.event(metric.Name(), "none", "", int(metric.Value()))
		}
	}
}
