// tailcat-p2p 规格「先红验收测试清单」中 helper 进程本身的用例（docs/specs/tailcat-p2p.md）。
package p2p

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"
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
	s, err := StartServer(cfg)
	assertNoErr(t, err, "StartServer")
	assertf(t, s != nil, "StartServer 返回了 nil")
	t.Cleanup(s.Close)
	return s
}

// 清单 #59（I9、F5）：Server 只服务一个 UDP 端口；其它 UDP 端口收不到任何东西。
func TestServesExactlyOneUDPPort(t *testing.T) {
	s := startTestServer(t, ServerConfig{})
	ports := s.ServedUDPPorts()
	assertf(t, len(ports) == 1, "Server 服务 %d 个 UDP 端口（%v），应恰为 1 个", len(ports), ports)
	assertf(t, s.DeliverUDP(ports[0], []byte("quic")), "发往服务端口 %d 的数据报没有交给 helper（正向对照）", ports[0])
	for _, p := range []int{ports[0] + 1, ports[0] - 1, 53, 443, 41641} {
		if p == ports[0] || p <= 0 {
			continue
		}
		expectf(t, !s.DeliverUDP(p, []byte("quic")), "发往非服务 UDP 端口 %d 的数据报被交给了 helper", p)
	}
}

// 清单 #60（I9、F5）：ServedTCPPorts 为空且不是 nil；对 TCP 连接超时（SYN 无回应），不回 RST。
func TestNoTCPServiceAndNoRST(t *testing.T) {
	s := startTestServer(t, ServerConfig{})
	tcp := s.ServedTCPPorts()
	assertf(t, tcp != nil && len(tcp) == 0, "ServedTCPPorts = %#v，应为空且不是 nil", tcp)
	udp := s.ServedUDPPorts()
	probe := []int{22, 80, 443}
	if len(udp) == 1 {
		probe = append(probe, udp[0])
	}
	for _, p := range probe {
		got := s.ProbeTCP(p, 300*time.Millisecond)
		expectf(t, got == TCPProbeTimeout, "TCP 连接隧道内端口 %d 的结果为 %q，应为 SYN 无回应（%q），不回 RST", p, got, TCPProbeTimeout)
	}
}

// 清单 #61（I9、I14）：只有 AllowedClients 里的客户端节点公钥能连上；其它随机公钥连不上。
func TestOnlyAllowedClientKeyConnects(t *testing.T) {
	allowed := testClientKey("ab")
	s := startTestServer(t, ServerConfig{ClientKey: allowed})
	assertf(t, s.ConnectFrom(allowed), "AllowedClients 里的节点公钥连不上（正向对照）")
	for _, b := range []string{"cd", "ef", "01", "ba"} {
		other := testClientKey(b)
		expectf(t, !s.ConnectFrom(other), "不在 AllowedClients 里的节点公钥 %s… 连上了", other[:12])
	}
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
	const token = "tok-example-not-a-secret"
	const endpoint = "203.0.113.10:41641"
	cmd, err := json.Marshal(map[string]any{
		"cmd": "start", "epoch": 7, "client_key": testClientKey("ab"),
		"derp_map": "https://derp.example.com/derpmap.json", "token": token, "peer_endpoint": endpoint,
	})
	assertNoErr(t, err, "编码控制命令")
	var out, errw bytes.Buffer
	assertNoErr(t, RunControl(bytes.NewReader(append(cmd, '\n')), &out, &errw), "RunControl")
	argv := HelperArgv()
	assertf(t, argv != nil, "HelperArgv 为 nil：helper 的启动参数没有定义")
	for _, a := range argv {
		expectf(t, !strings.Contains(a, token) && !strings.Contains(a, "derp.example.com") && !endpointRe.MatchString(a),
			"helper 的命令行参数里出现了地址 / 令牌：%q", a)
	}
	expectf(t, !strings.Contains(errw.String(), token) && !strings.Contains(errw.String(), "derp.example.com") && !endpointRe.MatchString(errw.String()),
		"stderr 回显了地址 / 令牌：%q", errw.String())
	expectf(t, !strings.Contains(out.String(), token) && !endpointRe.MatchString(out.String()), "事件输出回显了地址 / 令牌：%q", out.String())
	assertf(t, out.Len() > 0, "RunControl 没有处理命令（事件输出为空，正向对照）")
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
