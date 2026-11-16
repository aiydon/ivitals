#!/usr/bin/env bash
# ivitals remote collector.
#
# Delivered embedded in the binary; executed on the remote host as
#   printf '%s' '<b64>' | base64 -d | bash -s -- --alias <a> --topn <n> [--processes]
# so the script body never passes through shell quoting.
#
# Output (stdout):
#   line 1: metrics JSON (core schema, no extras yet)
#   line 2: processes JSON (only with --processes)
#
# Pseudo-value policy: an unavailable metric omits its field entirely.
# GPU: no nvidia-smi -> absent; nvidia-smi present but failing -> error.
set -u

ALIAS=""
TOPN=20
EMIT_PROCESSES=0
while [ $# -gt 0 ]; do
  case "$1" in
    --alias) ALIAS="${2:-}"; shift 2 ;;
    --topn) TOPN="${2:-20}"; shift 2 ;;
    --processes) EMIT_PROCESSES=1; shift ;;
    --extras) shift 2 ;;
    *) shift ;;
  esac
done
[ -n "$ALIAS" ] || ALIAS="remote"

now_ms() { date +%s%3N; }
T0=$(now_ms)

# ---------- CPU: two /proc/stat samples 200 ms apart ----------
cpu_sample() {
  awk '/^cpu /{idle=$5+$6; total=0; for(i=2;i<=NF;i++) total+=$i; printf "%d %d", total, idle}' /proc/stat
}
read -r S1T S1I <<<"$(cpu_sample)"
sleep 0.2
read -r S2T S2I <<<"$(cpu_sample)"
S1T=${S1T:-0}; S1I=${S1I:-0}; S2T=${S2T:-0}; S2I=${S2I:-0}
CPU_PCT=$(awk -v dt=$((S2T - S1T)) -v di=$((S2I - S1I)) \
  'BEGIN { if (dt > 0) printf "%.1f", (1 - di/dt) * 100; else printf "0.0" }')
CORES=$(grep -c '^processor' /proc/cpuinfo)
read -r LOAD1 LOAD5 LOAD15 _ < /proc/loadavg
LOAD1=${LOAD1:-0}

# ---------- memory ----------
read -r MT MU MA <<<"$(awk '/^MemTotal:/{t=$2*1024} /^MemAvailable:/{a=$2*1024} END{printf "%d %d %d", t, t-a, a}' /proc/meminfo)"
MT=${MT:-0}; MU=${MU:-0}; MA=${MA:-0}

# ---------- gpu ----------
GPU_JSON='{"state":"absent"}'
if command -v nvidia-smi >/dev/null 2>&1; then
  if SMI_OUT=$(nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu \
        --format=csv,noheader,nounits 2>/dev/null); then
    if [ -n "$SMI_OUT" ]; then
      GPUS=$(printf '%s\n' "$SMI_OUT" | awk -F', *' '{
        gsub(/"/, "\\\"", $2)
        printf "%s{\"index\":%s,\"name\":\"%s\",\"util_percent\":%s,\"mem_used_mb\":%s,\"mem_total_mb\":%s,\"temp_c\":%s}",
               (NR > 1 ? "," : ""), $1, $2, $3, $4, $5, $6
      }')
      GPU_JSON="{\"state\":\"ok\",\"gpus\":[${GPUS}]}"
    else
      GPU_JSON='{"state":"absent"}'
    fi
  else
    SMI_ERR=$(nvidia-smi 2>&1 >/dev/null | head -c 200 | tr -d '"\\')
    GPU_JSON="{\"state\":\"error\",\"message\":\"nvidia-smi failed: ${SMI_ERR}\"}"
  fi
fi

# ---------- metrics line ----------
TS=$(now_ms)
COLLECT_MS=$((TS - T0))
printf '{"schema_version":1,"ts_ms":%s,"server":"%s","mode":"remote","collect_ms":%s,"cpu":{"global_percent":%s,"core_count":%s,"load_avg_1m":%s},"memory":{"total_bytes":%s,"used_bytes":%s,"available_bytes":%s},"gpu":%s}\n' \
  "$TS" "$ALIAS" "$COLLECT_MS" "$CPU_PCT" "$CORES" "$LOAD1" "$MT" "$MU" "$MA" "$GPU_JSON"

# ---------- processes ----------
if [ "$EMIT_PROCESSES" = "1" ]; then
  TOTAL=$(ls /proc 2>/dev/null | grep -c '^[0-9]')
  ROWS=$(ps -eo pid=,pcpu=,pmem=,state=,time=,comm= --sort=-pcpu | head -n "$TOPN" | awk '{
    pid=$1; pcpu=$2; pmem=$3; st=$4; tm=$5
    name=$6; for (i=7; i<=NF; i++) name = name " " $i
    n = split(tm, t, ":")
    ms = (n == 3) ? (t[1]*3600 + t[2]*60 + t[3]) * 1000 : (t[1]*60 + t[2]) * 100
    gsub(/"/, "", name)
    printf "%s{\"pid\":%s,\"name\":\"%s\",\"cpu_percent\":%s,\"mem_percent\":%s,\"state\":\"%s\",\"cum_cpu_ms\":%s}",
           (NR > 1 ? "," : ""), pid, name, pcpu, pmem, st, ms
  }')
  printf '{"top_n":%s,"total_procs":%s,"processes":[%s]}\n' "$TOPN" "$TOTAL" "$ROWS"
fi
