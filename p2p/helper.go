package p2p

import (
	"errors"
	"io"
	"net"
	"time"
)

// ErrInvalidClientKey 表示 client_key 为空或无法解析（I14：tailcat v0.7.0 的空 AllowedClients 会放行所有客户端）。
var ErrInvalidClientKey = errors.New("p2p: empty or invalid client_key")

// ErrWriteTimeout 表示一次写入超过 write_timeout_ms 仍未完成（I25）。
var ErrWriteTimeout = errors.New("p2p: write timeout")

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
	Kind string
	// Path 是路径类别："direct"、"derp" 或 "none"。
	Path string
	// Reason 是原因类别码（gate_closed / error）。
	Reason string
	// Count 是计数类字段。
	Count int
}

// PathObservation 是 helper 内部看到的原始路径信息（含端点与 Go 错误），不得原样外发。
type PathObservation struct {
	CurAddr string
	Err     error
}

// EncodeEvent 把一次路径观测编码成发给 Rust 的事件行：只含路径类别、计数与原因类别码。
func EncodeEvent(obs PathObservation) []byte { return nil }

// RunControl 运行 helper 的控制协议：从 in 逐行读 JSON 命令，事件写到 out，诊断写到 errw；argv 不带任何地址。
func RunControl(in io.Reader, out, errw io.Writer) error { return nil }

// HelperArgv 是 Rust 启动 helper 时使用的命令行参数（不含程序名）。
func HelperArgv() []string { return nil }

// StartServer 校验配置并启动 Server。
func StartServer(cfg ServerConfig) (*Server, error) {
	return &Server{}, nil
}

// PacketWriter 是隧道的数据报写端（tailcat / gVisor 的 UDPConn）。
type PacketWriter interface {
	WriteTo(p []byte, addr net.Addr) (int, error)
}

// TunnelWriter 给隧道写入加上写超时看门狗。
type TunnelWriter struct{}

// NewTunnelWriter 包装 w；写入超过 timeout 时返回 ErrWriteTimeout 并调用 onFailure 上报直连失效。
func NewTunnelWriter(w PacketWriter, timeout time.Duration, onFailure func(error)) (*TunnelWriter, error) {
	return &TunnelWriter{}, nil
}

// WriteTo 写一个数据报。
func (t *TunnelWriter) WriteTo(p []byte, addr net.Addr) (int, error) {
	return noopWrite(p)
}

// PipeWriter 给候选 (ii) 往 Rust 方向的数据报管道写加上写超时看门狗。
type PipeWriter struct{}

// NewPipeWriter 包装 w；写入超过 timeout 时返回 ErrWriteTimeout 并调用 onFailure 上报直连失效。
func NewPipeWriter(w io.Writer, timeout time.Duration, onFailure func(error)) (*PipeWriter, error) {
	return &PipeWriter{}, nil
}

// Write 写一个数据报帧。
func (p *PipeWriter) Write(b []byte) (int, error) {
	return noopWrite(b)
}

// noopWrite 不碰底层写端：lenient 报告整包已写，strict 报告丢弃（0 字节、无错误）。
func noopWrite(p []byte) (int, error) {
	if noop == lenient {
		return len(p), nil
	}
	return 0, nil
}
