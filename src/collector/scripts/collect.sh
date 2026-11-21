#!/usr/bin/env bash
# ivitals remote collector (schema v2).
#
# Delivered embedded in the binary; executed on the remote host as
#   printf '%s' '<b64>' | base64 -d | bash -s -- --alias <a> --topn <n> [--processes]
# so the script body never passes through shell quoting.
#
# Output (stdout):
#   line 1: metrics JSON (core schema)
#   line 2: processes JSON (only with --processes), including state counts,
#           thread count and GPU compute processes
#   then, for enabled extras families (metrics runs only): one
#   "## <section>" header line followed by the raw source text.
#
# Pseudo-value policy: an unavailable metric omits its field entirely.
# GPU: no nvidia-smi -> absent; nvidia-smi present but failing -> error.
set -u

ALIAS=""
TOPN=50
EMIT_PROCESSES=0
EX_CPU=0
EX_MEM=0
EX_DISK=0
EX_NET=0
EX_GPU=0
EX_SLURM=0
EX_PSI=0
EX_FS=0
EX_HOST=0
while [ $# -gt 0 ]; do
  case "$1" in
    --alias) ALIAS="${2:-}"; shift 2 ;;
    --topn) TOPN="${2:-50}"; shift 2 ;;
    --processes) EMIT_PROCESSES=1; shift ;;
    --cpu-detail) EX_CPU=1; shift ;;
    --memory-detail) EX_MEM=1; shift ;;
    --disk-io) EX_DISK=1; shift ;;
    --net) EX_NET=1; shift ;;
    --gpu-detail) EX_GPU=1; shift ;;
    --slurm) EX_SLURM=1; shift ;;
    --psi) EX_PSI=1; shift ;;
    --fs) EX_FS=1; shift ;;
    --host) EX_HOST=1; shift ;;
    *) shift ;;
  esac
done
[ -n "$ALIAS" ] || ALIAS="remote"

