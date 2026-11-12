package p2p

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"tailscale.com/types/key"
)

type fixWriteResult struct {
	n   int
	err error
}

type fixWriter interface {
	Write([]byte) (int, error)
	WriteTo([]byte, net.Addr) (int, error)
}

func fixOpenWriter(t *testing.T, kind string, underlying fixWriter, timeout time.Duration, onFailure func(error)) func([]byte) (int, error) {
	t.Helper()
	if kind == "tunnel" {
		w, err := NewTunnelWriter(underlying, timeout, onFailure)
		assertNoErr(t, err, "NewTunnelWriter")
		assertf(t, w != nil, "NewTunnelWriter 返回 nil")
		return func(p []byte) (int, error) {
			return w.WriteTo(p, &net.UDPAddr{IP: net.IPv4(203, 0, 113, 10), Port: 41641})
		}
	}
	w, err := NewPipeWriter(underlying, timeout, onFailure)
	assertNoErr(t, err, "NewPipeWriter")
	assertf(t, w != nil, "NewPipeWriter 返回 nil")
	return w.Write
}

func fixBeginWrite(write func([]byte) (int, error)) <-chan fixWriteResult {
	done := make(chan fixWriteResult, 1)
	go func() {
		n, err := write([]byte("quic"))
		done <- fixWriteResult{n, err}
	}()
	return done
}

func fixAwaitWrite(t *testing.T, done <-chan fixWriteResult, limit time.Duration) fixWriteResult {
	t.Helper()
	select {
	case r := <-done:
		return r
	case <-time.After(limit):
		assertf(t, false, "写在 %v 内未返回", limit)
		return fixWriteResult{}
	}
}

type fixResultWriter struct {
	n   int
	err error
}

func (w fixResultWriter) Write([]byte) (int, error)             { return w.n, w.err }
func (w fixResultWriter) WriteTo([]byte, net.Addr) (int, error) { return w.n, w.err }

// #149（I25 / G1）：正常完成必须原样返回底层结果；空操作不能满足哨兵错误与短写。
func TestWriterPassesThroughUnderlyingResult(t *testing.T) {
	sentinel := errors.New("example short write")
	for _, kind := range []string{"tunnel", "pipe"} {
		t.Run(kind, func(t *testing.T) {
			write := fixOpenWriter(t, kind, fixResultWriter{n: 2, err: sentinel}, 500*time.Millisecond, nil)
			r := fixAwaitWrite(t, fixBeginWrite(write), time.Second)
			expectf(t, r.n == 2, "字节数 = %d，应原样返回 2", r.n)
			expectf(t, r.err == sentinel, "未原样返回底层哨兵错误：%v", r.err)
		})
	}
}

// #150（I25 / G2 / S2-2）：回调保持阻塞，调用者仍须按时返回，多次写只上报一次。
func TestWriteTimeoutReportsOnceAndNeverBlocksOnCallback(t *testing.T) {
	const timeout = 100 * time.Millisecond
	for _, kind := range []string{"tunnel", "pipe"} {
		t.Run(kind, func(t *testing.T) {
			conn := &stalledConn{release: make(chan struct{})}
			defer close(conn.release)
			releaseCallback := make(chan struct{})
			defer close(releaseCallback)
			var reports atomic.Int32
			callbackErrors := make(chan error, 8)
			write := fixOpenWriter(t, kind, conn, timeout, func(err error) {
				reports.Add(1)
				callbackErrors <- err
				<-releaseCallback
			})
			for i := 0; i < 4; i++ {
				start := time.Now()
				r := fixAwaitWrite(t, fixBeginWrite(write), timeout+100*time.Millisecond)
				expectf(t, time.Since(start) <= timeout+100*time.Millisecond, "第 %d 次写超过 timeout + 100 ms", i+1)
				expectf(t, r.n == 0 && errors.Is(r.err, ErrWriteTimeout), "第 %d 次写 = (%d, %v)，应为 (0, ErrWriteTimeout)", i+1, r.n, r.err)
			}
			select {
			case err := <-callbackErrors:
				expectf(t, errors.Is(err, ErrWriteTimeout), "回调原因不是 ErrWriteTimeout：%v", err)
			case <-time.After(100 * time.Millisecond):
				expectf(t, false, "没有收到异步超时回调")
			}
			expectf(t, reports.Load() == 1, "onFailure 调用了 %d 次，应恰好 1 次", reports.Load())
		})
	}
}

