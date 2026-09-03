#!/usr/bin/env bash
set -euo pipefail

KSSH="${KSSH:-target/debug/kssh}"
TEST_USER="${KADUOX_COMMAND_USER:-kaduox-command-ci}"
PORT="${KADUOX_COMMAND_PORT:-40423}"
WORK="${RUNNER_TEMP:-/tmp}/kaduox-ssh-command-integration"
KEY="$WORK/client_ed25519"
HOST_KEY="$WORK/ssh_host_ed25519_key"
SSH_DIR="/home/$TEST_USER/.ssh"
PID_FILE="$WORK/sshd.pid"
LOG="$WORK/sshd.log"
CONFIG="$WORK/sshd.conf"
REMOTE_CWD="/home/$TEST_USER/app release"
INJECTED="/home/$TEST_USER/kaduox-command-injected"

mkdir -p "$WORK"
chmod 700 "$WORK"

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
  echo "command integration failure: $*" >&2
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

run_kssh() {
  "$KSSH" 127.0.0.1 \
    --port "$PORT" \
    --user "$TEST_USER" \
    --identity "$KEY" \
    --host-key insecure \
    "$@"
}

[[ -x "$KSSH" ]] || fail "kssh binary not found at $KSSH"

if id "$TEST_USER" >/dev/null 2>&1; then
  sudo userdel -r "$TEST_USER" >/dev/null 2>&1 || true
fi
sudo useradd -m -s /bin/bash "$TEST_USER"
sudo passwd -d "$TEST_USER" >/dev/null

ssh-keygen -q -t ed25519 -N '' -f "$KEY"
ssh-keygen -q -t ed25519 -N '' -f "$HOST_KEY"
sudo install -d -m 0700 -o "$TEST_USER" -g "$TEST_USER" "$SSH_DIR"
sudo install -m 0600 -o "$TEST_USER" -g "$TEST_USER" "$KEY.pub" "$SSH_DIR/authorized_keys"
sudo install -d -m 0755 -o "$TEST_USER" -g "$TEST_USER" "$REMOTE_CWD"

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

sudo mkdir -p /run/sshd
sudo /usr/sbin/sshd -t -f "$CONFIG"
sudo /usr/sbin/sshd -f "$CONFIG" -E "$LOG"
wait_for_port || fail "command sshd did not start on port $PORT"

echo '[integration] typed exec preserves legacy argument behavior'
plain="$(run_kssh exec -- printf '%s' 'hello world')"
[[ "$plain" == 'hello world' ]] || fail "plain typed exec returned: $plain"

echo '[integration] typed exec working directory with spaces'
cwd="$(run_kssh exec --cwd "$REMOTE_CWD" -- pwd | tr -d '\r\n')"
[[ "$cwd" == "$REMOTE_CWD" ]] || fail "typed exec cwd returned: $cwd"

echo '[integration] typed exec environment value with spaces'
env_value="$(run_kssh exec --env 'APP_MESSAGE=hello typed world' -- sh -lc 'printf %s "$APP_MESSAGE"')"
[[ "$env_value" == 'hello typed world' ]] || fail "typed exec environment returned: $env_value"

echo '[integration] shell-active environment value remains literal data'
payload='$(touch /home/'"$TEST_USER"'/kaduox-command-injected)'
literal="$(run_kssh exec --env "PAYLOAD=$payload" -- sh -lc 'printf %s "$PAYLOAD"')"
[[ "$literal" == "$payload" ]] || fail "shell-active environment value changed: $literal"
if sudo test -e "$INJECTED"; then
  fail 'shell-active environment value executed unexpectedly'
fi

echo '[integration] invalid environment name fails before remote execution'
if run_kssh exec --env 'BAD-NAME=value' -- true >"$WORK/invalid-env.log" 2>&1; then
  fail 'invalid environment variable name unexpectedly succeeded'
fi
grep -q 'invalid remote environment variable name' "$WORK/invalid-env.log" || {
  cat "$WORK/invalid-env.log" >&2
  fail 'invalid environment variable error was not explicit'
}

echo '[integration] duplicate environment name fails closed'
if run_kssh exec --env 'MODE=one' --env 'MODE=two' -- true >"$WORK/duplicate-env.log" 2>&1; then
  fail 'duplicate environment variable unexpectedly succeeded'
fi
grep -q 'duplicate remote environment variable' "$WORK/duplicate-env.log" || {
  cat "$WORK/duplicate-env.log" >&2
  fail 'duplicate environment variable error was not explicit'
}
