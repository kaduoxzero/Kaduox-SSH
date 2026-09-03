#!/usr/bin/env bash
set -euo pipefail

KSSH="${KSSH:-target/debug/kssh}"
TEST_USER="${KADUOX_AUTH_DISCOVERY_USER:-kaduox-auth-ci}"
TARGET_PORT="${KADUOX_AUTH_TARGET_PORT:-40324}"
JUMP_PORT="${KADUOX_AUTH_JUMP_PORT:-40325}"
WORK="${RUNNER_TEMP:-/tmp}/kaduox-ssh-auth-discovery-integration"
CLIENT_KEY="$WORK/client_ed25519"
TARGET_HOST_KEY="$WORK/target_host_ed25519"
JUMP_HOST_KEY="$WORK/jump_host_ed25519"
AUTH_HOME="$WORK/home"
SSH_DIR="/home/$TEST_USER/.ssh"

mkdir -p "$WORK" "$AUTH_HOME/.ssh"
chmod 700 "$WORK" "$AUTH_HOME/.ssh"

cleanup() {
  set +e
  for name in target jump; do
    pid_file="$WORK/$name.pid"
    if [[ -f "$pid_file" ]]; then
      pid="$(cat "$pid_file" 2>/dev/null || true)"
      if [[ -n "$pid" ]]; then
        sudo kill "$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
      fi
    fi
  done
  sudo userdel -r "$TEST_USER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

fail() {
  echo "auth-discovery integration failure: $*" >&2
  for log in "$WORK"/*.log; do
    [[ -f "$log" ]] || continue
    echo "--- $log ---" >&2
    sudo tail -n 120 "$log" >&2 || true
  done
  exit 1
}

wait_for_port() {
  local port="$1"
  for _ in $(seq 1 100); do
    if (echo >/dev/tcp/127.0.0.1/"$port") >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

start_sshd() {
  local name="$1"
  local port="$2"
  local host_key="$3"
  local config="$WORK/$name.conf"
  local pid_file="$WORK/$name.pid"
  local log="$WORK/$name.log"

  cat >"$config" <<EOF
Port $port
ListenAddress 127.0.0.1
HostKey $host_key
PidFile $pid_file
AuthorizedKeysFile .ssh/authorized_keys
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PermitRootLogin no
UsePAM no
AllowUsers $TEST_USER
AllowTcpForwarding yes
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
LogLevel VERBOSE
EOF

  sudo /usr/sbin/sshd -t -f "$config"
  sudo /usr/sbin/sshd -f "$config" -E "$log"
  wait_for_port "$port" || fail "$name sshd did not start on port $port"
}

[[ -x "$KSSH" ]] || fail "kssh binary not found at $KSSH"

if id "$TEST_USER" >/dev/null 2>&1; then
  sudo userdel -r "$TEST_USER" >/dev/null 2>&1 || true
fi
sudo useradd -m -s /bin/bash "$TEST_USER"
sudo passwd -d "$TEST_USER" >/dev/null

ssh-keygen -q -t ed25519 -N '' -f "$CLIENT_KEY"
ssh-keygen -q -t ed25519 -N '' -f "$TARGET_HOST_KEY"
ssh-keygen -q -t ed25519 -N '' -f "$JUMP_HOST_KEY"
sudo install -d -m 0700 -o "$TEST_USER" -g "$TEST_USER" "$SSH_DIR"
sudo install -m 0600 -o "$TEST_USER" -g "$TEST_USER" "$CLIENT_KEY.pub" "$SSH_DIR/authorized_keys"

cat >"$AUTH_HOME/.ssh/config" <<EOF
Host default-target
  HostName 127.0.0.1
  Port $TARGET_PORT
  User $TEST_USER

Host default-jump
  HostName 127.0.0.1
  Port $JUMP_PORT
  User $TEST_USER
EOF
chmod 600 "$AUTH_HOME/.ssh/config"

# The authentication path must succeed because of default identity discovery,
# not because an ambient agent happens to contain the generated key.
unset SSH_AUTH_SOCK || true
install -m 0600 "$CLIENT_KEY" "$AUTH_HOME/.ssh/id_ed25519"

sudo mkdir -p /run/sshd
start_sshd target "$TARGET_PORT" "$TARGET_HOST_KEY"
start_sshd jump "$JUMP_PORT" "$JUMP_HOST_KEY"

echo '[integration] default identity discovery without IdentityFile'
direct="$(HOME="$AUTH_HOME" "$KSSH" default-target --host-key insecure exec -- printf default-identity-ok)"
[[ "$direct" == 'default-identity-ok' ]] || fail "default identity discovery returned: $direct"

echo '[integration] ProxyJump uses default identity discovery for every hop'
via_jump="$(HOME="$AUTH_HOME" "$KSSH" default-target --host-key insecure -J default-jump exec -- printf default-jump-ok)"
[[ "$via_jump" == 'default-jump-ok' ]] || fail "ProxyJump default identity discovery returned: $via_jump"
