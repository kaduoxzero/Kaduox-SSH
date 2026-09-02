#!/usr/bin/env bash
set -euo pipefail

KSSH="${KSSH:-target/release/kssh}"
PORT="${KADUOX_BENCH_PORT:-41222}"
SIZE_MB="${KADUOX_BENCH_SIZE_MB:-64}"
ITERATIONS="${KADUOX_BENCH_ITERATIONS:-10}"
SMALL_FILES="${KADUOX_BENCH_SMALL_FILES:-200}"
WORK="${RUNNER_TEMP:-/tmp}/kaduox-ssh-bench"
USER_NAME="$(id -un)"
KEY="$WORK/client_ed25519"
HOST_KEY="$WORK/host_ed25519"
AUTH_KEYS="$WORK/authorized_keys"
CONFIG="$WORK/sshd.conf"
PID_FILE="$WORK/sshd.pid"
LOG="$WORK/sshd.log"
REMOTE_DIR="/tmp/kaduox-bench-$USER_NAME"

rm -rf "$WORK"
mkdir -p "$WORK"
chmod 700 "$WORK"

cleanup() {
  set +e
  if [[ -f "$PID_FILE" ]]; then
    sudo kill "$(cat "$PID_FILE")" 2>/dev/null || true
  fi
  "$KSSH" 127.0.0.1 --port "$PORT" --user "$USER_NAME" --identity "$KEY" --host-key insecure exec -- rm -rf "$REMOTE_DIR" >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_for_port() {
  for _ in $(seq 1 100); do
    if (echo >/dev/tcp/127.0.0.1/"$PORT") >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

now_ns() {
  date +%s%N
}

elapsed_ms() {
  local start="$1"
  local end="$2"
  awk -v s="$start" -v e="$end" 'BEGIN { printf "%.3f", (e-s)/1000000 }'
}

throughput_mib() {
  local mib="$1"
  local start="$2"
  local end="$3"
  awk -v m="$mib" -v s="$start" -v e="$end" 'BEGIN { printf "%.2f", m/((e-s)/1000000000) }'
}

run_kssh() {
  "$KSSH" 127.0.0.1 \
    --port "$PORT" \
    --user "$USER_NAME" \
    --identity "$KEY" \
    --host-key insecure \
    "$@"
}

if [[ ! -x "$KSSH" ]]; then
  echo "benchmark error: kssh binary not found at $KSSH" >&2
  exit 1
fi

ssh-keygen -q -t ed25519 -N '' -f "$KEY"
ssh-keygen -q -t ed25519 -N '' -f "$HOST_KEY"
cp "$KEY.pub" "$AUTH_KEYS"
chmod 600 "$AUTH_KEYS"

cat >"$CONFIG" <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey $HOST_KEY
PidFile $PID_FILE
AuthorizedKeysFile $AUTH_KEYS
StrictModes no
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
AllowUsers $USER_NAME
AllowTcpForwarding yes
Subsystem sftp internal-sftp
LogLevel ERROR
EOF

sudo mkdir -p /run/sshd
sudo /usr/sbin/sshd -t -f "$CONFIG"
sudo /usr/sbin/sshd -f "$CONFIG" -E "$LOG"
wait_for_port || { cat "$LOG" >&2 || true; exit 1; }

run_kssh exec -- mkdir -p "$REMOTE_DIR"

printf 'metric,value,unit\n'

total_ns=0
for _ in $(seq 1 "$ITERATIONS"); do
  start="$(now_ns)"
  run_kssh exec -- true >/dev/null
  end="$(now_ns)"
  total_ns=$((total_ns + end - start))
done
avg_ms="$(awk -v total="$total_ns" -v n="$ITERATIONS" 'BEGIN { printf "%.3f", total/n/1000000 }')"
printf 'connect_exec_avg,%s,ms\n' "$avg_ms"

large="$WORK/large.bin"
dd if=/dev/zero of="$large" bs=1M count="$SIZE_MB" status=none

start="$(now_ns)"
run_kssh upload "$large" "$REMOTE_DIR/large.bin" --write-concurrency 32 --packet-size 262144 >/dev/null
end="$(now_ns)"
printf 'sftp_upload,%s,MiB/s\n' "$(throughput_mib "$SIZE_MB" "$start" "$end")"

start="$(now_ns)"
run_kssh download "$REMOTE_DIR/large.bin" "$WORK/download.bin" --write-concurrency 32 --packet-size 262144 >/dev/null
end="$(now_ns)"
printf 'sftp_download,%s,MiB/s\n' "$(throughput_mib "$SIZE_MB" "$start" "$end")"
cmp "$large" "$WORK/download.bin"

mkdir -p "$WORK/small"
for i in $(seq 1 "$SMALL_FILES"); do
  printf '%04096d' "$i" >"$WORK/small/file-$i.txt"
done
start="$(now_ns)"
run_kssh upload "$WORK/small" "$REMOTE_DIR/small" -r --jobs 16 --write-concurrency 16 >/dev/null
end="$(now_ns)"
printf 'small_tree_upload,%s,ms\n' "$(elapsed_ms "$start" "$end")"
printf 'small_tree_files,%s,count\n' "$SMALL_FILES"

printf 'benchmark_size,%s,MiB\n' "$SIZE_MB"
printf 'benchmark_iterations,%s,count\n' "$ITERATIONS"
