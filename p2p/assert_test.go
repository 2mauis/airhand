package p2p

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// assertf / expectf 是先红用例共用的断言：失败信息以 "ASSERT: " 开头，预期红名单门禁
// （tests/p2p-expected-red.mjs）据此区分断言失败与其它原因的红。assertf 立即结束用例，expectf 继续。
func assertf(t testing.TB, cond bool, format string, args ...any) {
	t.Helper()
	if !cond {
		t.Fatalf("ASSERT: "+format, args...)
	}
}

func expectf(t testing.TB, cond bool, format string, args ...any) {
	t.Helper()
	if !cond {
		t.Errorf("ASSERT: "+format, args...)
	}
}

// assertNoErr 把构造 / 调用失败也报成断言失败（空操作桩不返回错误，出现错误本身就是断言不满足）。
func assertNoErr(t testing.TB, err error, what string) {
	t.Helper()
	assertf(t, err == nil, "%s: %v", what, err)
}

// record 写一条只记录、不作断言的观测值（soak、随机丢包与 OQ-26 变体）：输出 "RECORD: <用例>: …"，
// 设置了 P2P_RECORD_DIR 时同时追加到该目录下的 <用例>.txt。
func record(t testing.TB, format string, args ...any) {
	t.Helper()
	line := fmt.Sprintf(format, args...)
	t.Logf("RECORD: %s: %s", t.Name(), line)
	dir := os.Getenv("P2P_RECORD_DIR")
	if dir == "" {
		return
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return
	}
	name := strings.NewReplacer("/", "__", " ", "_").Replace(t.Name())
	f, err := os.OpenFile(filepath.Join(dir, name+".txt"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		return
	}
	defer f.Close()
	fmt.Fprintf(f, "%s: %s\n", t.Name(), line)
}

// TestMain 按 P2P_TEST_NOOP 选择空操作极性（#99）；未设置时为常规作业的 strict。
func TestMain(m *testing.M) {
	switch v := os.Getenv("P2P_TEST_NOOP"); v {
	case "", "strict":
		noop = strict
	case "lenient":
		noop = lenient
	default:
		fmt.Fprintf(os.Stderr, "P2P_TEST_NOOP=%q：只能是 lenient 或 strict\n", v)
		os.Exit(2)
	}
	os.Exit(m.Run())
}
