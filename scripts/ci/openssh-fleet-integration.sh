#!/usr/bin/env bash
set -euo pipefail

FLEET="${KSSH_FLEET:-target/debug/kssh-fleet}"
TEST_USER="${KADUOX_FLEET_USER:-kaduox-fleet-ci}"
PORT="${KADUOX_FLEET_PORT:-40433}"
BAD_PORT="${KADUOX_FLEET_BAD_PORT:-40434}"
WORK="${RUNNER_TEMP:-/tmp}/kaduox-ssh-fleet-integration"
KEY="$WORK/client_ed25519"
HOST_KEY="$WORK/ssh_host_ed25519_key"
SSH_DIR="/home/$TEST_USER/.ssh"
PID_FILE="$WORK/sshd.pid"
LOG="$WORK/sshd.log"
CONFIG="$WORK/sshd.conf"
CLIENT_HOME="$WORK/client-home"
CLIENT_CONFIG="$CLIENT_HOME/.ssh/config"
PREFLIGHT_MARKER="/home/$TEST_USER/kaduox-fleet-preflight-marker"
RUNTIME_MARKER="/home/$TEST_USER/kaduox-fleet-runtime-marker"

mkdir -p "$WORK" "$CLIENT_HOME/.ssh"
chmod 700 "$WORK" "$CLIENT_HOME/.ssh"

cleanup() {
  set +e
  if [[ -f "$PID_FILE" ]]; then
    pid="$(cat "$PID_FILE" 2>/dev/null || true)"
    if [[ -n "$pid" ]]; then
      sudo kill "$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  fi
  sudo userdel -r "$TEST_USER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

fail() {
  echo "fleet integration failure: $*" >&2
  if [[ -f "$LOG" ]]; then
    sudo tail -n 120 "$LOG" >&2 || true
  fi
  exit 1
}

wait_for_port() {
  for _ in $(seq 1 100); do
    if (echo >/dev/tcp/127.0.0.1/"$PORT") >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

run_fleet() {
  HOME="$CLIENT_HOME" "$FLEET" \
    --identity "$KEY" \
    --host-key insecure \
    "$@"
}

[[ -x "$FLEET" ]] || fail "kssh-fleet binary not found at $FLEET"

if id "$TEST_USER" >/dev/null 2>&1; then
  sudo userdel -r "$TEST_USER" >/dev/null 2>&1 || true
fi
sudo useradd -m -s /bin/bash "$TEST_USER"
sudo passwd -d "$TEST_USER" >/dev/null

ssh-keygen -q -t ed25519 -N '' -f "$KEY"
ssh-keygen -q -t ed25519 -N '' -f "$HOST_KEY"
sudo install -d -m 0700 -o "$TEST_USER" -g "$TEST_USER" "$SSH_DIR"
sudo install -m 0600 -o "$TEST_USER" -g "$TEST_USER" "$KEY.pub" "$SSH_DIR/authorized_keys"

cat >"$CONFIG" <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey $HOST_KEY
PidFile $PID_FILE
AuthorizedKeysFile .ssh/authorized_keys
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PermitRootLogin no
UsePAM no
AllowUsers $TEST_USER
AllowTcpForwarding yes
AllowAgentForwarding yes
X11Forwarding no
PermitTunnel no
LogLevel VERBOSE
EOF

cat >"$CLIENT_CONFIG" <<EOF
Host fleet-a fleet-b
  HostName 127.0.0.1
  Port $PORT
  User $TEST_USER
Host fleet-bad
  HostName 127.0.0.1
  Port $BAD_PORT
  User $TEST_USER
EOF
chmod 600 "$CLIENT_CONFIG"

sudo mkdir -p /run/sshd
sudo /usr/sbin/sshd -t -f "$CONFIG"
sudo /usr/sbin/sshd -f "$CONFIG" -E "$LOG"
wait_for_port || fail "fleet sshd did not start on port $PORT"

sudo rm -f "$PREFLIGHT_MARKER" "$RUNTIME_MARKER"

echo '[integration] fleet executes typed command on two aliases concurrently'
run_fleet -H fleet-a -H fleet-b --jobs 2 -- printf '%s' 'fleet hello' >"$WORK/two-hosts.log" 2>&1 || {
  cat "$WORK/two-hosts.log" >&2
  fail 'two-host fleet command failed'
}
grep -q 'target=fleet-a' "$WORK/two-hosts.log" || fail 'fleet-a result block missing'
grep -q 'target=fleet-b' "$WORK/two-hosts.log" || fail 'fleet-b result block missing'
[[ "$(grep -c 'fleet hello' "$WORK/two-hosts.log")" -eq 2 ]] || {
  cat "$WORK/two-hosts.log" >&2
  fail 'expected command output from both fleet aliases'
}

echo '[integration] malformed target fails before any remote command side effect'
if run_fleet \
  -H fleet-a \
  -H 'bad.example:22' \
  -- sh -c "touch '$PREFLIGHT_MARKER'" >"$WORK/preflight.log" 2>&1; then
  fail 'malformed host:port fleet target unexpectedly succeeded'
fi
if sudo test -e "$PREFLIGHT_MARKER"; then
  cat "$WORK/preflight.log" >&2
  fail 'valid fleet target executed before malformed-target preflight completed'
fi

echo '[integration] runtime connection failure is isolated from valid target'
if run_fleet \
  -H fleet-a \
  -H fleet-bad \
  --jobs 2 \
  -- sh -c "printf '%s' runtime-ok > '$RUNTIME_MARKER'" >"$WORK/runtime-failure.log" 2>&1; then
  fail 'fleet invocation with unreachable runtime target unexpectedly succeeded'
fi
sudo test -f "$RUNTIME_MARKER" || {
  cat "$WORK/runtime-failure.log" >&2
  fail 'valid target did not execute while independent target failed at runtime'
}
[[ "$(sudo cat "$RUNTIME_MARKER")" == 'runtime-ok' ]] || fail 'valid target marker content mismatch'
grep -q 'target=fleet-a' "$WORK/runtime-failure.log" || fail 'successful fleet-a result missing'
grep -q 'target=fleet-bad' "$WORK/runtime-failure.log" || fail 'failed fleet-bad result missing'
grep -q 'connect/authentication failed' "$WORK/runtime-failure.log" || {
  cat "$WORK/runtime-failure.log" >&2
  fail 'runtime connection failure was not reported per target'
}

echo '[integration] output retention cap truncates without stalling remote command'
run_fleet \
  -H fleet-a \
  -H fleet-b \
  --jobs 2 \
  --output-limit 32 \
  -- sh -c 'head -c 128 /dev/zero | tr "\000" A' >"$WORK/truncated.log" 2>&1 || {
  cat "$WORK/truncated.log" >&2
  fail 'bounded-output fleet command failed'
}
[[ "$(grep -c -- '--- stdout (truncated) ---' "$WORK/truncated.log")" -eq 2 ]] || {
  cat "$WORK/truncated.log" >&2
  fail 'expected both fleet result streams to report truncation'
}

echo '[integration] aggregated output escapes terminal controls by default'
control_payload=$'\033[31mred\rX\n'
run_fleet -H fleet-a -- printf '%s' "$control_payload" >"$WORK/terminal-safe.log" 2>&1 || {
  cat "$WORK/terminal-safe.log" >&2
  fail 'terminal-safety fleet command failed'
}
grep -Fq '\x1b[31mred\rX' "$WORK/terminal-safe.log" || {
  cat "$WORK/terminal-safe.log" >&2
  fail 'fleet output did not render ESC/CR as visible escapes'
}

printf '[integration] fleet OpenSSH suite passed\n'