// fixTrackedWriter 用计数与显式释放观测底层写，不依赖运行时 goroutine 总数。
type fixTrackedWriter struct {
	mu       sync.Mutex
	calls    int
	inflight int
	max      int
	entered  chan struct{}
	release  chan struct{}
}

func newFixTrackedWriter() *fixTrackedWriter {
	return &fixTrackedWriter{entered: make(chan struct{}, 16), release: make(chan struct{})}
}

func (w *fixTrackedWriter) Write(p []byte) (int, error) {
	w.mu.Lock()
	w.calls++
	w.inflight++
	if w.inflight > w.max {
		w.max = w.inflight
	}
	w.mu.Unlock()
	w.entered <- struct{}{}
	<-w.release
	w.mu.Lock()
	w.inflight--
	w.mu.Unlock()
	return len(p), nil
}

func (w *fixTrackedWriter) WriteTo(p []byte, _ net.Addr) (int, error) { return w.Write(p) }

func (w *fixTrackedWriter) counts() (int, int) {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.calls, w.max
}

func fixAwaitEntry(t *testing.T, w *fixTrackedWriter) {
	t.Helper()
	select {
	case <-w.entered:
	case <-time.After(100 * time.Millisecond):
		assertf(t, false, "第一次写没有到达底层（正向对照）")
	}
}

// #151（I25 / G3）：超时锁死；管道串行且排队有超时；隧道仍允许并发。
func TestWriterLatchesAfterTimeoutAndPipeSerializes(t *testing.T) {
	const timeout = 200 * time.Millisecond
	for _, kind := range []string{"tunnel", "pipe"} {
		t.Run(kind+"_latches", func(t *testing.T) {
			underlying := newFixTrackedWriter()
			defer close(underlying.release)
			reports := make(chan error, 8)
			write := fixOpenWriter(t, kind, underlying, timeout, func(err error) { reports <- err })
			first := fixBeginWrite(write)
			fixAwaitEntry(t, underlying)
			r := fixAwaitWrite(t, first, timeout+100*time.Millisecond)
			assertf(t, errors.Is(r.err, ErrWriteTimeout), "首次写没有超时：%v", r.err)
			select {
			case err := <-reports:
				assertf(t, errors.Is(err, ErrWriteTimeout), "首次写未上报超时：%v", err)
			case <-time.After(100 * time.Millisecond):
				assertf(t, false, "首次底层写卡住但未上报超时（正向对照）")
			}
			for i := 0; i < 4; i++ {
				// 100 ms 小于本用例 timeout；排除重新开启完整超时的实现。
				r = fixAwaitWrite(t, fixBeginWrite(write), 100*time.Millisecond)
				expectf(t, r.n == 0 && errors.Is(r.err, ErrWriteTimeout), "锁死后写 = (%d, %v)", r.n, r.err)
			}
			calls, _ := underlying.counts()
			assertf(t, calls == 1, "首次超时后又调用底层写：总调用 %d 次", calls)
		})
	}
	t.Run("pipe_serializes_and_queue_times_out", func(t *testing.T) {
		underlying := newFixTrackedWriter()
		defer close(underlying.release)
		write := fixOpenWriter(t, "pipe", underlying, timeout, nil)
		first := fixBeginWrite(write)
		fixAwaitEntry(t, underlying)
		queued := make([]<-chan fixWriteResult, 8)
		for i := range queued {
			queued[i] = fixBeginWrite(write)
		}
		r := fixAwaitWrite(t, first, timeout+100*time.Millisecond)
		expectf(t, errors.Is(r.err, ErrWriteTimeout), "首次底层写应超时：%v", r.err)
		for _, done := range queued {
			r = fixAwaitWrite(t, done, 100*time.Millisecond)
			expectf(t, errors.Is(r.err, ErrWriteTimeout), "排队写应超时：%v", r.err)
		}
		calls, max := underlying.counts()
		assertf(t, calls == 1 && max == 1, "管道底层调用 %d 次，最多同时在途 %d，应都为 1", calls, max)
	})
	t.Run("pipe_releases_after_underlying_completion", func(t *testing.T) {
		underlying := newFixTrackedWriter()
		defer close(underlying.release)
		write := fixOpenWriter(t, "pipe", underlying, 500*time.Millisecond, nil)
		first := fixBeginWrite(write)
		fixAwaitEntry(t, underlying)
		second := fixBeginWrite(write)
		underlying.release <- struct{}{}
		r := fixAwaitWrite(t, first, 100*time.Millisecond)
		assertf(t, r.n == 4 && r.err == nil, "首次正常完成结果 = (%d, %v)", r.n, r.err)
		fixAwaitEntry(t, underlying)
		underlying.release <- struct{}{}
		r = fixAwaitWrite(t, second, 100*time.Millisecond)
		assertf(t, r.n == 4 && r.err == nil, "第二次正常完成结果 = (%d, %v)", r.n, r.err)
		calls, max := underlying.counts()
		assertf(t, calls == 2 && max == 1, "正常完成后管道调用 %d 次，最大在途 %d", calls, max)
	})
	t.Run("tunnel_allows_concurrent_writes", func(t *testing.T) {
		underlying := newFixTrackedWriter()
		defer close(underlying.release)
		write := fixOpenWriter(t, "tunnel", underlying, 500*time.Millisecond, nil)
		first := fixBeginWrite(write)
		fixAwaitEntry(t, underlying)
		second := fixBeginWrite(write)
		fixAwaitEntry(t, underlying)
		calls, max := underlying.counts()
		assertf(t, calls == 2 && max == 2, "隧道应允许并发，调用 %d 次、最大在途 %d", calls, max)
		underlying.release <- struct{}{}
		underlying.release <- struct{}{}
		for _, done := range []<-chan fixWriteResult{first, second} {
			r := fixAwaitWrite(t, done, 100*time.Millisecond)
			assertf(t, r.n == 4 && r.err == nil, "隧道正常完成结果 = (%d, %v)", r.n, r.err)
		}
	})
}

