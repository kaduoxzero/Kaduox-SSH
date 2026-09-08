#!/bin/sh
# Read-only Linux snapshot. Never interpolates user input.
export LC_ALL=C
printf '__KADUOX_SYSTEM_METRICS_V1__\n'
printf 'hostname='; hostname 2>/dev/null
printf '\nplatform='; uname -srmo 2>/dev/null
printf '\nusername='; id -un 2>/dev/null
printf '\nuptime='; uptime -p 2>/dev/null || uptime 2>/dev/null
printf '\naddresses='; hostname -I 2>/dev/null
printf '\ncpu_model='
awk -F: '/model name|Hardware|Processor/{gsub(/^[ \t]+/,"",$2); print $2; exit}' /proc/cpuinfo 2>/dev/null
printf '\ncpu_cores='; nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null
printf '\nload='; cut -d' ' -f1-3 /proc/loadavg 2>/dev/null
before=$(awk '/^cpu /{t=0;for(i=2;i<=9;i++)t+=$i;print t,$5+$6;exit}' /proc/stat 2>/dev/null)
sleep 0.2
printf '\ncpu_usage='
awk -v before="$before" '/^cpu /{split(before,b," ");t=0;for(i=2;i<=9;i++)t+=$i;d=t-b[1]; if(d>0)printf "%.2f\n",100*(d-($5+$6-b[2]))/d;exit}' /proc/stat 2>/dev/null
printf '\nmemory='
awk '/^MemTotal:/{t=$2} /^MemAvailable:/{a=$2} END{if(t>0)printf "%.0f %.0f %.0f\n",t*1024,(t-a)*1024,a*1024}' /proc/meminfo 2>/dev/null
printf '\ndisk='; df -P -B1 / 2>/dev/null | awk 'NR==2{gsub(/%/,"",$5);print $2,$3,$4,$5}'
nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits 2>/dev/null | while IFS= read -r device; do printf '\ngpu=%s\n' "$device"; done
printf '\nnetwork='
awk 'BEGIN{rx=0;tx=0} /:/{gsub(/:/,"",$1);if($1!="lo"){rx+=$2;tx+=$10}} END{printf "%.0f %.0f\n",rx,tx}' /proc/net/dev 2>/dev/null
