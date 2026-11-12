package p2p

import (
	"context"
	"errors"
	"os"
	"strings"
	"testing"
	"time"
)

// B 级：包构造的错误只暴露类别，底层原因仍可供内部解包。
func TestPackageConstructedErrorsAreCategoryOnly(t *testing.T) {
	t.Run("invalid_config", func(t *testing.T) {
		expectf(t, ErrInvalidConfig.Error() == "p2p: invalid_config", "ErrInvalidConfig 文案必须恰为 p2p: invalid_config")
		expectf(t, errors.Is(ErrInvalidConfig, ErrInvalidConfig), "ErrInvalidConfig 必须仍能由 errors.Is 匹配")
	})
	t.Run("discovery_timeout", func(t *testing.T) {
		n, err := newDiscoNet(TestNetConfig{UpgradeTimeout: time.Nanosecond})
		if n != nil {
			defer n.close()
		}
		assertf(t, err != nil, "发现超时必须返回错误")
		expectf(t, err.Error() == "p2p: unknown", "发现超时文案必须恰为 p2p: unknown")
		expectf(t, errors.Is(err, context.DeadlineExceeded), "发现超时必须保留 context.DeadlineExceeded 原因")
	})
}

// B 级：环境变量只由测试读取；生产 Go 文件只看测试设置的包级开关。
func TestNoopSwitchReadOnlyInTests(t *testing.T) {
	entries, err := os.ReadDir(".")
	assertNoErr(t, err, "读取 p2p 目录")
	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
			continue
		}
		t.Run(name, func(t *testing.T) {
			source, err := os.ReadFile(name)
			assertNoErr(t, err, "读取非测试 Go 文件")
			expectf(t, !strings.Contains(string(source), `"P2P_TEST_NOOP"`), "%s 不得含空操作环境变量字符串字面量", name)
			expectf(t, !strings.Contains(string(source), "os.Getenv("), "%s 不得读取环境变量", name)
		})
	}
}