// 独立于已有 fakeBackend，严格记录配置和启动顺序。
type fixBackend struct {
	calls    []string
	allowed  []string
	startErr error
}

func (b *fixBackend) SetLogf(func(string, ...any)) { b.calls = append(b.calls, "SetLogf") }
func (b *fixBackend) SetAllowedClients(keys []string) {
	b.calls = append(b.calls, "SetAllowedClients")
	b.allowed = append([]string(nil), keys...)
}
func (b *fixBackend) AddAllowedClient(string) { b.calls = append(b.calls, "AddAllowedClient") }
func (b *fixBackend) Start() error {
	b.calls = append(b.calls, "Start")
	return b.startErr
}

// #152（I14、I7 / G5）：一次设置允许列表必须发生在 Start 之前。
func TestStartServerConfiguresBackendBeforeStart(t *testing.T) {
	be := &fixBackend{}
	s, err := StartServer(ServerConfig{ClientKey: testClientKey("ab"), Backend: be})
	assertNoErr(t, err, "StartServer")
	assertf(t, s != nil, "StartServer 返回 nil")
	defer s.Close()
	assertf(t, reflect.DeepEqual(be.calls, []string{"SetLogf", "SetAllowedClients", "Start"}), "Backend 调用顺序 = %v", be.calls)
	assertf(t, reflect.DeepEqual(be.allowed, []string{testClientKey("ab")}), "允许列表不匹配")
}

// #153（I2、I7 / G7 / S2-3）：返回安全类别，原因只经解包保留。
func TestStartFailureReturnsCategoryErrorOnly(t *testing.T) {
	const endpoint = "203.0.113.10:41641"
	const original = "example backend socket failure"
	for _, cause := range []error{errors.New(endpoint + ": " + original), fmt.Errorf("%s: %s: %w", endpoint, original, ErrWriteTimeout)} {
		be := &fixBackend{startErr: cause}
		s, err := StartServer(ServerConfig{ClientKey: testClientKey("ab"), Backend: be})
		assertf(t, err != nil, "Backend.Start 失败却未返回错误")
		expectf(t, s == nil, "启动失败却返回 Server")
		expectf(t, err.Error() == "p2p: start_failed", "Error() 必须恰为 p2p: start_failed")
		expectf(t, !strings.Contains(err.Error(), endpoint) && !strings.Contains(err.Error(), original), "错误泄露底层端点或原文")
		expectf(t, errors.Is(err, cause), "errors.Is 无法取得底层原因")
		var wrapped interface {
			error
			Unwrap() error
		}
		expectf(t, errors.As(err, &wrapped) && wrapped.Unwrap() == cause, "原因必须通过 Unwrap 保留")
		var ev Event
		line := EncodeEvent(PathObservation{Err: err})
		assertf(t, len(line) > 0 && json.Unmarshal(line, &ev) == nil, "未编码出合法错误事件")
		expectf(t, ev.Reason == "start_failed", "启动错误事件 reason = %q", ev.Reason)
	}
}

