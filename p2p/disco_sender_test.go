package p2p

import (
	"context"
	"sync/atomic"
	"testing"
	"time"

	"tailscale.com/ipn/ipnstate"
)

type blockedDiscoClient struct {
	active atomic.Int32
	high   atomic.Int32
	four   chan struct{}
}

func (c *blockedDiscoClient) DiscoPing(ctx context.Context) (*ipnstate.PingResult, error) {
	n := c.active.Add(1)
	defer c.active.Add(-1)
	for {
		old := c.high.Load()
		if n <= old || c.high.CompareAndSwap(old, n) {
			break
		}
	}
	if n == 4 {
		close(c.four)
	}
	<-ctx.Done()
	return nil, ctx.Err()
}

// This verifies sender scheduling/cancellation, independently of tailcat's own
// concurrency viability (#55 remains the real-client acceptance test).
func TestDiscoSenderPipelineDoesNotWaitForPreviousCall(t *testing.T) {
	client := &blockedDiscoClient{four: make(chan struct{})}
	sender := NewDiscoSender(client)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	results := sender.Pipeline(ctx, func() time.Duration { return 950 * time.Millisecond })
	select {
	case <-client.four:
	case <-time.After(2 * time.Second):
		t.Fatal("ASSERT: pipeline did not reach four in-flight calls")
	}
	cancel()
	done := time.After(2 * time.Second)
	for {
		select {
		case _, ok := <-results:
			if !ok {
				if client.active.Load() != 0 {
					t.Fatal("ASSERT: cancellation leaked calls")
				}
				if client.high.Load() < 4 {
					t.Fatal("ASSERT: sender serialized calls")
				}
				return
			}
		case <-done:
			t.Fatal("ASSERT: pipeline did not stop after cancellation")
		}
	}
}
