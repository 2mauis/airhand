#!/usr/bin/env python3
"""Summarise Phase 0 results from $OUT/results into Markdown tables (stdout)."""
import glob
import json
import os
import sys

R = os.path.join(os.environ.get("OUT", "/tmp/p2p"), "results")


def load(name):
    p = os.path.join(R, name + ".json")
    if not os.path.exists(p) or os.path.getsize(p) == 0:
        return None
    return json.load(open(p))["out"]


def jsonl(name):
    p = os.path.join(R, name)
    if not os.path.exists(p):
        return []
    out = []
    for line in open(p):
        try:
            out.append(json.loads(line))
        except ValueError:
            pass
    return out


def ts(name):
    p = os.path.join(R, name)
    return int(open(p).read().strip()) if os.path.exists(p) else None


def f(x, d=2):
    return "—" if x is None else f"{x:.{d}f}"


def section(t):
    print(f"\n### {t}\n")


def rtt():
    section("LAN RTT (control-lane ping/pong, n=2000, ms)")
    print("| path | p50 | p90 | p99 | max | extra p50 vs plain | extra p99 vs plain |")
    print("|---|---|---|---|---|---|---|")
    base = load("lan-rtt-plain")
    b = base["result"]["rtt_ms"] if base else None
    for via in ["plain", "udp", "pipe", "relay"]:
        o = load(f"lan-rtt-{via}")
        if not o:
            print(f"| {via} | missing |")
            continue
        r = o["result"]["rtt_ms"]
        e50 = r["p50"] - b["p50"] if b else None
        e99 = r["p99"] - b["p99"] if b else None
        print(f"| {via} | {f(r['p50'],3)} | {f(r['p90'],3)} | {f(r['p99'],3)} | {f(r['max'],3)} | {f(e50,3)} | {f(e99,3)} |")


