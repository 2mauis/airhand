package p2p

import (
	"fmt"
	"os"
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
