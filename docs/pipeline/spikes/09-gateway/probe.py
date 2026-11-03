"""S-09 throwaway: which no-root check proves the router is reachable over Wi-Fi?

Run as the normal user from Terminal.app:  python3 -I probe.py <label> [seconds]
Prints one line per candidate per second, then a summary. No root, no network changes.
"""
import os, random, socket, struct, subprocess, sys, time

IFACE = "en0"
IP_BOUND_IF = 25
label = sys.argv[1] if len(sys.argv) > 1 else "run"
seconds = int(sys.argv[2]) if len(sys.argv) > 2 else 20

route = subprocess.run(["/sbin/route", "-n", "get", "-ifscope", IFACE, "default"], capture_output=True, text=True).stdout
gw = next(l.split(":")[1].strip() for l in route.splitlines() if "gateway:" in l)
idx = socket.if_nametoindex(IFACE)


def sock(kind, proto=0):
    s = socket.socket(socket.AF_INET, kind, proto)
    s.setsockopt(socket.IPPROTO_IP, IP_BOUND_IF, struct.pack("I", idx))
    s.settimeout(1)
    return s


def err(e):
    return type(e).__name__ if isinstance(e, socket.timeout) else f"errno{getattr(e, 'errno', '?')}"


def icmp():
    s = sock(socket.SOCK_DGRAM, socket.IPPROTO_ICMP)
    p = bytearray(struct.pack("!BBHHH", 8, 0, 0, 0x4152, random.randrange(65536)) + b"s09")
    c = sum(struct.unpack("!%dH" % (len(p) // 2), bytes(p[: len(p) // 2 * 2]))) + (p[-1] << 8 if len(p) % 2 else 0)
    c = (c >> 16) + (c & 0xFFFF)
    p[2:4] = struct.pack("!H", ~(c + (c >> 16)) & 0xFFFF)
    try:
        s.sendto(bytes(p), (gw, 0))
        s.recvfrom(1500)
        return "reply"
    except OSError as e:
        return err(e)


def tcp(port):
    s = sock(socket.SOCK_STREAM)
    try:
        s.connect((gw, port))
        return "open"
    except ConnectionRefusedError:
        return "refused"  # an RST: the router is there
    except OSError as e:
        return err(e)
    finally:
        s.close()


def dns():
    s = sock(socket.SOCK_DGRAM)
    name = os.urandom(6).hex() + ".google.com"
    q = os.urandom(2) + b"\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
    q += b"".join(bytes([len(x)]) + x.encode() for x in name.split(".")) + b"\x00\x00\x01\x00\x01"
    try:
        s.sendto(q, (gw, 53))
        r = s.recv(512)
        return f"rcode{r[3] & 15}"
    except OSError as e:
        return err(e)


def arp():
    out = subprocess.run(["/usr/sbin/arp", "-la"], capture_output=True, text=True).stdout
    row = next((l.split() for l in out.splitlines() if l.split()[:1] == [gw]), None)
    return f"{row[1]} exp={row[2]} prbs={row[6] if len(row) > 6 else 0}" if row else "none"


print(f"S-09 {label} gw={gw} iface={IFACE} {time.strftime('%H:%M:%S')}")
seen = {}
for _ in range(seconds):
    r = {"icmp": icmp(), "tcp53": tcp(53), "tcp80": tcp(80), "tcp443": tcp(443), "dns": dns(), "arp": arp()}
    print(time.strftime("%H:%M:%S"), " ".join(f"{k}={v}" for k, v in r.items()), flush=True)
    for k, v in r.items():
        if k != "arp":
            seen.setdefault(k, {}).setdefault(v, 0)
            seen[k][v] += 1
    time.sleep(1)
print("summary", label, {k: v for k, v in seen.items()})
