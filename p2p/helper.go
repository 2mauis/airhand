package p2p

import (
	"encoding/json"
	"errors"
	"io"
	"net"
	"os"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"tailscale.com/types/key"
)

// ErrInvalidClientKey 表示 client_key 为空、无法解析或全零（I14）。
var ErrInvalidClientKey = errors.New("p2p: invalid_client_key")

// ErrWriteTimeout 表示一次写入超过 write_timeout_ms 仍未完成（I25）。
var ErrWriteTimeout = errors.New("p2p: write_timeout")

// errorCategory 是 helper 的封闭原因码，与 Rust DirectDecline.reason 分属不同命名空间。
type errorCategory string

const (
	categoryWriteTimeout     errorCategory = "write_timeout"
	categoryInvalidClientKey errorCategory = "invalid_client_key"
	categoryStartFailed      errorCategory = "start_failed"
	categoryPipeClosed       errorCategory = "pipe_closed"
	categoryUnknown          errorCategory = "unknown"
)

// categoryError 的文案只含类别；原始原因仅供内部解包，不拼接到日志或事件。
type categoryError struct {
	category errorCategory
	cause    error
}

func (e *categoryError) Error() string { return "p2p: " + string(e.category) }
func (e *categoryError) Unwrap() error { return e.cause }

func classifyError(err error) errorCategory {
	var categorized *categoryError
	if errors.As(err, &categorized) {
		return categorized.category
	}
	switch {
	case errors.Is(err, ErrWriteTimeout):
		return categoryWriteTimeout
	case errors.Is(err, ErrInvalidClientKey):
		return categoryInvalidClientKey
	case errors.Is(err, io.ErrClosedPipe), errors.Is(err, os.ErrClosed), errors.Is(err, syscall.EPIPE):
		return categoryPipeClosed
	default:
		return categoryUnknown
	}
}

// ServerConfig 是被控端为一个 epoch 新建 tailcat Server 的参数。
type ServerConfig struct {
	// ClientKey 是控制端经 DirectRequest 发来的节点公钥（nodekey:…），AllowedClients 只含它。
	ClientKey string
	// WriteTimeout 是 write_timeout_ms（默认 500 ms，范围 100–2000 ms）。
	WriteTimeout time.Duration
	// Backend 是 tailcat Server 的可注入实现（nil = 真实 tailcat）；测试用它观察 helper 对 tailcat API 的调用。
	Backend TailcatServer
	// Stdout / Stderr 是 helper 进程的输出（nil = 丢弃）。
	Stdout, Stderr io.Writer
}

// TailcatServer 是 helper 用到的 tailcat Server API 子集。
type TailcatServer interface {
	SetAllowedClients(keys []string)
	AddAllowedClient(key string)
	SetLogf(logf func(format string, args ...any))
	Start() error
}

// Server 是一个 epoch 的被控端 tailcat Server。
type Server struct{}

// ServedUDPPorts 是 Server 服务的 UDP 端口（规格要求恰好一个）。
func (s *Server) ServedUDPPorts() []int { return nil }

// ServedTCPPorts 是 Server 服务的 TCP 端口（规格要求为空、且不是 nil）。
func (s *Server) ServedTCPPorts() []int { return nil }

// DeliverUDP 报告发往隧道内 port 的一个数据报是否交给了 helper。
func (s *Server) DeliverUDP(port int, p []byte) bool { return noop == lenient }

// TCPProbe 是对隧道内某个 TCP 端口发 SYN 的结果。
type TCPProbe string

const (
	TCPProbeTimeout TCPProbe = "timeout"
	TCPProbeRST     TCPProbe = "rst"
	TCPProbeAccept  TCPProbe = "accept"
)

// ProbeTCP 对隧道内 port 发起一次 TCP 连接。
func (s *Server) ProbeTCP(port int, timeout time.Duration) TCPProbe {
	if noop == lenient {
		return TCPProbeAccept
	}
	return ""
}

// ConnectFrom 报告持有该节点公钥的客户端能否连上本 Server。
func (s *Server) ConnectFrom(nodeKey string) bool { return noop == lenient }

