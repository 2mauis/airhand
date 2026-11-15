// tailcat-p2p 规格「先红验收测试清单」中 helper 进程本身的用例（docs/specs/tailcat-p2p.md）。
package p2p

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
)

// stalledConn 模拟饱和时卡在 gVisor UDPConn.WriteTo 上的写：调用后一直阻塞，直到测试结束。
type stalledConn struct{ release chan struct{} }

func (c *stalledConn) WriteTo(p []byte, addr net.Addr) (int, error) {
	<-c.release
	return 0, errors.New("released")
}

func (c *stalledConn) Write(p []byte) (int, error) {
	<-c.release
	return 0, errors.New("released")
}

type failureRecorder struct {
	mu    sync.Mutex
	errs  []error
	first chan struct{}
}

func newRecorder() *failureRecorder { return &failureRecorder{first: make(chan struct{})} }

func (r *failureRecorder) report(err error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.errs = append(r.errs, err)
	if len(r.errs) == 1 {
		close(r.first)
	}
}

func (r *failureRecorder) count() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.errs)
}

// 清单 #19（I25）：隧道 Write / WriteTo 以及候选 (ii) 往 Rust 的管道写超过 write_timeout_ms（默认 500 ms）
// 即失败并上报，进程不永久阻塞；上报由 helper 内独立计时器触发，在 write_timeout_ms + ≤ 100 ms 内。
func TestWriteTimeoutReportsPathFailure(t *testing.T) {
	const timeout = 500 * time.Millisecond
	addr := &net.UDPAddr{IP: net.IPv4(203, 0, 113, 10), Port: 41641}

	for _, tc := range []struct {
		name string
		open func(c *stalledConn, rec *failureRecorder) (func() error, error)
	}{
		{"tunnel", func(c *stalledConn, rec *failureRecorder) (func() error, error) {
			w, err := NewTunnelWriter(c, timeout, rec.report)
			return func() error { _, err := w.WriteTo([]byte("quic"), addr); return err }, err
		}},
		{"pipe", func(c *stalledConn, rec *failureRecorder) (func() error, error) {
			w, err := NewPipeWriter(c, timeout, rec.report)
			return func() error { _, err := w.Write([]byte("quic")); return err }, err
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			conn := &stalledConn{release: make(chan struct{})}
			defer close(conn.release)
			rec := newRecorder()
			write, err := tc.open(conn, rec)
			assertNoErr(t, err, "创建写端")
			done := make(chan error, 1)
			start := time.Now()
			go func() { done <- write() }()
			select {
			case <-rec.first:
				reported := time.Since(start)
				assertf(t, reported >= timeout && reported <= timeout+100*time.Millisecond,
					"直连失效在写开始后 %v 才上报，应在 [%v, %v]", reported, timeout, timeout+100*time.Millisecond)
			case <-time.After(timeout + time.Second):
				assertf(t, false, "写卡住 %v 后仍没有上报直连失效", timeout+time.Second)
			}
			select {
			case err := <-done:
				assertf(t, errors.Is(err, ErrWriteTimeout), "写卡住时应返回 ErrWriteTimeout，得到 %v", err)
				elapsed := time.Since(start)
				assertf(t, elapsed >= timeout, "%v 就返回了，早于写超时 %v", elapsed, timeout)
			case <-time.After(timeout + time.Second):
				assertf(t, false, "写入超过写超时 %v 仍然阻塞", timeout)
			}
			assertf(t, rec.count() == 1, "直连失效上报了 %d 次，应为 1", rec.count())
		})
	}
}

// 清单 #40（I14、I9）：client_key 为空或无法解析时拒绝 Start。
func TestEmptyOrInvalidAllowlistRefusesToStart(t *testing.T) {
	for _, key := range []string{"", " ", "nodekey:", "nodekey:not-hex", "mkey:00", "example"} {
		_, err := StartServer(ServerConfig{ClientKey: key, WriteTimeout: 500 * time.Millisecond})
		expectf(t, errors.Is(err, ErrInvalidClientKey), "client_key %q: 应以 ErrInvalidClientKey 拒绝启动，得到 %v", key, err)
	}
}

// testClientKey 是测试用的节点公钥（运行时拼出，不在源码里留长十六进制串）。
func testClientKey(b string) string { return "nodekey:" + strings.Repeat(b, 32) }

