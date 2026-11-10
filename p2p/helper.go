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
}

// Server 是一个 epoch 的被控端 tailcat Server。
type Server struct{}

// StartServer 校验配置并启动 Server。
func StartServer(cfg ServerConfig) (*Server, error) {
	return nil, ErrNotImplemented
}

// PacketWriter 是隧道的数据报写端（tailcat / gVisor 的 UDPConn）。
type PacketWriter interface {
	WriteTo(p []byte, addr net.Addr) (int, error)
}

// TunnelWriter 给隧道写入加上写超时看门狗。
type TunnelWriter struct{}

// NewTunnelWriter 包装 w；写入超过 timeout 时返回 ErrWriteTimeout 并调用 onFailure 上报直连失效。
func NewTunnelWriter(w PacketWriter, timeout time.Duration, onFailure func(error)) (*TunnelWriter, error) {
	return nil, ErrNotImplemented
}

// WriteTo 写一个数据报。
func (t *TunnelWriter) WriteTo(p []byte, addr net.Addr) (int, error) {
	return 0, ErrNotImplemented
}

// PipeWriter 给候选 (ii) 往 Rust 方向的数据报管道写加上写超时看门狗。
type PipeWriter struct{}

// NewPipeWriter 包装 w；写入超过 timeout 时返回 ErrWriteTimeout 并调用 onFailure 上报直连失效。
func NewPipeWriter(w io.Writer, timeout time.Duration, onFailure func(error)) (*PipeWriter, error) {
	return nil, ErrNotImplemented
}

// Write 写一个数据报帧。
func (p *PipeWriter) Write(b []byte) (int, error) {
	return 0, ErrNotImplemented
}