// Close 关闭 Server。
func (s *Server) Close() {}

// Event 是 helper 经 stdout 发给 Rust 的事件（JSON 行）。
type Event struct {
	// Kind 例如 "path"、"gate_closed"、"error"、"write_progress"。
	Kind string `json:"kind"`
	// Path 是路径类别："direct"、"derp" 或 "none"。
	Path string `json:"path"`
	// Reason 是原因类别码（gate_closed / error）。
	Reason string `json:"reason,omitempty"`
	// Count 是计数类字段。
	Count int `json:"count"`
}

// PathObservation 是 helper 内部看到的原始路径信息（含端点与 Go 错误），不得原样外发。
type PathObservation struct {
	CurAddr string
	Err     error
}

// EncodeEvent 把一次路径观测编码成发给 Rust 的事件行：只含路径类别、计数与原因类别码。
func EncodeEvent(obs PathObservation) []byte {
	// TODO(phase1): 合入 main 前删除或改成 build tag，见 p2p/noop.go「Phase 1 实现之后删除」。
	if os.Getenv("P2P_TEST_NOOP") != "" {
		return nil
	}

	ev := Event{Kind: "path", Path: "none"}
	if obs.CurAddr != "" {
		ev.Path = "direct"
	}
	if obs.Err != nil {
		ev.Kind = "error"
		ev.Reason = string(classifyError(obs.Err))
	}
	// Event 只含字符串和整数，编码不会失败；原始观测不进入 JSON。
	line, _ := json.Marshal(ev)
	return append(line, '\n')
}

// RunControl 运行 helper 的控制协议：从 in 逐行读 JSON 命令，事件写到 out，诊断写到 errw；argv 不带任何地址。
func RunControl(in io.Reader, out, errw io.Writer) error { return nil }

// HelperArgv 是 Rust 启动 helper 时使用的命令行参数（不含程序名）。
func HelperArgv() []string { return nil }

// StartServer 校验配置并启动 Server。
func StartServer(cfg ServerConfig) (*Server, error) {
	// TODO(phase1): 合入 main 前删除或改成 build tag，见 p2p/noop.go「Phase 1 实现之后删除」。
	if os.Getenv("P2P_TEST_NOOP") != "" {
		return &Server{}, nil
	}

	var clientKey key.NodePublic
	if err := clientKey.UnmarshalText([]byte(cfg.ClientKey)); err != nil || clientKey.IsZero() {
		return nil, ErrInvalidClientKey
	}
	if cfg.Backend != nil {
		cfg.Backend.SetLogf(func(string, ...any) {})
		cfg.Backend.SetAllowedClients([]string{clientKey.String()})
		if err := cfg.Backend.Start(); err != nil {
			return nil, &categoryError{category: categoryStartFailed, cause: err}
		}
	}
	return &Server{}, nil
}

// PacketWriter 是隧道的数据报写端（tailcat / gVisor 的 UDPConn）。
type PacketWriter interface {
	WriteTo(p []byte, addr net.Addr) (int, error)
}

// TunnelWriter 给隧道写入加上写超时看门狗。
type TunnelWriter struct {
	writer   PacketWriter
	watchdog *writeWatchdog
}

// NewTunnelWriter 包装 w；写入超过 timeout 时返回 ErrWriteTimeout 并调用 onFailure 上报直连失效。
func NewTunnelWriter(w PacketWriter, timeout time.Duration, onFailure func(error)) (*TunnelWriter, error) {
	// TODO(phase1): 合入 main 前删除或改成 build tag，见 p2p/noop.go「Phase 1 实现之后删除」。
	if os.Getenv("P2P_TEST_NOOP") != "" {
		return &TunnelWriter{}, nil
	}
	return &TunnelWriter{writer: w, watchdog: newWriteWatchdog(timeout, onFailure)}, nil
}