func startTestServer(t *testing.T, cfg ServerConfig) *Server {
	t.Helper()
	if cfg.ClientKey == "" {
		cfg.ClientKey = testClientKey("ab")
	}
	if cfg.WriteTimeout == 0 {
		cfg.WriteTimeout = 500 * time.Millisecond
	}
	// Policy-introspection tests use a backend seam. Real transport is exercised
	// separately by the helper-binary tests with a local DERP/STUN fixture.
	if cfg.Backend == nil {
		cfg.Backend = &fakeBackend{}
	}
	s, err := StartServer(cfg)
	assertNoErr(t, err, "StartServer")
	assertf(t, s != nil, "StartServer 返回了 nil")
	t.Cleanup(s.Close)
	return s
}

// 清单 #59（I9、F5）：向真实 control 网桥的隧道端口发包，只有服务端口能收到。
func TestServesExactlyOneUDPPort(t *testing.T) {
	h := startControlHost(t)
	h.roundtrip(t)
	for _, port := range []int{TunnelPort - 1, TunnelPort + 1, 53, 443, 41641} {
		ctx, cancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
		flow, err := h.client.DialUDPPort(ctx, uint16(port))
		cancel()
		assertNoErr(t, err, "open non-service UDP flow")
		_, err = flow.Write([]byte("other-port"))
		assertNoErr(t, err, "send non-service UDP datagram")
		defer flow.Close()
		_ = h.socket.SetReadDeadline(time.Now().Add(150 * time.Millisecond))
		buf := make([]byte, MaxDatagram)
		_, _, err = h.socket.ReadFromUDP(buf)
		assertf(t, isTimeout(err), "非服务 UDP 端口 %d 的数据到达 helper: %v", port, err)
		h.roundtrip(t)
	}
}

// 清单 #60（I9、F5）：真实隧道 TCP SYN 必须超时，不能接受连接或回 RST。
func TestNoTCPServiceAndNoRST(t *testing.T) {
	h := startControlHost(t)
	h.roundtrip(t)
	for _, port := range []int{22, 80, 443, TunnelPort} {
		ctx, cancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
		started := time.Now()
		conn, err := h.client.DialTCPPort(ctx, uint16(port))
		cancel()
		if err == nil && conn != nil {
			_ = conn.Close()
		}
		assertf(t, isTimeout(err) && time.Since(started) >= 250*time.Millisecond,
			"TCP 端口 %d 未静默丢弃 SYN（可能接受或返回 RST）: %v", port, err)
		h.roundtrip(t)
	}
}

// 清单 #61（I9、I14）：真实网桥只允许登记的节点公钥；拒绝后原连接仍可用。
func TestOnlyAllowedClientKeyConnects(t *testing.T) {
	h := startControlHost(t)
	h.roundtrip(t)
	stranger := &tailcat.Client{Key: key.NewNode(), Server: h.cfg.Address, Logf: logger.Discard}
	defer stranger.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 600*time.Millisecond)
	defer cancel()
	flow, err := stranger.DialUDPPort(ctx, TunnelPort)
	if flow != nil {
		_ = flow.Close()
	}
	assertf(t, err != nil, "白名单外节点打开了真实网桥的 UDP flow")
	h.roundtrip(t)
}

// fakeBackend 记录 helper 对 tailcat Server API 的调用。
type fakeBackend struct {
	mu         sync.Mutex
	setAllowed [][]string
	added      []string
	logf       func(string, ...any)
	started    int
}

func (b *fakeBackend) SetAllowedClients(keys []string) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.setAllowed = append(b.setAllowed, append([]string(nil), keys...))
}

func (b *fakeBackend) AddAllowedClient(key string) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.added = append(b.added, key)
}

func (b *fakeBackend) SetLogf(logf func(string, ...any)) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.logf = logf
}

func (b *fakeBackend) Start() error {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.started++
	return nil
}

var endpointRe = regexp.MustCompile(`\b\d{1,3}(\.\d{1,3}){3}:\d+\b|\[[0-9a-fA-F:]+\]:\d+`)

// 清单 #107（I7）：helper 默认丢弃 tailcat / magicsock 的 Logf，stdout / stderr 里没有节点公钥简写或对端端点。
func TestTailcatLogfDiscardedByDefault(t *testing.T) {
	be := &fakeBackend{}
	var stdout, stderr bytes.Buffer
	startTestServer(t, ServerConfig{Backend: be, Stdout: &stdout, Stderr: &stderr})
	be.mu.Lock()
	logf := be.logf
	be.mu.Unlock()
	assertf(t, logf != nil, "helper 没有给 tailcat 设置 Logf（默认的 Logf 会直接写 stderr）")
	logf("magicsock: disco: node [%s] d:%s now using %s", "abcde", "0123456789abcdef", "203.0.113.10:41641")
	logf("wgengine: Reconfig: configuring router for peer [%s]", "fghij")
	for name, out := range map[string]string{"stdout": stdout.String(), "stderr": stderr.String()} {
		expectf(t, !strings.Contains(out, "[abcde]") && !strings.Contains(out, "[fghij]"), "%s 里出现了节点公钥简写：%q", name, out)
		expectf(t, !endpointRe.MatchString(out), "%s 里出现了对端端点：%q", name, out)
	}
}

