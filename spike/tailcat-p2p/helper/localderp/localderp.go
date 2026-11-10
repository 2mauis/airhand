// Package localderp runs a self-hosted DERP relay and STUN server in-process,
// so the spike never touches Tailscale-hosted DERP or the default DERP map.
//
// The TLS certificate is self-signed for derp.example.com and pinned through
// DERPNode.CertName ("sha256-raw:…"), which tailcat v0.7.0 carries in its
// address encoding.
package localderp

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/hex"
	"encoding/json"
	"errors"
	"expvar"
	"math/big"
	"net"
	"net/http"
	"strconv"
	"time"

	"tailscale.com/derp/derpserver"
	"tailscale.com/net/stunserver"
	"tailscale.com/tailcfg"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
)

const HostName = "derp.example.com"

type Relay struct {
	Region *tailcfg.DERPRegion
	srv    *derpserver.Server
	stats  expvar.Var
	http   *http.Server
	stun   *stunserver.STUNServer
	cancel context.CancelFunc
}

// Start listens on ip:derpPort (DERP over TLS) and ip:stunPort (STUN).
// A port of 0 picks a free one.
func Start(ip string, derpPort, stunPort int) (*Relay, error) {
	cert, certHash, err := selfSigned()
	if err != nil {
		return nil, err
	}
	ln, err := net.Listen("tcp", net.JoinHostPort(ip, strconv.Itoa(derpPort)))
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(context.Background())
	stun := stunserver.New(ctx)
	if err := stun.Listen(net.JoinHostPort(ip, strconv.Itoa(stunPort))); err != nil {
		cancel()
		ln.Close()
		return nil, err
	}
	go stun.Serve()

	s := derpserver.New(key.NewNode(), logger.Discard)
	mux := http.NewServeMux()
	mux.Handle("/derp", derpserver.Handler(s))
	mux.HandleFunc("/derp/probe", derpserver.ProbeHandler)
	mux.HandleFunc("/derp/latency-check", derpserver.ProbeHandler)
	mux.HandleFunc("/generate_204", derpserver.ServeNoContent)
	hs := &http.Server{
		Handler:   mux,
		TLSConfig: &tls.Config{Certificates: []tls.Certificate{cert}},
		ErrorLog:  nil,
	}
	go hs.ServeTLS(ln, "", "")

	_, dp, _ := net.SplitHostPort(ln.Addr().String())
	dport, _ := strconv.Atoi(dp)
	_, sp, _ := net.SplitHostPort(stun.LocalAddr().String())
	sport, _ := strconv.Atoi(sp)
	region := &tailcfg.DERPRegion{
		RegionID:   900,
		RegionCode: "spike",
		RegionName: "spike local relay",
		Nodes: []*tailcfg.DERPNode{{
			Name:     "900a",
			RegionID: 900,
			HostName: HostName,
			CertName: "sha256-raw:" + certHash,
			IPv4:     ip,
			IPv6:     "none",
			DERPPort: dport,
			STUNPort: sport,
		}},
	}
	return &Relay{Region: region, srv: s, stats: s.ExpVar(false), http: hs, stun: stun, cancel: cancel}, nil
}

// Counters returns the relay's packet counters. "data" counts relayed
// non-disco packets (WireGuard data and tailcat meow), "disco" counts
// path-discovery packets.
func (r *Relay) Counters() (data, disco int64) {
	var m struct {
		Kinds map[string]int64 `json:"counter_packets_received_kind"`
	}
	_ = json.Unmarshal([]byte(r.stats.String()), &m)
	return m.Kinds["other"], m.Kinds["disco"]
}

func (r *Relay) Close() error {
	r.cancel()
	return errors.Join(r.http.Close(), r.srv.Close())
}

func selfSigned() (tls.Certificate, string, error) {
	priv, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return tls.Certificate{}, "", err
	}
	tmpl := &x509.Certificate{
		SerialNumber: big.NewInt(time.Now().UnixNano()),
		Subject:      pkix.Name{CommonName: HostName},
		DNSNames:     []string{HostName},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     time.Now().Add(24 * time.Hour),
		KeyUsage:     x509.KeyUsageDigitalSignature,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &priv.PublicKey, priv)
	if err != nil {
		return tls.Certificate{}, "", err
	}
	sum := sha256.Sum256(der)
	return tls.Certificate{Certificate: [][]byte{der}, PrivateKey: priv}, hex.EncodeToString(sum[:]), nil
}