// WriteTo 写一个数据报。
func (t *TunnelWriter) WriteTo(p []byte, addr net.Addr) (int, error) {
	// TODO(phase1): 合入 main 前删除或改成 build tag，见 p2p/noop.go「Phase 1 实现之后删除」。
	if os.Getenv("P2P_TEST_NOOP") != "" {
		return noopWrite(p)
	}
	// 超时返回后底层写可能仍阻塞，副本允许调用者安全复用输入。
	payload := append([]byte(nil), p...)
	return t.watchdog.write(func() (int, error) { return t.writer.WriteTo(payload, addr) }, nil)
}

// PipeWriter 给候选 (ii) 往 Rust 方向的数据报管道写加上写超时看门狗。
type PipeWriter struct {
	writer   io.Writer
	watchdog *writeWatchdog
	serial   chan struct{}
}

// NewPipeWriter 包装 w；写入超过 timeout 时返回 ErrWriteTimeout 并调用 onFailure 上报直连失效。
func NewPipeWriter(w io.Writer, timeout time.Duration, onFailure func(error)) (*PipeWriter, error) {
	// TODO(phase1): 合入 main 前删除或改成 build tag，见 p2p/noop.go「Phase 1 实现之后删除」。
	if os.Getenv("P2P_TEST_NOOP") != "" {
		return &PipeWriter{}, nil
	}
	return &PipeWriter{writer: w, watchdog: newWriteWatchdog(timeout, onFailure), serial: make(chan struct{}, 1)}, nil
}

// Write 写一个数据报帧。
func (p *PipeWriter) Write(b []byte) (int, error) {
	// TODO(phase1): 合入 main 前删除或改成 build tag，见 p2p/noop.go「Phase 1 实现之后删除」。
	if os.Getenv("P2P_TEST_NOOP") != "" {
		return noopWrite(b)
	}
	// 超时返回后底层写可能仍阻塞，副本允许调用者安全复用输入。
	payload := append([]byte(nil), b...)
	return p.watchdog.write(func() (int, error) { return p.writer.Write(payload) }, p.serial)
}

type writeWatchdog struct {
	timeout     time.Duration
	onFailure   func(error)
	failureOnce sync.Once
	failed      atomic.Bool
}

func newWriteWatchdog(timeout time.Duration, onFailure func(error)) *writeWatchdog {
	if timeout == 0 {
		timeout = 500 * time.Millisecond
	}
	return &writeWatchdog{timeout: timeout, onFailure: onFailure}
}

func (w *writeWatchdog) write(write func() (int, error), serial chan struct{}) (int, error) {
	if w.failed.Load() {
		return 0, ErrWriteTimeout
	}
	type result struct {
		n   int
		err error
	}
	timer := time.NewTimer(w.timeout)
	defer timer.Stop()
	if serial != nil {
		// 管道排队与底层写共用本次计时器；取得后把持有权交给底层写协程。
		select {
		case serial <- struct{}{}:
			if w.failed.Load() {
				<-serial
				return 0, ErrWriteTimeout
			}
		case <-timer.C:
			return w.writeTimeout()
		}
	}
	// 缓冲让超时后才完成的底层写也能退出，不必等待调用者接收。
	done := make(chan result, 1)
	go func() {
		if serial != nil {
			// 超时返回不能释放；只有底层写返回后才允许下一个底层写。
			defer func() { <-serial }()
		}
		n, err := write()
		done <- result{n: n, err: err}
	}()

	// 此计时器与等待分支均独立于可能永久阻塞的底层写协程。
	select {
	case r := <-done:
		return r.n, r.err
	case <-timer.C:
		return w.writeTimeout()
	}
}

func (w *writeWatchdog) writeTimeout() (int, error) {
	w.failed.Store(true)
	w.failureOnce.Do(func() {
		if w.onFailure != nil {
			go w.onFailure(ErrWriteTimeout)
		}
	})
	return 0, ErrWriteTimeout
}

// noopWrite 不碰底层写端：lenient 报告整包已写，strict 报告丢弃（0 字节、无错误）。
func noopWrite(p []byte) (int, error) {
	if noop == lenient {
		return len(p), nil
	}
	return 0, nil
}