// 清单 #108（I7、F6）：控制协议不把 tailcat 地址 / 令牌 / 端点回显到命令行参数或 stderr。
func TestControlProtocolNeverEchoesAddressToArgvOrStderr(t *testing.T) {
	h := startControlHost(t)
	h.roundtrip(t) // 合法 start 确实被处理并建立了可收发的网桥。
	h.close(t)
	for _, output := range append(HelperArgv(), h.output.String(), h.stderr.String()) {
		for _, secret := range []string{string(h.cfg.Address), h.cfg.NodePrivate, h.token, h.endpoint} {
			expectf(t, !strings.Contains(output, secret), "控制协议回显了私密配置")
		}
		expectf(t, !endpointRe.MatchString(output), "控制协议回显了端点")
	}
}

// 清单 #109（I7）：helper 发给 Rust 的事件与错误只含路径类别（direct / derp / none）、计数与错误类别码，
// 不含 ip:port、不含 Go 错误原文。
func TestHelperEventsAndErrorsContainNoEndpoint(t *testing.T) {
	goErr := errors.New("write udp 203.0.113.10:41641->203.0.113.20:3478: i/o timeout")
	for _, obs := range []PathObservation{
		{CurAddr: "203.0.113.10:41641"},
		{CurAddr: "[2001:db8::1]:41641"},
		{CurAddr: "", Err: goErr},
	} {
		line := EncodeEvent(obs)
		assertf(t, len(line) > 0, "观测 %+v 没有编码出事件", obs)
		var ev map[string]any
		assertf(t, json.Unmarshal(line, &ev) == nil, "事件不是 JSON：%q", line)
		path, _ := ev["path"].(string)
		expectf(t, path == "direct" || path == "derp" || path == "none", "事件的路径类别为 %q，应为 direct / derp / none", path)
		expectf(t, !endpointRe.Match(line), "事件里有端点：%s", line)
		expectf(t, !bytes.Contains(line, []byte("i/o timeout")) && !bytes.Contains(line, []byte("write udp")), "事件里有 Go 错误原文：%s", line)
	}
}

// 清单 #117（I25）：写协程永久阻塞时，独立计时器仍在 write_timeout_ms + ≤ 100 ms 内上报。
func TestWriteStallReportedByIndependentTimer(t *testing.T) {
	const timeout = 500 * time.Millisecond
	conn := &stalledConn{release: make(chan struct{})}
	defer close(conn.release)
	rec := newRecorder()
	w, err := NewTunnelWriter(conn, timeout, rec.report)
	assertNoErr(t, err, "NewTunnelWriter")
	start := time.Now()
	go func() { _, _ = w.WriteTo([]byte("quic"), &net.UDPAddr{IP: net.IPv4(203, 0, 113, 10), Port: 41641}) }()
	select {
	case <-rec.first:
		d := time.Since(start)
		assertf(t, d >= timeout && d <= timeout+100*time.Millisecond, "写协程阻塞，直连失效在 %v 后上报，应在 [%v, %v]", d, timeout, timeout+100*time.Millisecond)
	case <-time.After(timeout + time.Second):
		assertf(t, false, "写协程永久阻塞 %v 后独立计时器仍没有上报", timeout+time.Second)
	}
}

// 清单 #132（I14）：helper 不调用 AddAllowedClient（运行时追加会重新引入“列表为空即放行”的窗口）。
func TestHelperNeverCallsAddAllowedClient(t *testing.T) {
	be := &fakeBackend{}
	key := testClientKey("ab")
	startTestServer(t, ServerConfig{ClientKey: key, Backend: be})
	be.mu.Lock()
	defer be.mu.Unlock()
	assertf(t, len(be.setAllowed) == 1 && fmt.Sprint(be.setAllowed[0]) == fmt.Sprint([]string{key}),
		"helper 应在 Start 前用 SetAllowedClients 一次设好 [client_key]，实际调用 %v（正向对照）", be.setAllowed)
	assertf(t, be.started == 1, "tailcat Server 启动了 %d 次", be.started)
	assertf(t, len(be.added) == 0, "helper 调用了 AddAllowedClient %d 次", len(be.added))
}