def tput(prefix, vias, title):
    section(title)
    print("| path | target Mbps | avg Mbps | min 1 s Mbps | QUIC sent pkts | lost pkts | loss % | MTU | host helper CPU % | ctrl helper CPU % | host quinn CPU % |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for via in vias:
        o = load(f"{prefix}-{via}")
        if not o:
            print(f"| {via} | FAILED / missing |")
            continue
        r = o["result"]
        q, c = r["quic_sender"], r["cpu_pct_single_core"]
        print(f"| {via} | {r['target_mbps']} | {f(r['avg_mbps'],1)} | {f(r['per_sec_mbps'].get('min'),1)} | {q['sent_packets']} | {q['lost_packets']} | {f(q['loss_pct'],4)} | {q['mtu']} | {f(c['host_helper'],1)} | {f(c['ctrl_helper'],1)} | {f(c['host_bench_quinn'],1)} |")


def upgrade(prefix, vias, title):
    section(title)
    print("| path | runs | failures | request→commit p50 / p90 / max (ms) | request→ctrl direct p50 (ms) | host direct − ctrl direct p50 / min / max (ms) |")
    print("|---|---|---|---|---|---|")
    for via in vias:
        o = load(f"{prefix}-{via}")
        if not o:
            print(f"| {via} | missing |")
            continue
        r = o
        a, b, c = r["request_to_commit_ms"], r["request_to_ctrl_direct_ms"], r["host_direct_minus_ctrl_direct_ms"]
        print(f"| {via} | {r['n']} | {r['failures']} | {f(a.get('p50'),0)} / {f(a.get('p90'),0)} / {f(a.get('max'),0)} | {f(b.get('p50'),0)} | {f(c.get('p50'),0)} / {f(c.get('min'),0)} / {f(c.get('max'),0)} |")


def derp_during(counters, start, end):
    pts = [c for c in counters if start <= c["t_us"] <= end]
    if len(pts) < 2:
        return None
    return pts[-1]["data"] - pts[0]["data"], pts[-1]["disco"] - pts[0]["disco"]


def double_send():
    section("DERP data packets while the path is direct (local derper counters)")
    counters = jsonl("lan-derp-counters.jsonl")
    print("| run | duration s | DERP data pkts | DERP disco pkts | QUIC datagrams sent by host |")
    print("|---|---|---|---|---|")
    for name in ["lan-tput60x300-udp", "lan-tput60x300-pipe", "lan-tput20-udp", "lan-tput20-pipe"]:
        s, e = ts(name + ".start"), ts(name + ".end")
        o = load(name)
        if not (s and e and o):
            continue
        # skip the first 3 s (bootstrap goes over DERP by design)
        d = derp_during(counters, s + 3_000_000, e)
        print(f"| {name} | {(e-s)/1e6:.0f} | {d[0] if d else '—'} | {d[1] if d else '—'} | {o['result']['quic_sender']['udp_tx_datagrams']} |")


def fence():
    section("Relay RTT and fence gap (relay control arrival − direct control arrival), direct one-way 1 ms")
    print("| relay one-way (ms) | relay RTT p50 / p99 (ms) | gap p50 | gap p99 | gap max (ms) |")
    print("|---|---|---|---|---|")
    for ms in [5, 20, 50, 100, 200]:
        a, b = load(f"wan-relayrtt-{ms}"), load(f"wan-fence-{ms}")
        if not (a and b):
            continue
        r = a["result"]["rtt_ms"]
        g = b["result"]["relay_minus_direct_arrival_ms"]
        print(f"| {ms} | {f(r['p50'],1)} / {f(r['p99'],1)} | {f(g['p50'],1)} | {f(g['p99'],1)} | {f(g['max'],1)} |")


def cut(prefix="wan-cut", note="host gate = Server.Status() CurAddr only"):
    section(f"Path cut (silent UDP drop host⇄ctrl at t=15 s; 5 Mbps host→ctrl; relay one-way 20 ms; {note})")
    wan = jsonl(f"{prefix}-wanemu.log")
    counters = jsonl(f"{prefix}-derp-counters.jsonl")
    cuts = [w["t_us"] for w in wan if w.get("cmd", "").startswith("cut")]
    restores = [w["t_us"] for w in wan if w.get("cmd", "").startswith("restore")]
    hostlog = jsonl(f"{prefix}-host.jsonl")
    print("| run | ctrl gate closed after cut (ms) | host gate closed after cut (ms) | DERP data pkts cut→host gate closed |")
    print("|---|---|---|---|")
    for p in sorted(glob.glob(os.path.join(R, f"{prefix}-*-[0-9].json"))):
        name = os.path.basename(p)[:-5]
        o = load(name)
        s, e = ts(name + ".start"), ts(name + ".end")
        c = next((x for x in cuts if s <= x <= e), None)
        rs = next((x for x in restores if s <= x <= e), None)
        if not (o and c):
            print(f"| {name} | missing |")
            continue
        r = o["result"]
        def first(paths, after, pred):
            for x in paths or []:
                if x["t_us"] >= after and pred(x["path"]):
                    return x["t_us"]
            return None
        host_paths = r.get("host_paths") or [
            h["event"] for h in hostlog
            if h.get("src") == "helper" and h["event"].get("ev") == "path"
            and h["event"].get("role") == "host" and s <= h["event"]["t_us"] <= e]
        cg = first(r.get("ctrl_paths"), c, lambda p: p != "direct")
        hg = first(host_paths, c, lambda p: p != "direct")
        cr = first(r.get("ctrl_paths"), rs or c, lambda p: p == "direct") if rs else None
        hr = first(host_paths, rs or c, lambda p: p == "direct") if rs else None
        d1 = derp_during(counters, c, hg) if hg else None
        d2 = derp_during(counters, c, rs) if rs else None
        ms = lambda a, b: f((a - b) / 1e3, 0) if a and b else "—"
        print(f"| {name} | {ms(cg, c)} | {ms(hg, c)} | {d1[0] if d1 else '—'} |")


def soak():
    section("Soak (static desktop: no media, direct Ping every 250 ms, K = 2 s, host rx-silence gate 1000 ms)")
    print("| scenario | secs | pings | ctrl K misses | host K misses | ctrl gate left direct | host gate left direct | ping RTT p50 / p99 / max (ms) |")
    print("|---|---|---|---|---|---|---|---|")
    for name in ["wan-soak-rtt400", "wan-soak-wifi"]:
        o = load(name)
        if not o:
            print(f"| {name} | missing |")
            continue
        r = o["result"]
        p = r["ping_rtt_ms"]
        print(f"| {name} | {r['secs']} | {r['pings']} | {r['ctrl_k_misses']} | {r['host_k_misses']} | {r['ctrl_gate_left_direct']} | {r['host_gate_left_direct']} | {f(p.get('p50'),1)} / {f(p.get('p99'),1)} / {f(p.get('max'),1)} |")


def consent():
    section("Consent step (r4), emulated WAN: relay one-way 20 ms per peer, direct 1 ms; simulated user answers 2 s after Hello")
    print("| set | runs | failures | approval→commit p50 / p90 / max (ms) | DirectRequest→commit p50 / p90 (ms) | ctrl helper spawn+init p50 (ms) | host helper spawn p50 / serve p50 (ms) | Hello→HostHello rx p50 (ms) | stop/kick→both paths closed p50 / max (ms) |")
    print("|---|---|---|---|---|---|---|---|---|")
    for name in ["consent-ask-cold-udp", "consent-ask-prewarm-udp", "consent-never-udp",
                 "consent-ask-cold-pipe", "consent-ask-prewarm-pipe", "consent-never-pipe"]:
        o = load(name)
        if not o:
            print(f"| {name} | missing |")
            continue
        m = o["ms"]
        g = lambda k, q: f(m.get(k, {}).get(q), 1) if m.get(k) else "—"
        print(f"| {name} | {o['n']} | {o['failures']} | {g('approval_to_commit','p50')} / {g('approval_to_commit','p90')} / {g('approval_to_commit','max')} "
              f"| {g('req_to_commit','p50')} / {g('req_to_commit','p90')} | {g('ctrl_helper_ready','p50')} | {g('host_spawn','p50')} / {g('host_serve','p50')} "
              f"| {g('hello_to_host_hello_rx','p50')} | {g('stop_to_relay_media_stream_end','p50')} / {g('stop_to_relay_media_stream_end','max')} |")


if __name__ == "__main__":
    rtt()
    tput("lan-tput20", ["plain", "udp", "pipe"], "LAN 20 Mbps × 60 s (1080p60 operating point)")
    tput("lan-tput60x300", ["udp", "pipe"], "LAN 60 Mbps × 300 s")
    tput("lan-tput60", ["relay"], "LAN 60 Mbps × 60 s via the real airhand relay (baseline)")
    tput("lan-tputmax", ["plain", "udp", "pipe", "relay"], "LAN unpaced (target 2000 Mbps) × 20 s")
    upgrade("lan-upgrade", ["udp", "pipe"], "Upgrade time, LAN (DERP RTT < 1 ms)")
    upgrade("wan-upgrade", ["udp", "pipe"], "Upgrade time, emulated WAN (DERP one-way 20 ms, direct 1 ms)")
    double_send()
    fence()
    cut()
    cut("wan-cutsil", "host gate also closes after 1000 ms without inbound datagrams")
    soak()
    consent()
