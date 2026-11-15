package p2p

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
)

type controlCapture struct {
	sync.Mutex
	bytes.Buffer
}

func (c *controlCapture) Write(p []byte) (int, error) {
	c.Lock()
	defer c.Unlock()
	return c.Buffer.Write(p)
}
func (c *controlCapture) String() string {
	c.Lock()
	defer c.Unlock()
	return c.Buffer.String()
}

// Drive the same RunControl/newUDPBridge path as the sidecar. Only DERP/STUN
// discovery is local; the Tailcat stack, filter, allowlist and UDP bridge are real.
type controlHost struct {
	cfg             controlCommand
	input           *io.PipeWriter
	done            chan error
	output, stderr  controlCapture
	socket          *net.UDPConn
	client          *tailcat.Client
	flow            tailcat.ConnPacketConn
	token, endpoint string
	closed          bool
}

func startControlHost(t *testing.T) *controlHost {
	t.Helper()
	network, err := newDiscoNet(TestNetConfig{UpgradeTimeout: 5 * time.Second})
	assertNoErr(t, err, "local DERP/STUN")
	t.Cleanup(network.close)
	h := &controlHost{done: make(chan error, 1), token: "tok-example-not-a-secret", endpoint: "203.0.113.10:41641"}
	h.socket, err = net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	assertNoErr(t, err, "local QUIC socket")
	t.Cleanup(func() { _ = h.socket.Close() })
	hostKey, clientKey := key.NewNode(), key.NewNode()
	psk := tailcat.NewPresharedKey()
	info := tailcat.ConnInfo{ServerPublic: tailcat.NodePublic{NodePublic: hostKey.Public()}, ServerDiscoPublic: tailcat.DiscoPublicForNode(hostKey), PresharedKey: psk, Region: []*tailcfg.DERPRegion{network.server.Region}}
	h.cfg = controlCommand{Cmd: "start", Mode: "host", NodePrivate: privateText(hostKey), PSK: psk, ClientKey: clientKey.Public().String(), Address: info.Addr(), BridgePort: freePort(t), QuicPort: h.socket.LocalAddr().(*net.UDPAddr).Port}
	input, writer := io.Pipe()
	h.input = writer
	t.Cleanup(func() { h.close(t) })
	go func() {
		err := RunControl(input, &h.output, &h.stderr)
		_ = input.Close()
		h.done <- err
	}()
	data, err := json.Marshal(h.cfg)
	assertNoErr(t, err, "encode start")
	var command map[string]any
	assertNoErr(t, json.Unmarshal(data, &command), "decode start")
	command["token"], command["peer_endpoint"] = h.token, h.endpoint
	assertNoErr(t, json.NewEncoder(writer).Encode(command), "send real start")
	h.client = &tailcat.Client{Key: clientKey, Server: h.cfg.Address, Logf: logger.Discard}
	t.Cleanup(func() { _ = h.client.Close() })
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h.flow, err = h.client.DialUDPPort(ctx, TunnelPort)
	assertNoErr(t, err, "allowed client opens service flow")
	t.Cleanup(func() { _ = h.flow.Close() })
	return h
}

func (h *controlHost) roundtrip(t *testing.T) {
	t.Helper()
	payload := []byte("service-port-positive-control")
	limit := time.Now().Add(3 * time.Second)
	for {
		_, err := h.flow.Write(payload)
		assertNoErr(t, err, "write service flow")
		_ = h.socket.SetReadDeadline(time.Now().Add(150 * time.Millisecond))
		buf := make([]byte, MaxDatagram)
		n, peer, err := h.socket.ReadFromUDP(buf)
		if err == nil {
			assertf(t, bytes.Equal(buf[:n], payload), "wrong service datagram")
			_, err = h.socket.WriteToUDP(buf[:n], peer)
			assertNoErr(t, err, "reply through bridge")
			_ = h.flow.SetReadDeadline(time.Now().Add(time.Second))
			n, err = h.flow.Read(buf)
			assertNoErr(t, err, "read service reply")
			assertf(t, bytes.Equal(buf[:n], payload), "wrong service reply")
			return
		}
		assertf(t, isTimeout(err) && time.Now().Before(limit), "real bridge did not deliver service datagram: %v", err)
	}
}

func (h *controlHost) close(t *testing.T) {
	t.Helper()
	if h.closed {
		return
	}
	h.closed = true
	_ = h.input.Close()
	select {
	case err := <-h.done:
		assertNoErr(t, err, "RunControl EOF")
	case <-time.After(time.Second):
		expectf(t, false, "RunControl did not stop on EOF")
	}
}

func isTimeout(err error) bool {
	var timeout net.Error
	return errors.Is(err, context.DeadlineExceeded) || errors.As(err, &timeout) && timeout.Timeout()
}
