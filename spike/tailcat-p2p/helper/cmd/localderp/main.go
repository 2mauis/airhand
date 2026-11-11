// Command localderp runs a self-hosted DERP + STUN server for the spike and
// prints its region (JSON) on stdout, then relay packet counters once per
// second on stderr as JSON lines.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"time"

	"example.com/airhand-p2p-spike/localderp"
)

func main() {
	ip := flag.String("ip", "127.0.0.1", "listen IP")
	derpPort := flag.Int("derp-port", 8443, "DERP TLS port")
	stunPort := flag.Int("stun-port", 3478, "STUN port")
	every := flag.Duration("every", 100*time.Millisecond, "counter print interval")
	flag.Parse()
	r, err := localderp.Start(*ip, *derpPort, *stunPort)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	b, _ := json.Marshal(r.Region)
	fmt.Println(string(b))
	for range time.Tick(*every) {
		data, disco := r.Counters()
		accepts, conns := r.Conns()
		fmt.Fprintf(os.Stderr, "{\"t_us\":%d,\"data\":%d,\"disco\":%d,\"accepts\":%d,\"conns\":%d}\n",
			time.Now().UnixMicro(), data, disco, accepts, conns)
	}
}
