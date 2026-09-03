#!/usr/bin/env bash
set -euo pipefail

KSSH="${KSSH:-target/debug/kssh}"
TEST_USER="${KADUOX_PROBE_USER:-kaduox-probe-ci}"
PORT="${KADUOX_PROBE_PORT:-40323}"
WORK="${RUNNER_TEMP:-/tmp}/kaduox-ssh-probe-integration"
KEY="$WORK/client_ed25519"
HOST_KEY="$WORK/ssh_host_ed25519_key"
KNOWN_HOSTS="$WORK/known_hosts"
PROBE_HOME="$WORK/home"
SSH_DIR="/home/$TEST_USER/.ssh"
PID_FILE="$WORK/sshd.pid"
LOG="$WORK/sshd.log"
CONFIG="$WORK/sshd.conf"

mkdir -p "$WORK" "$PROBE_HOME/.ssh"
chmod 700 "$WORK" "$PROBE_HOME/.ssh"

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
  echo "probe integration failure: $*" >&2
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

cat >"$PROBE_HOME/.ssh/config" <<EOF
Host probe-target
  HostName 127.0.0.1
  Port $PORT
  User $TEST_USER
  IdentityFile $KEY
  IdentitiesOnly yes
  UserKnownHostsFile $KNOWN_HOSTS
EOF
chmod 600 "$PROBE_HOME/.ssh/config"

sudo mkdir -p /run/sshd
sudo /usr/sbin/sshd -t -f "$CONFIG"
sudo /usr/sbin/sshd -f "$CONFIG" -E "$LOG"
wait_for_port || fail "probe sshd did not start on port $PORT"

run_probe() {
  HOME="$PROBE_HOME" "$KSSH" probe-target --identity "$KEY" "$@" probe
}

assert_common_probe_output() {
  local output="$1"
  grep -q '^endpoint: 127.0.0.1:' <<<"$output" || fail "probe did not report resolved endpoint"
  grep -q "^user: $TEST_USER$" <<<"$output" || fail "probe did not report SSH user"
  grep -q '^route: direct$' <<<"$output" || fail "probe did not report direct route"
  grep -Eq '^connect-auth-ms: [0-9]+$' <<<"$output" || fail "probe did not report numeric connect/auth timing"
  grep -q '^host-key-algorithm: ssh-ed25519$' <<<"$output" || fail "probe did not report ed25519 host-key algorithm"
  grep -q '^host-key-fingerprint: SHA256:' <<<"$output" || fail "probe did not report SHA-256 host-key fingerprint"
}

echo '[integration] probe accept-new learns unknown key'
first="$(run_probe --host-key accept-new)"
assert_common_probe_output "$first"
grep -q '^host-key-policy: accept-new$' <<<"$first" || fail "probe policy output mismatch"
grep -q '^host-key-verification: accept-new-learned$' <<<"$first" || fail "first probe did not report learned key"
[[ -s "$KNOWN_HOSTS" ]] || fail "accept-new probe did not persist known_hosts entry"

echo '[integration] probe recognizes persisted key'
second="$(run_probe --host-key accept-new)"
assert_common_probe_output "$second"
grep -q '^host-key-verification: known-hosts$' <<<"$second" || fail "second probe did not recognize known key"

echo '[integration] probe labels insecure host-key mode'
insecure="$(run_probe --host-key insecure)"
assert_common_probe_output "$insecure"
grep -q '^host-key-verification: insecure-unverified$' <<<"$insecure" || fail "insecure probe verification label mismatch"

echo '[integration] probe rejects forwarding options'
if HOME="$PROBE_HOME" "$KSSH" probe-target --identity "$KEY" --host-key insecure -L 39091:127.0.0.1:22 probe >"$WORK/forward-reject.log" 2>&1; then
  fail 'probe unexpectedly accepted -L forwarding'
fi
grep -q 'probe does not start port forwards' "$WORK/forward-reject.log" || {
  cat "$WORK/forward-reject.log" >&2
  fail 'probe forwarding rejection did not explain the constraint'
}
