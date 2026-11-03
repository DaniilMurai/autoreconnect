import socket, struct, time, random, string, statistics, urllib.request, ssl

def rnd(): return ''.join(random.choices(string.ascii_lowercase+string.digits, k=12))
def q(name, srv, timeout=2.0):
    tid = random.randint(0,65535)
    pkt = struct.pack('>HHHHHH', tid, 0x0100, 1,0,0,0) + b''.join(bytes([len(l)])+l.encode() for l in name.split('.')) + b'\0' + struct.pack('>HH',1,1)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(timeout)
    t=time.perf_counter()
    s.sendto(pkt,(srv,53))
    try:
        d,_=s.recvfrom(4096); dt=time.perf_counter()-t
        return dt, len(pkt), len(d), d[3]&0xf, struct.unpack('>H',d[6:8])[0]
    except socket.timeout:
        return time.perf_counter()-t, len(pkt), 0, None, 0
def pct(v,p): v=sorted(v); return v[min(len(v)-1,int(round(p/100*(len(v)-1))))]

print("== direct UDP DNS to 1.1.1.1, random names (rcode 0=ok,3=NXDOMAIN)")
for dom in ["example.com","google.com","nonexistent-tld-zz"]:
    r=[q(f"{rnd()}.{dom}","1.1.1.1") for _ in range(10)]
    print(dom, "p50 %.1f ms p95 %.1f ms"%(pct([x[0] for x in r],50)*1e3,pct([x[0] for x in r],95)*1e3),
      "sent",r[0][1],"recv",r[0][2],"rcodes",sorted(set(x[3] for x in r)))
print("== getaddrinfo cache test (random name under example.com => NXDOMAIN, and a real name)")
for name in [f"{rnd()}.example.com", "example.com", "www.apple.com"]:
    ts=[]
    for _ in range(3):
        t=time.perf_counter()
        try: socket.getaddrinfo(name,443)
        except Exception as e: err=type(e).__name__
        ts.append((time.perf_counter()-t)*1e3)
    print(name, ["%.2f ms"%x for x in ts])
print("== blackhole resolver 192.0.2.1")
for to in (1.0,2.0):
    r=q(f"{rnd()}.example.com","192.0.2.1",to); print("timeout",to,"-> %.3f s"%r[0],"recv",r[2])
print("== HTTP probes, 20 samples, fresh connection each (urllib)")
ctx=ssl.create_default_context()
for url in ["http://captive.apple.com/hotspot-detect.html","http://connectivitycheck.gstatic.com/generate_204","http://www.msftconnecttest.com/connecttest.txt"]:
    ts=[];sz=None;code=None
    for _ in range(20):
        t=time.perf_counter()
        try:
            r=urllib.request.urlopen(urllib.request.Request(url,headers={'User-Agent':'x'}),timeout=5)
            b=r.read(); code=r.status; sz=len(b)
        except Exception as e: code=repr(e)
        ts.append((time.perf_counter()-t)*1e3)
    print(url,code,"body",sz,"p50 %.0f ms p95 %.0f ms"%(pct(ts,50),pct(ts,95)))
