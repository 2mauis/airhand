package p2p

// Hermetic PV-1 fixture, authored independently of the Phase 0 spike.
import (
	"context"
	"errors"
	"net"
	"net/http/httptest"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/derp/derpserver"
	"tailscale.com/net/stun"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
	"tailscale.com/wgengine/filter"
)

type discoNet struct {
	client *tailcat.Client
	server *tailcat.Server
	sender *DiscoSender
	derp   *derpserver.Server
	http   *httptest.Server
	stun   *net.UDPConn
}

func newDiscoNet(cfg TestNetConfig) (_ *discoNet, err error) {
	n := &discoNet{}
	defer func() {
		if err != nil {
			n.close()
		}
	}()
	n.derp = derpserver.New(key.NewNode(), logger.Discard)
	n.http = httptest.NewUnstartedServer(derpserver.Handler(n.derp))
	n.http.StartTLS()
	n.stun, err = net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		return nil, err
	}
	go func() {
		data := make([]byte, 2048)
		for {
			size, peer, err := n.stun.ReadFromUDPAddrPort(data)
			if err != nil {
				return
			}
			id, err := stun.ParseBindingRequest(data[:size])
			if err == nil {
				_, _ = n.stun.WriteToUDPAddrPort(stun.Response(id, peer), peer)
			}
		}
	}()
	region := &tailcfg.DERPRegion{RegionID: 1, RegionCode: "pv", Nodes: []*tailcfg.DERPNode{{
		Name: "pv1", RegionID: 1, HostName: "127.0.0.1", IPv4: "127.0.0.1", IPv6: "none",
		DERPPort: n.http.Listener.Addr().(*net.TCPAddr).Port,
		STUNPort: n.stun.LocalAddr().(*net.UDPAddr).Port, STUNTestIP: "127.0.0.1", InsecureForTests: true,
	}}}
	n.client = &tailcat.Client{Logf: logger.Discard}
	n.server = &tailcat.Server{Region: region, Logf: logger.Discard,
		AllowedClients: []key.NodePublic{n.client.PublicKey()},
		ServedTCPPorts: []filter.PortRange{}, ServedUDPPorts: []filter.PortRange{},
	}
	if err = n.server.Start(); err != nil {
		return nil, &categoryError{category: categoryStartFailed, cause: err}
	}
	n.client.Server = n.server.TailcatAddr()
	n.sender = NewDiscoSender(n.client)
	ctx, cancel := context.WithTimeout(context.Background(), cfg.UpgradeTimeout)
	defer cancel()
	for {
		call, done := context.WithTimeout(ctx, time.Second)
		result, pingErr := n.client.DiscoPing(call)
		done()
		if pingErr == nil && result.Endpoint != "" {
			return n, nil
		}
		select {
		case <-ctx.Done():
			return nil, &categoryError{category: categoryUnknown, cause: errors.Join(ctx.Err(), pingErr)}
		case <-time.After(25 * time.Millisecond):
		}
	}
}

func (n *discoNet) close() {
	if n.client != nil {
		_ = n.client.Close()
	}
	if n.server != nil {
		_ = n.server.Close()
	}
	if n.stun != nil {
		_ = n.stun.Close()
	}
	if n.derp != nil {
		_ = n.derp.Close()
	}
	if n.http != nil {
		n.http.CloseClientConnections()
		n.http.Close()
	}
}
