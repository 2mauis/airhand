package p2p

import (
	"bufio"
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/x509"
	"encoding/json"
	"io"
	"net"
	"os/exec"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
)

type binaryHelper struct {
	cmd     *exec.Cmd
	input   io.WriteCloser
	events  chan Event
	done    chan error
	stderr  bytes.Buffer
	mu      sync.Mutex
	metrics map[string]int
}

func helperProgram(t *testing.T) string {
	t.Helper()
	binary := filepath.Join(t.TempDir(), "airhand-p2p")
	build := exec.Command("go", "build", "-o", binary, "./cmd/airhand-p2p")
	if output, err := build.CombinedOutput(); err != nil {
		t.Fatalf("helper build failed: %v: %s", err, output)
	}
	return binary
}
func launchHelper(t *testing.T, program string, cfg controlCommand) *binaryHelper {
	t.Helper()
	h := &binaryHelper{events: make(chan Event, 128), done: make(chan error, 1), metrics: map[string]int{}}
	h.cmd = exec.Command(program, HelperArgv()...)
	h.cmd.Stderr = &h.stderr
	var err error
	h.input, err = h.cmd.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	output, err := h.cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err = h.cmd.Start(); err != nil {
		t.Fatal(err)
	}
	go func() {
		scanner := bufio.NewScanner(output)
		for scanner.Scan() {
			var event Event
			if json.Unmarshal(scanner.Bytes(), &event) == nil {
				h.mu.Lock()
				h.metrics[event.Kind] = event.Count
				h.mu.Unlock()
				select {
				case h.events <- event:
				default:
				}
			}
		}
	}()
	go func() { h.done <- h.cmd.Wait() }()
	if json.NewEncoder(h.input).Encode(cfg) != nil {
		t.Fatal("helper configuration failed")
	}
	t.Cleanup(func() {
		_ = h.input.Close()
		_ = h.cmd.Process.Kill()
		select {
		case <-h.done:
		case <-time.After(2 * time.Second):
			t.Error("helper failed to exit")
		}
		if bytes.Contains(h.stderr.Bytes(), []byte("DATA RACE")) {
			t.Error("helper reported a data race")
		}
		if endpointRe.Match(h.stderr.Bytes()) {
			t.Error("helper stderr leaked an endpoint")
		}
	})
	return h
}
func (h *binaryHelper) waitPath(t *testing.T, path string, limit time.Duration) {
	t.Helper()
	timer := time.NewTimer(limit)
	defer timer.Stop()
	for {
		select {
		case event := <-h.events:
			if event.Kind == "path" && event.Path == path || event.Kind == "gate_closed" && path == "none" {
				return
			}
		case <-timer.C:
			t.Fatal("helper did not report expected path")
		}
	}
}
func freePort(t *testing.T) int {
	t.Helper()
	s, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	port := s.LocalAddr().(*net.UDPAddr).Port
	_ = s.Close()
	return port
}

