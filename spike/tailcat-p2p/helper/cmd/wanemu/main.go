// Command wanemu is a userspace L3 network emulator for the spike: the VM
// kernel has no netem, so each network namespace gets a TUN device and this
// process routes IPv4 packets between them with per-direction delay,
// jitter, loss and "cut" (silent UDP drop) rules.
//
//	wanemu serve --sock /tmp/wan.sock --node host=ah-host:192.0.2.10 --node ...
//	wanemu ctl --sock /tmp/wan.sock 'link host ctrl delay=5 jitter=2 loss=0.5'
//	wanemu ctl --sock /tmp/wan.sock 'cut host ctrl' / 'restore host ctrl'
//
// "link a b" and "cut a b" apply to both directions. Every rule change is
// printed on stdout with its wall-clock time in µs.
package main

import (
	"bufio"
	"encoding/binary"
	"flag"
	"fmt"
	"math/rand/v2"
	"net"
	"net/netip"
	"os"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unsafe"

	"golang.org/x/sys/unix"
)

type node struct {
	name string
	ip   netip.Addr
	tun  *os.File
}

type rule struct {
	delay  time.Duration
	jitter time.Duration
	loss   float64
	cut    bool
}

type link struct {
	from, to *node
	rule     atomic.Pointer[rule]
	q        chan pkt
	last     time.Time
	dropped  atomic.Uint64
	passed   atomic.Uint64
}

type pkt struct {
	at time.Time
	b  []byte
}

var (
	nodes []*node
	links = map[[2]string]*link{}
	mu    sync.Mutex
)

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: wanemu serve|ctl ...")
		os.Exit(2)
	}
	switch os.Args[1] {
	case "serve":
		serve(os.Args[2:])
	case "ctl":
		ctl(os.Args[2:])
	default:
		os.Exit(2)
	}
}

type multi []string

func (m *multi) String() string     { return strings.Join(*m, ",") }
func (m *multi) Set(s string) error { *m = append(*m, s); return nil }

func ctl(args []string) {
	fs := flag.NewFlagSet("ctl", flag.ExitOnError)
	sock := fs.String("sock", "/tmp/wanemu.sock", "")
	fs.Parse(args)
	c, err := net.Dial("unix", *sock)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	fmt.Fprintln(c, strings.Join(fs.Args(), " "))
	r := bufio.NewScanner(c)
	if r.Scan() {
		fmt.Println(r.Text())
	}
}

func serve(args []string) {
	fs := flag.NewFlagSet("serve", flag.ExitOnError)
	sock := fs.String("sock", "/tmp/wanemu.sock", "")
	var specs multi
	fs.Var(&specs, "node", "name=netns:ipv4")
	fs.Parse(args)
	for _, s := range specs {
		name, rest, _ := strings.Cut(s, "=")
		ns, ip, _ := strings.Cut(rest, ":")
		tun, err := openTunIn(ns, "wan0")
		if err != nil {
			fmt.Fprintln(os.Stderr, "tun", ns, err)
			os.Exit(1)
		}
		nodes = append(nodes, &node{name: name, ip: netip.MustParseAddr(ip), tun: tun})
	}
	for _, a := range nodes {
		for _, b := range nodes {
			if a == b {
				continue
			}
			l := &link{from: a, to: b, q: make(chan pkt, 65536)}
			l.rule.Store(&rule{})
			links[[2]string{a.name, b.name}] = l
			go l.run()
		}
	}
	for _, n := range nodes {
		go forward(n)
	}
	os.Remove(*sock)
	ln, err := net.Listen("unix", *sock)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	os.Chmod(*sock, 0o666)
	fmt.Println("ready")
	for {
		c, err := ln.Accept()
		if err != nil {
			continue
		}
		go handle(c)
	}
}

func handle(c net.Conn) {
	defer c.Close()
	r := bufio.NewScanner(c)
	for r.Scan() {
		reply := apply(strings.Fields(r.Text()))
		fmt.Fprintln(c, reply)
		fmt.Printf("{\"t_us\":%d,\"cmd\":%q,\"reply\":%q}\n", time.Now().UnixMicro(), r.Text(), reply)
	}
}

