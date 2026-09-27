#!/bin/sh
# Compact read-only Linux snapshot consumed by the MCP server wire format.
# Emits __KADUOX_SYSTEM_METRICS_V1__ followed by key=value lines. Never
# interpolates user input. Keep pure ASCII.
printf '__KADUOX_SYSTEM_METRICS_V1__\n'
printf 'hostname='; hostname 2>/dev/null
printf '\nplatform='; uname -srmo 2>/dev/null
printf '\nusername='; id -un 2>/dev/null
printf '\nuptime='; uptime -p 2>/dev/null || uptime 2>/dev/null
printf '\naddresses='; hostname -I 2>/dev/null
printf '\ncpu_model='; awk -F: '/model name|Hardware|Processor/{gsub(/^[ \t]+/,"",$2); print $2; exit}' /proc/cpuinfo 2>/dev/null
printf '\ncpu_cores='; nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null
printf '\nload='; cut -d' ' -f1-3 /proc/loadavg 2>/dev/null
printf '\nmemory='; free -b 2>/dev/null | awk '/^Mem:/{print $2,$3,$7; exit}'
printf '\ndisk='; df -P -B1 / 2>/dev/null | awk 'NR==2{gsub(/%/,"",$5); print $2,$3,$4,$5; exit}'
gpu_name=$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -n1)
[ -n "$gpu_name" ] || gpu_name=$(lspci 2>/dev/null | grep -Ei 'vga|3d|display' | head -n1)
printf '\ngpu_name=%s\n' "$gpu_name"
printf 'gpu_usage='; nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>/dev/null | head -n1
printf '\ngpu_memory='; nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader,nounits 2>/dev/null | head -n1
printf '\ngpu_temperature='; nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>/dev/null | head -n1
printf '\nnetwork='; awk 'BEGIN{rx=0;tx=0} /:/{gsub(/:/,"",$1); if ($1 != "lo"){rx+=$2; tx+=$10}} END{print rx,tx}' /proc/net/dev 2>/dev/null
