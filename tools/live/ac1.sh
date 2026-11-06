#!/bin/bash
# AC-1: release bundle live on healthy Wi-Fi, CPU and physical footprint sampled every 10 s for 10 min. No fault, no sudo.
. "$(dirname "$0")/common.sh"
begin ac1
NO_TRACE=1 launch
BIN=$APP/Contents/MacOS/autoreconnect
echo "t_s,cpu_pct,phys_footprint_bytes" > "$D/samples.csv"
for i in $(seq 1 60); do
  sleep 10
  [ "$(pgrep -x autoreconnect)" = "$PID" ] || { echo "app pid changed or gone at sample $i"; stop_app; exit 1; }
  cpu=$(ps -o %cpu= -p "$PID" | tr -d ' ')
  fp=$(footprint -f bytes --noCategories "$PID" 2>/dev/null | sed -n 's/.*phys_footprint: \([0-9]*\) B.*/\1/p')
  echo "$((i * 10)),$cpu,$fp" | tee -a "$D/samples.csv"
done
stop_app
size=$(stat -f %z "$BIN")
ls -l "$BIN"
acts=$(grep -c '"event":"recovery_action"' "$D/events.jsonl")
python3 - "$D/samples.csv" "$size" "$acts" <<'PY'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1])))
cpu = [float(r["cpu_pct"]) for r in rows]
fp = [int(r["phys_footprint_bytes"]) for r in rows if r["phys_footprint_bytes"]]
size, acts = int(sys.argv[2]), int(sys.argv[3])
checks = [
    ("samples", len(rows) == 60 and len(fp) == 60, f"{len(rows)} cpu, {len(fp)} footprint (need 60)"),
    ("mean CPU", sum(cpu) / len(cpu) < 0.5, f"{sum(cpu) / len(cpu):.2f} % (limit 0.5), max {max(cpu):.1f} %"),
    ("max physical footprint", max(fp) < 60 * 2**20, f"{max(fp) / 2**20:.1f} MB (limit 60), min {min(fp) / 2**20:.1f} MB"),
    ("binary size", size < 10 * 2**20, f"{size / 2**20:.2f} MB (limit 10)"),
    ("recovery_action lines", acts == 0, f"{acts} (limit 0)"),
]
ok = all(c[1] for c in checks)
print(f"AC-1: {'PASS' if ok else 'FAIL'}")
for name, good, shown in checks:
    print(f"  {'ok  ' if good else 'FAIL'} {name}: {shown}")
sys.exit(0 if ok else 1)
PY
finish $? 0
