// Command airhand-p2p-spike is the Phase 0 helper process.
//
// Usage: airhand-p2p-spike host|ctrl udp|pipe
//
// Control is JSON lines on stdin/stdout; secrets (the tailcat address, node
// keys) only travel over these pipes, never argv, env or stderr. In "pipe"
// mode datagrams use inherited fds 3 (Rust→helper) and 4 (helper→Rust).
package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"sync"
	"time"

	"example.com/airhand-p2p-spike/p2p"
	"github.com/tailscale/tailcat"
	"tailscale.com/tailcfg"
)

type cmd struct {
	Op        string              `json:"op"`
	ClientKey string              `json:"client_key"`
	Addr      string              `json:"addr"`
	Port      uint16              `json:"port"`
	PeerPort  uint16              `json:"peer_port"`
	Region    *tailcfg.DERPRegion `json:"region"`
	PollMs    int                 `json:"poll_ms"`
	RxSilMs   int                 `json:"rx_silence_ms"`
	PingMs    int                 `json:"ping_ms"`
	MinTOms   int                 `json:"min_timeout_ms"`
	Misses    int                 `json:"misses"`
}

var outMu sync.Mutex

func emit(ev p2p.Event) {
	b, _ := json.Marshal(ev)
	outMu.Lock()
	os.Stdout.Write(append(b, '\n'))
	outMu.Unlock()
}

func fail(err error) {
	emit(p2p.Event{"ev": "error", "error": err.Error()})
	os.Exit(1)
}

func main() {
	if len(os.Args) != 3 {
		fmt.Fprintln(os.Stderr, "usage: airhand-p2p-spike host|ctrl udp|pipe")
		os.Exit(2)
	}
	role, mode := os.Args[1], os.Args[2]

	var local p2p.Local
	var udp *p2p.UDPLocal
	switch mode {
	case "udp":
		u, err := p2p.NewUDPLocal()
		if err != nil {
			fail(err)
		}
		udp, local = u, u
		emit(p2p.Event{"ev": "chan", "port": u.Port()})
	case "pipe":
		local = &p2p.PipeLocal{R: os.NewFile(3, "dgram-in"), W: os.NewFile(4, "dgram-out")}
	default:
		fail(fmt.Errorf("unknown channel %q", mode))
	}

	var host *p2p.Host
	var ctrl *p2p.Ctrl
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<20)
	for in.Scan() {
		var c cmd
		if err := json.Unmarshal(in.Bytes(), &c); err != nil {
			fail(err)
		}
		switch c.Op {
		case "peer":
			if udp == nil {
				fail(fmt.Errorf("peer: not in udp mode"))
			}
			udp.SetPeer(c.PeerPort)
		case "serve":
			if role != "host" || host != nil {
				fail(fmt.Errorf("serve: wrong role or already serving"))
			}
			h, err := p2p.NewHost(p2p.HostConfig{
				ClientKey:    c.ClientKey,
				Port:         c.Port,
				Region:       c.Region,
				PollInterval: time.Duration(c.PollMs) * time.Millisecond,
				RxSilence:    time.Duration(c.RxSilMs) * time.Millisecond,
				Emit:         emit,
			})
			if err != nil {
				fail(err)
			}
			t0 := time.Now()
			addr, err := h.Start(local)
			if err != nil {
				fail(err)
			}
			host = h
			emit(p2p.Event{"ev": "offer", "addr": string(addr), "start_ms": time.Since(t0).Milliseconds()})
		case "init":
			if role != "ctrl" || ctrl != nil {
				fail(fmt.Errorf("init: wrong role or already initialised"))
			}
			ctrl = p2p.NewCtrl(p2p.CtrlConfig{
				Port:         c.Port,
				PingInterval: time.Duration(c.PingMs) * time.Millisecond,
				MinTimeout:   time.Duration(c.MinTOms) * time.Millisecond,
				Misses:       c.Misses,
				Emit:         emit,
			})
			k, _ := ctrl.PublicKey().MarshalText()
			emit(p2p.Event{"ev": "key", "client_key": string(k)})
		case "dial":
			if ctrl == nil {
				fail(fmt.Errorf("dial before init"))
			}
			go func(addr string) {
				ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
				defer cancel()
				if err := ctrl.Dial(ctx, tailcat.Addr(addr), local); err != nil {
					emit(p2p.Event{"ev": "dial_failed", "error": err.Error()})
				}
			}(c.Addr)
		case "stats":
			var ev p2p.Event
			switch {
			case host != nil:
				ev = host.Counters.Snapshot()
			case ctrl != nil:
				ev = ctrl.Counters.Snapshot()
			default:
				ev = p2p.Event{}
			}
			ev["ev"] = "stats"
			emit(ev)
		case "close":
			if host != nil {
				host.Close()
			}
			if ctrl != nil {
				ctrl.Close()
			}
			emit(p2p.Event{"ev": "closed"})
			os.Exit(0)
		default:
			fail(fmt.Errorf("unknown op %q", c.Op))
		}
	}
	// stdin closed: the parent is gone.
	if host != nil {
		host.Close()
	}
	if ctrl != nil {
		ctrl.Close()
	}
}
