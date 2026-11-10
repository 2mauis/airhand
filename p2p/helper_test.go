// tailcat-p2p 规格「先红验收测试清单」中 helper 进程本身的用例（docs/specs/tailcat-p2p.md）。
package p2p

import (
	"errors"
	"net"
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
	mu   sync.Mutex
	errs []error
}

func (r *failureRecorder) report(err error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.errs = append(r.errs, err)
}

func (r *failureRecorder) count() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.errs)
}

// 清单 #19（I25）：隧道 Write / WriteTo 以及候选 (ii) 往 Rust 的管道写超过 write_timeout_ms（默认 500 ms）
// 即失败并上报，进程不永久阻塞。
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
			rec := &failureRecorder{}
			write, err := tc.open(conn, rec)
			if err != nil {
				t.Fatalf("创建写端: %v", err)
			}
			done := make(chan error, 1)
			start := time.Now()
			go func() { done <- write() }()
			select {
			case err := <-done:
				if !errors.Is(err, ErrWriteTimeout) {
					t.Fatalf("写卡住时应返回 ErrWriteTimeout，得到 %v", err)
				}
				if elapsed := time.Since(start); elapsed < timeout {
					t.Fatalf("%v 就返回了，早于写超时 %v", elapsed, timeout)
				}
			case <-time.After(timeout + time.Second):
				t.Fatal("写入超过写超时仍然阻塞")
			}
			if rec.count() != 1 {
				t.Fatalf("直连失效上报了 %d 次，应为 1", rec.count())
			}
		})
	}
}

// 清单 #40（I14、I9）：client_key 为空或无法解析时拒绝 Start。
func TestEmptyOrInvalidAllowlistRefusesToStart(t *testing.T) {
	for _, key := range []string{"", " ", "nodekey:", "nodekey:not-hex", "mkey:00", "example"} {
		_, err := StartServer(ServerConfig{ClientKey: key, WriteTimeout: 500 * time.Millisecond})
		if !errors.Is(err, ErrInvalidClientKey) {
			t.Errorf("client_key %q: 应以 ErrInvalidClientKey 拒绝启动，得到 %v", key, err)
		}
	}
}