func apply(f []string) string {
	if len(f) == 1 && f[0] == "stats" {
		var sb strings.Builder
		for k, l := range links {
			fmt.Fprintf(&sb, "%s>%s pass=%d drop=%d; ", k[0], k[1], l.passed.Load(), l.dropped.Load())
		}
		return sb.String()
	}
	if len(f) < 3 {
		return "err: want 'link|cut|restore a b [k=v...]'"
	}
	pair := [][2]string{{f[1], f[2]}, {f[2], f[1]}}
	for _, p := range pair {
		l := links[p]
		if l == nil {
			return "err: no such link"
		}
		r := *l.rule.Load()
		switch f[0] {
		case "cut":
			r.cut = true
		case "restore":
			r.cut = false
		case "link":
			for _, kv := range f[3:] {
				k, v, _ := strings.Cut(kv, "=")
				x, _ := strconv.ParseFloat(v, 64)
				switch k {
				case "delay":
					r.delay = time.Duration(x * float64(time.Millisecond))
				case "jitter":
					r.jitter = time.Duration(x * float64(time.Millisecond))
				case "loss":
					r.loss = x / 100
				}
			}
		default:
			return "err: unknown verb"
		}
		l.rule.Store(&r)
	}
	return "ok"
}

func forward(n *node) {
	buf := make([]byte, 65536)
	for {
		k, err := n.tun.Read(buf)
		if err != nil {
			return
		}
		b := buf[:k]
		if k < 20 || b[0]>>4 != 4 {
			continue // IPv4 only
		}
		dst := netip.AddrFrom4([4]byte(b[16:20]))
		var to *node
		for _, m := range nodes {
			if m.ip == dst {
				to = m
				break
			}
		}
		if to == nil || to == n {
			continue
		}
		l := links[[2]string{n.name, to.name}]
		r := l.rule.Load()
		if r.cut && b[9] == 17 { // UDP
			l.dropped.Add(1)
			continue
		}
		if r.loss > 0 && rand.Float64() < r.loss {
			l.dropped.Add(1)
			continue
		}
		d := r.delay
		if r.jitter > 0 {
			d += time.Duration(rand.Int64N(int64(r.jitter)))
		}
		at := time.Now().Add(d)
		if at.Before(l.last) {
			at = l.last // keep per-direction FIFO order
		}
		l.last = at
		select {
		case l.q <- pkt{at, append([]byte(nil), b...)}:
		default:
			l.dropped.Add(1)
		}
	}
}

func (l *link) run() {
	for p := range l.q {
		if w := time.Until(p.at); w > 0 {
			time.Sleep(w)
		}
		l.to.tun.Write(p.b)
		l.passed.Add(1)
	}
}

func openTunIn(ns, name string) (*os.File, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	self, err := os.Open("/proc/thread-self/ns/net")
	if err != nil {
		return nil, err
	}
	defer self.Close()
	target, err := os.Open("/run/netns/" + ns)
	if err != nil {
		return nil, err
	}
	defer target.Close()
	if err := unix.Setns(int(target.Fd()), unix.CLONE_NEWNET); err != nil {
		return nil, err
	}
	defer unix.Setns(int(self.Fd()), unix.CLONE_NEWNET)
	fd, err := unix.Open("/dev/net/tun", unix.O_RDWR|unix.O_CLOEXEC, 0)
	if err != nil {
		return nil, err
	}
	var ifr [unix.IFNAMSIZ + 64]byte
	copy(ifr[:], name)
	binary.NativeEndian.PutUint16(ifr[unix.IFNAMSIZ:], unix.IFF_TUN|unix.IFF_NO_PI)
	if _, _, e := unix.Syscall(unix.SYS_IOCTL, uintptr(fd), unix.TUNSETIFF, uintptr(unsafe.Pointer(&ifr[0]))); e != 0 {
		unix.Close(fd)
		return nil, e
	}
	return os.NewFile(uintptr(fd), name), nil
}