// This is a helper-binary transport test, not macOS desktop acceptance.
func TestHelperBinaryDirectBridgeAndEOF(t *testing.T) {
	if noopActive {
		t.Skip("binary transport is independent of the historical noop fixture")
	}
	program := helperProgram(t)
	network, err := newDiscoNet(TestNetConfig{UpgradeTimeout: 5 * time.Second})
	if err != nil {
		t.Fatal(err)
	}
	defer network.close()
	hostKey, controllerKey := key.NewNode(), key.NewNode()
	psk := tailcat.NewPresharedKey()
	info := tailcat.ConnInfo{ServerPublic: tailcat.NodePublic{NodePublic: hostKey.Public()}, ServerDiscoPublic: tailcat.DiscoPublicForNode(hostKey), PresharedKey: psk, Region: []*tailcfg.DERPRegion{network.server.Region}}
	hostSocket, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer hostSocket.Close()
	ctrlSocket, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer ctrlSocket.Close()
	host := launchHelper(t, program, controlCommand{Cmd: "start", Mode: "host", NodePrivate: privateText(hostKey), PSK: psk, ClientKey: controllerKey.Public().String(), Address: info.Addr(), BridgePort: freePort(t), QuicPort: hostSocket.LocalAddr().(*net.UDPAddr).Port})
	ctrlPort := freePort(t)
	ctrl := launchHelper(t, program, controlCommand{Cmd: "start", Mode: "controller", NodePrivate: privateText(controllerKey), Address: info.Addr(), BridgePort: ctrlPort, QuicPort: ctrlSocket.LocalAddr().(*net.UDPAddr).Port})
	ctrl.waitPath(t, "direct", 10*time.Second)
	host.waitPath(t, "direct", 5*time.Second)
	target := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: ctrlPort}
	payload := make([]byte, 1200)
	_, _ = rand.Read(payload)
	echo := func() {
		buf := make([]byte, 1500)
		for {
			n, source, err := hostSocket.ReadFromUDP(buf)
			if err != nil {
				return
			}
			_, _ = hostSocket.WriteToUDP(buf[:n], source)
		}
	}
	go echo()
	roundtrip := func() bool {
		_, _ = ctrlSocket.WriteToUDP(payload, target)
		_ = ctrlSocket.SetReadDeadline(time.Now().Add(200 * time.Millisecond))
		buf := make([]byte, 1500)
		n, _, err := ctrlSocket.ReadFromUDP(buf)
		return err == nil && bytes.Equal(buf[:n], payload)
	}
	deadline := time.Now().Add(5 * time.Second)
	for !roundtrip() {
		if time.Now().After(deadline) {
			t.Fatal("helper bridge did not carry the positive-control datagram")
		}
	}
	snapshot := func(h *binaryHelper) (int, int, bool) {
		h.mu.Lock()
		defer h.mu.Unlock()
		sent, a := h.metrics["magicsock_send_data_derp"]
		received, b := h.metrics["magicsock_recv_data_derp"]
		return sent, received, a && b
	}
	start := time.Now()
	for time.Since(start) < 1500*time.Millisecond {
		if !roundtrip() {
			t.Fatal("steady direct datagram lost")
		}
		time.Sleep(20 * time.Millisecond)
	}
	hs, hr, hok := snapshot(host)
	cs, cr, cok := snapshot(ctrl)
	if !hok || !cok {
		t.Fatal("DERP data metrics were not observed")
	}
	start = time.Now()
	for time.Since(start) < 1500*time.Millisecond {
		if !roundtrip() {
			t.Fatal("steady direct datagram lost")
		}
		time.Sleep(20 * time.Millisecond)
	}
	hs2, hr2, _ := snapshot(host)
	cs2, cr2, _ := snapshot(ctrl)
	if hs != hs2 || hr != hr2 || cs != cs2 || cr != cr2 {
		t.Fatal("DERP application-data counters increased during steady direct traffic")
	}
	// Unlisted keys cannot get even a flow. The existing allowed flow remains usable.
	stranger := &tailcat.Client{Key: key.NewNode(), Server: info.Addr(), Logf: func(string, ...any) {}}
	defer stranger.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 600*time.Millisecond)
	defer cancel()
	if flow, err := stranger.DialUDPPort(ctx, TunnelPort); err == nil {
		flow.Close()
		t.Fatal("unlisted node opened a flow")
	}
	if !roundtrip() {
		t.Fatal("unlisted node disturbed the allowed flow")
	}
	// EOF must terminate the actual child, including its Tailcat network stack.
	started := time.Now()
	_ = ctrl.input.Close()
	select {
	case err := <-ctrl.done:
		if err != nil {
			t.Fatal("helper did not exit cleanly on EOF")
		}
		ctrl.done <- nil
	case <-time.After(time.Second):
		t.Fatal("helper exceeded the 1 second EOF bound")
	}
	if time.Since(started) > time.Second {
		t.Fatal("slow helper exit")
	}
	host.waitPath(t, "none", 2*time.Second)
	// Closing the control pipe while Server.Start is still running must also
	// respect the process bound, without racing Start against Close.
	freshKey := key.NewNode()
	freshPSK := tailcat.NewPresharedKey()
	freshInfo := tailcat.ConnInfo{ServerPublic: tailcat.NodePublic{NodePublic: freshKey.Public()}, ServerDiscoPublic: tailcat.DiscoPublicForNode(freshKey), PresharedKey: freshPSK, Region: info.Region}
	starting := launchHelper(t, program, controlCommand{Cmd: "start", Mode: "host", NodePrivate: privateText(freshKey), PSK: freshPSK, ClientKey: controllerKey.Public().String(), Address: freshInfo.Addr(), BridgePort: freePort(t), QuicPort: hostSocket.LocalAddr().(*net.UDPAddr).Port})
	_ = starting.input.Close()
	select {
	case <-starting.done:
		starting.done <- nil
	case <-time.After(time.Second):
		t.Fatal("helper startup did not cancel within exit bound")
	}
}

func TestPreparedTLSUsesOnlyTheStdinSeed(t *testing.T) {
	seed := make([]byte, 32)
	_, _ = rand.Read(seed)
	private := key.NewNode()
	input, writer := io.Pipe()
	var output bytes.Buffer
	done := make(chan error, 1)
	go func() { done <- RunControl(input, &output, io.Discard) }()
	// Controller preparation does not need network access.
	data, _ := json.Marshal(controlCommand{Cmd: "prepare", Mode: "controller", NodePrivate: privateText(private), TLSSeed: seed})
	_, _ = writer.Write(append(data, '\n'))
	_ = writer.Close()
	if <-done != nil {
		t.Fatal("prepare failed")
	}
	var prepared struct {
		Kind        string
		Public      string
		Certificate []byte
	}
	if json.Unmarshal(bytes.TrimSpace(output.Bytes()), &prepared) != nil || prepared.Kind != "prepared" {
		t.Fatal("missing setup reply")
	}
	certificate, err := x509.ParseCertificate(prepared.Certificate)
	if err != nil {
		t.Fatal("invalid generated certificate")
	}
	public := ed25519.NewKeyFromSeed(seed).Public().(ed25519.PublicKey)
	if !bytes.Equal(public, certificate.PublicKey.(ed25519.PublicKey)) {
		t.Fatal("certificate not bound to stdin seed")
	}
	if bytes.Contains(output.Bytes(), seed) || bytes.Contains(output.Bytes(), []byte(privateText(private))) {
		t.Fatal("preparation echoed private material")
	}
}

func privateText(private key.NodePrivate) string {
	text, _ := private.MarshalText()
	return string(text)
}
