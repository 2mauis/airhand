package p2p

import (
	"context"
	"sync"
	"sync/atomic"
	"time"

	"tailscale.com/ipn/ipnstate"
)

// DiscoSender is only a concurrent sender. It makes no path/gate decisions and
// never serializes calls behind a previous response.
type DiscoClient interface {
	DiscoPing(context.Context) (*ipnstate.PingResult, error)
}

type DiscoSender struct {
	client DiscoClient
	pongs  atomic.Int64
}

func NewDiscoSender(client DiscoClient) *DiscoSender { return &DiscoSender{client: client} }

func (s *DiscoSender) Ping(ctx context.Context) (*ipnstate.PingResult, error) {
	result, err := s.client.DiscoPing(ctx)
	if err == nil {
		s.pongs.Add(1)
	}
	return result, err
}

func (s *DiscoSender) PongsReceived() int { return int(s.pongs.Load()) }

type DiscoResult struct {
	Sequence uint64
	Sent     time.Time
	Pong     *ipnstate.PingResult
	Err      error
}

// Pipeline starts immediately, then every 250 ms. Each call gets its own
// context and deadline supplied by the caller (the D policy is outside PV-1).
// Cancel ctx to stop launches and release outstanding result deliveries.
func (s *DiscoSender) Pipeline(ctx context.Context, deadline func() time.Duration) <-chan DiscoResult {
	results := make(chan DiscoResult, 16)
	go func() {
		var calls sync.WaitGroup
		defer func() { calls.Wait(); close(results) }()
		ticker := time.NewTicker(250 * time.Millisecond)
		defer ticker.Stop()
		for seq := uint64(0); ; seq++ {
			if ctx.Err() != nil {
				return
			}
			callCtx, cancel := context.WithTimeout(ctx, deadline())
			sent := time.Now()
			calls.Add(1)
			go func(seq uint64) {
				defer calls.Done()
				defer cancel()
				pong, err := s.Ping(callCtx)
				select {
				case results <- DiscoResult{seq, sent, pong, err}:
				case <-ctx.Done():
				}
			}(seq)
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
			}
		}
	}()
	return results
}