// #154（I2、I7 / G9、G10）：封闭枚举、精确的哨兵匹配与单行小写 JSON。
func TestEncodeEventFieldsAreLowercaseEnumAndSingleLine(t *testing.T) {
	for _, tc := range []struct {
		err    error
		reason string
	}{
		{nil, ""},
		{ErrWriteTimeout, "write_timeout"},
		{fmt.Errorf("example: %w", ErrWriteTimeout), "write_timeout"},
		{ErrInvalidClientKey, "invalid_client_key"},
		{fmt.Errorf("example: %w", ErrInvalidClientKey), "invalid_client_key"},
		{io.ErrClosedPipe, "pipe_closed"},
		{os.ErrClosed, "pipe_closed"},
		{syscall.EPIPE, "pipe_closed"},
		{fmt.Errorf("example: %w", syscall.EPIPE), "pipe_closed"},
		{io.EOF, "unknown"},
		{errors.New("example\npipe closed: 203.0.113.10:41641"), "unknown"},
	} {
		line := EncodeEvent(PathObservation{Err: tc.err})
		assertf(t, len(line) > 0, "EncodeEvent 返回空事件")
		expectf(t, line[len(line)-1] == '\n' && bytes.Count(line, []byte{'\n'}) == 1 && !bytes.ContainsRune(line[:len(line)-1], '\r'), "事件必须恰为一行：%q", line)
		var fields map[string]json.RawMessage
		assertf(t, json.Unmarshal(line, &fields) == nil, "事件不是合法 JSON：%q", line)
		for name := range fields {
			expectf(t, name == "kind" || name == "path" || name == "reason" || name == "count", "事件出现未知或非小写字段 %q", name)
		}
		for _, name := range []string{"kind", "path", "count"} {
			_, ok := fields[name]
			expectf(t, ok, "事件缺少字段 %q", name)
		}
		var ev Event
		assertf(t, json.Unmarshal(line, &ev) == nil, "事件字段类型错误")
		expectf(t, ev.Path == "none", "非直连必须报告 none，得到 %q", ev.Path)
		if tc.err == nil {
			_, hasReason := fields["reason"]
			expectf(t, !hasReason, "无错误不应报告 reason")
		} else {
			expectf(t, ev.Reason == "write_timeout" || ev.Reason == "invalid_client_key" || ev.Reason == "start_failed" || ev.Reason == "pipe_closed" || ev.Reason == "unknown", "reason 不在封闭枚举内：%q", ev.Reason)
			expectf(t, ev.Reason == tc.reason, "reason = %q，应为 %q", ev.Reason, tc.reason)
		}
	}
	for _, tc := range []struct {
		err  error
		text string
	}{{ErrWriteTimeout, "p2p: write_timeout"}, {ErrInvalidClientKey, "p2p: invalid_client_key"}} {
		expectf(t, tc.err.Error() == tc.text, "导出哨兵的 Error() 应只含类别码 %q", tc.text)
	}
}

// #155（I14 / S3-5、S3-6）：拒绝零密钥，合法允许列表使用规范化字符串。
func TestClientKeyZeroRejectedAndAllowlistCanonical(t *testing.T) {
	be := &fixBackend{}
	s, err := StartServer(ServerConfig{ClientKey: testClientKey("00"), Backend: be})
	expectf(t, s == nil && errors.Is(err, ErrInvalidClientKey), "全零公钥应返回 ErrInvalidClientKey")
	expectf(t, len(be.calls) == 0, "全零公钥调用了 Backend：%v", be.calls)
	for _, input := range []string{testClientKey("ab"), testClientKey("AB")} {
		be := &fixBackend{}
		s, err := StartServer(ServerConfig{ClientKey: input, Backend: be})
		if err != nil {
			expectf(t, input == testClientKey("AB") && errors.Is(err, ErrInvalidClientKey) && len(be.calls) == 0, "只有大写输入可被拒绝，且不得调用 Backend")
			continue
		}
		assertf(t, s != nil, "合法输入返回 nil Server")
		s.Close()
		var parsed key.NodePublic
		assertNoErr(t, parsed.UnmarshalText([]byte(input)), "解析合法公钥")
		expectf(t, reflect.DeepEqual(be.allowed, []string{parsed.String()}), "允许列表没有使用 clientKey.String()")
		expectf(t, len(be.allowed) == 1 && be.allowed[0] == strings.ToLower(be.allowed[0]), "允许列表必须恰含一个小写规范公钥")
	}
}