now_ms() {
  local t="${EPOCHREALTIME:-}"
  if [ -n "$t" ]; then
    t=${t//[.,]/}
    printf '%s' "$((10#$t / 1000))"
    return
  fi
  local s ns
  s=$(date +%s)
  ns=$(date +%N 2>/dev/null || echo 0)
  printf '%s' "$((s * 1000 + 10#${ns:0:3}))"
}
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
  if SMI_OUT=$(nvidia-smi --query-gpu=index,name,utilization.gpu,utilization.memory,memory.used,memory.total,temperature.gpu \
        --format=csv,noheader,nounits 2>/dev/null); then
    if [ -n "$SMI_OUT" ]; then
      GPUS=$(printf '%s\n' "$SMI_OUT" | awk -F', *' '
        function esc(s,   i,c,out) {
          out="";
          for (i=1; i<=length(s); i++) {
            c=substr(s,i,1);
            if (c=="\\") out=out "\\\\";
            else if (c=="\"") out=out "\\\"";
            else if (c=="\t") out=out "\\t";
            else if (c ~ /[\r\n]/) out=out " ";
            else out=out c;
          }
          return out;
        }
        {
          mem_util = ($6+0 > 0) ? sprintf("\"mem_util_percent\":%.2f,", ($5/$6)*100) : "";
          printf "%s{\"index\":%s,\"name\":\"%s\",\"util_percent\":%s,%s\"mem_used_mb\":%s,\"mem_total_mb\":%s,\"temp_c\":%s}",
                 (NR > 1 ? "," : ""), $1, esc($2), $3, mem_util, $5, $6, $7;
        }')
      GPU_JSON="{\"state\":\"ok\",\"gpus\":[${GPUS}]}"
    else
      GPU_JSON='{"state":"absent"}'
    fi
  else
    SMI_ERR=$(nvidia-smi 2>&1 >/dev/null | head -c 200 | tr -d '"\\\r\n')
    GPU_JSON="{\"state\":\"error\",\"message\":\"nvidia-smi failed: ${SMI_ERR}\"}"
  fi
fi

# ---------- metrics line ----------
TS=$(now_ms)
COLLECT_MS=$((TS - T0))
printf '{"schema_version":2,"ts_ms":%s,"server":"%s","mode":"remote","collect_ms":%s,"cpu":{"global_percent":%s,"core_count":%s,"load_avg_1m":%s},"memory":{"total_bytes":%s,"used_bytes":%s,"available_bytes":%s},"gpu":%s}\n' \
  "$TS" "$ALIAS" "$COLLECT_MS" "$CPU_PCT" "$CORES" "$LOAD1" "$MT" "$MU" "$MA" "$GPU_JSON"

# ---------- processes ----------
if [ "$EMIT_PROCESSES" = "1" ]; then
  TOTAL=$(ls /proc 2>/dev/null | grep -c '^[0-9]')
  # GNU ps: nlwp = light-weight processes (threads), rss in KiB.
  TOTAL_THREADS=$(ps -e -o nlwp= 2>/dev/null | awk '{s+=$1} END{printf "%d", s+0}')
  TOTAL_THREADS=${TOTAL_THREADS:-0}
  STATE_JSON=$(ps -eo state= 2>/dev/null | awk '{s=$1; if (s=="") s="?"; c[s]++} END{first=1; for (k in c) {printf "%s\"%s\":%d", (first?"":","), k, c[k]; first=0}}')
  ROWS=$(ps -eo pid=,ppid=,pcpu=,pmem=,state=,time=,nlwp=,rss=,user=,comm= --sort=-pcpu 2>/dev/null | head -n "$TOPN" | while read -r pid ppid pcpu pmem st tm nlwp rss user comm; do
    [ -n "$pid" ] || continue
    uid=$(awk '/^Uid:/{print $2; exit}' /proc/$pid/status 2>/dev/null)
    gid=$(awk '/^Gid:/{print $2; exit}' /proc/$pid/status 2>/dev/null)
    cmd=$(tr '\0' ' ' < /proc/$pid/cmdline 2>/dev/null | cut -c 1-512 | tr '\t\n' '  ' | sed -E 's/[[:space:]]+$//')
    cwd=$(readlink /proc/$pid/cwd 2>/dev/null | cut -c 1-256 | tr '\t\n' '  ' | sed -E 's/[[:space:]]+$//')
    exe=$(readlink /proc/$pid/exe 2>/dev/null | cut -c 1-256 | tr '\t\n' '  ' | sed -E 's/[[:space:]]+$//')
    parent=$(cat /proc/$ppid/comm 2>/dev/null | cut -c 1-64 | tr '\t\n' '  ' | sed -E 's/[[:space:]]+$//')
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$pid" "$ppid" "$user" "$uid" "$gid" "$pcpu" "$pmem" "$st" "$tm" "$nlwp" "$rss" "$comm" "$cmd" "$cwd" "$exe" "$parent"
  done | awk -F '\t' '
    function esc(s,   i,c,out) {
      out="";
      for (i=1; i<=length(s); i++) {
        c=substr(s,i,1);
        if (c=="\\") out=out "\\\\";
        else if (c=="\"") out=out "\\\"";
        else if (c=="\t") out=out "\\t";
        else out=out c;
      }
      return out;
    }
    function text(key, value) {
      if (value == "") return "";
      return "\"" key "\":\"" esc(value) "\",";
    }
    function number(key, value) {
      if (value !~ /^[0-9]+$/) return "";
      return "\"" key "\":" value ",";
    }
    {
      pid=$1; ppid=$2; user=$3; uid=$4; gid=$5
      pcpu=$6; pmem=$7; st=$8; tm=$9; threads=$10; rss_kb=$11
      name=$12; cmd=$13; cwd=$14; exe=$15; parent=$16
      n_days = split(tm, d, "-")
      if (n_days == 2) { days=d[1]; clock=d[2] } else { days=0; clock=tm }
      n = split(clock, t, ":")
      if (n == 3) secs = t[1]*3600 + t[2]*60 + t[3]
      else secs = t[1]*60 + t[2]
      ms = (days*86400 + secs) * 1000
      printf "%s{\"pid\":%s,\"ppid\":%s,%s%s%s\"name\":\"%s\",%s%s%s%s\"cpu_percent\":%s,\"mem_percent\":%s,\"rss_bytes\":%s,\"threads\":%s,\"state\":\"%s\",\"cum_cpu_ms\":%s}",
        (count++ ? "," : ""), pid, ppid,
        number("uid", uid), number("gid", gid), text("user", user),
        esc(name),
        text("cmdline", cmd), text("cwd", cwd), text("exe", exe), text("parent_name", parent),
        pcpu, pmem, rss_kb*1024, threads, st, ms
    }')
  GPU_PROC_JSON=""
  if command -v nvidia-smi >/dev/null 2>&1; then
    GPU_PROC_JSON=$(nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name,used_memory \
      --format=csv,noheader,nounits 2>/dev/null | awk -F', *' '
      function esc(s,   i,c,out) {
        out="";
        for (i=1; i<=length(s); i++) {
          c=substr(s,i,1);
          if (c=="\\") out=out "\\\\";
          else if (c=="\"") out=out "\\\"";
          else if (c=="\t") out=out "\\t";
          else out=out c;
        }
        return out;
      }
      NF >= 4 {
        printf "%s{\"gpu_index\":0,\"gpu_uuid\":\"%s\",\"pid\":%s,\"name\":\"%s\",\"mem_mb\":%s,\"kind\":\"compute\"}",
               (NR > 1 ? "," : ""), esc($1), $2, esc($3), $4;
      }')
  fi
  printf '{"schema_version":2,"ts_ms":%s,"server":"%s","mode":"remote","collect_ms":%s,"top_n":%s,"total_procs":%s,"total_threads":%s,"state_counts":{%s},"gpu_processes":[%s],"processes":[%s]}\n' \
    "$TS" "$ALIAS" "$COLLECT_MS" "$TOPN" "$TOTAL" "$TOTAL_THREADS" "$STATE_JSON" "$GPU_PROC_JSON" "$ROWS"
fi

# ---------- extras sections (metrics runs only) ----------
# Each enabled family emits a "## <name>" header plus raw source text.
# A header with empty content means "source missing on this host"; the
# Rust parser treats that identically to a missing section (field absent).
if [ "$EX_CPU" = 1 ] || [ "$EX_MEM" = 1 ] || [ "$EX_DISK" = 1 ] || \
   [ "$EX_NET" = 1 ] || [ "$EX_PSI" = 1 ] || [ "$EX_GPU" = 1 ] || \
   [ "$EX_SLURM" = 1 ] || [ "$EX_FS" = 1 ] || [ "$EX_HOST" = 1 ]; then
  echo ""
  if [ "$EX_HOST" = 1 ]; then
    echo "## host"
    printf 'hostname=%s\n' "$(hostname 2>/dev/null | tr -d '\n')"
    printf 'os=%s\n' "$( . /etc/os-release 2>/dev/null; printf '%s' "${PRETTY_NAME:-}" | tr -d '\n')"
    printf 'kernel=%s\n' "$(uname -r 2>/dev/null | tr -d '\n')"
    printf 'arch=%s\n' "$(uname -m 2>/dev/null | tr -d '\n')"
    printf 'cpu_model=%s\n' "$(awk -F': ' '/model name/{print $2; exit}' /proc/cpuinfo 2>/dev/null | tr -d '\n')"
    printf 'core_count=%s\n' "$(nproc 2>/dev/null || grep -c '^processor' /proc/cpuinfo)"
    printf 'mem_total_bytes=%s\n' "$(awk '/^MemTotal:/{printf "%.0f", $2*1024}' /proc/meminfo)"
    printf 'uptime_secs=%s\n' "$(cut -d. -f1 /proc/uptime)"
    printf 'gpu_models=%s\n' "$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | paste -sd'|' - | tr -d '\n')"
  fi
  if [ "$EX_CPU" = 1 ]; then
    echo "## cpu_detail"
    cat /proc/stat 2>/dev/null
    echo "#loadavg"
    cat /proc/loadavg 2>/dev/null
  fi
  if [ "$EX_MEM" = 1 ]; then
    echo "## memory_detail"
    cat /proc/meminfo 2>/dev/null
    echo "#vmstat"
    grep -E '^(oom_kill|pswpin|pswpout) ' /proc/vmstat 2>/dev/null
  fi
  if [ "$EX_DISK" = 1 ]; then
    echo "## diskstats"
    cat /proc/diskstats 2>/dev/null
  fi
  if [ "$EX_NET" = 1 ]; then
    echo "## net"
    cat /proc/net/dev 2>/dev/null
    echo "#snmp_tcp"
    grep -A1 '^Tcp:' /proc/net/snmp 2>/dev/null
  fi
  if [ "$EX_PSI" = 1 ]; then
    if [ -r /proc/pressure/cpu ]; then
      echo "## psi_cpu"
      cat /proc/pressure/cpu 2>/dev/null
    fi
    if [ -r /proc/pressure/memory ]; then
      echo "## psi_mem"
      cat /proc/pressure/memory 2>/dev/null
    fi
    if [ -r /proc/pressure/io ]; then
      echo "## psi_io"
      cat /proc/pressure/io 2>/dev/null
    fi
  fi
  if [ "$EX_FS" = 1 ]; then
    echo "## filesystems"
    df -P -T -B1 2>/dev/null
  fi
  if [ "$EX_GPU" = 1 ] && command -v nvidia-smi >/dev/null 2>&1; then
    echo "## gpu_detail"
    nvidia-smi --query-gpu=index,power.draw,fan.speed,clocks.sm,clocks.mem,pstate,clocks_throttle_reasons.active \
      --format=csv,noheader,nounits 2>/dev/null || true
  fi
  if [ "$EX_SLURM" = 1 ] && command -v sinfo >/dev/null 2>&1; then
    echo "## slurm_nodes"
    sinfo -N -h -o '%T %c %C %G' 2>/dev/null || true
    echo "## slurm_jobs"
    squeue -h -o '%t' 2>/dev/null || true
  fi
fi
exit 0
