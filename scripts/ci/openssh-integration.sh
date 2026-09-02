#!/usr/bin/env bash
set -euo pipefail

KSSH="${KSSH:-target/debug/kssh}"
TEST_USER="${KADUOX_TEST_USER:-kaduox-ci}"
JUMP_PORT="${KADUOX_JUMP_PORT:-40222}"
TARGET_PORT="${KADUOX_TARGET_PORT:-40223}"
RSA_HOST_PORT="${KADUOX_RSA_HOST_PORT:-40224}"
HTTP_PORT="${KADUOX_HTTP_PORT:-39080}"
LOCAL_FORWARD_PORT="${KADUOX_LOCAL_FORWARD_PORT:-39081}"
SOCKS_PORT="${KADUOX_SOCKS_PORT:-39082}"
REMOTE_FORWARD_PORT="${KADUOX_REMOTE_FORWARD_PORT:-39083}"
WORK="${RUNNER_TEMP:-/tmp}/kaduox-ssh-integration"
KEY="$WORK/client_ed25519"
RSA_KEY="$WORK/client_rsa"
SSH_DIR="/home/$TEST_USER/.ssh"

mkdir -p "$WORK"
chmod 700 "$WORK"

PIDS=()
cleanup() {
  set +e
  for pid in "${PIDS[@]:-}"; do
    sudo kill "$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  if [[ -n "${SSH_AGENT_PID:-}" ]]; then
    ssh-agent -k >/dev/null 2>&1 || true
  fi
  sudo rm -f /etc/sudoers.d/kaduox-ci /etc/kaduox-ci-integration.conf
  sudo userdel -r "$TEST_USER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

fail() {
  echo "integration failure: $*" >&2
  for log in "$WORK"/sshd-*.log; do
    if [[ -f "$log" ]]; then
      echo "--- $log ---" >&2
      sudo tail -n 120 "$log" >&2 || true
    fi
  done
  exit 1
}

wait_for_port() {
  local host="$1"
  local port="$2"
  for _ in $(seq 1 100); do
    if (echo >/dev/tcp/"$host"/"$port") >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

stop_pid() {
  local pid="$1"
  sudo kill "$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

start_sshd() {
  local name="$1"
  local port="$2"
  local host_key_type="${3:-ed25519}"
  local config="$WORK/sshd-$name.conf"
  local pid_file="$WORK/sshd-$name.pid"
  local host_key="$WORK/ssh_host_${name}_${host_key_type}_key"
  local log="$WORK/sshd-$name.log"

  if [[ "$host_key_type" == 'rsa' ]]; then
    ssh-keygen -q -t rsa -b 3072 -N '' -f "$host_key"
  else
    ssh-keygen -q -t ed25519 -N '' -f "$host_key"
  fi
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
AllowAgentForwarding yes
GatewayPorts no
X11Forwarding no
PermitTunnel no
Subsystem sftp internal-sftp
LogLevel VERBOSE
EOF

  sudo mkdir -p /run/sshd
  sudo /usr/sbin/sshd -t -f "$config"
  sudo /usr/sbin/sshd -f "$config" -E "$log"
  wait_for_port 127.0.0.1 "$port" || {
    cat "$log" >&2 || true
    fail "sshd $name did not start on port $port"
  }
  local pid
  pid="$(cat "$pid_file")"
  PIDS+=("$pid")
  echo "$pid"
}

run_kssh() {
  "$KSSH" 127.0.0.1 \
    --port "$TARGET_PORT" \
    --user "$TEST_USER" \
    --identity "$KEY" \
    --host-key insecure \
    "$@"
}

if [[ ! -x "$KSSH" ]]; then
  fail "kssh binary not found at $KSSH"
fi

if id "$TEST_USER" >/dev/null 2>&1; then
  sudo userdel -r "$TEST_USER" >/dev/null 2>&1 || true
fi
sudo useradd -m -s /bin/bash "$TEST_USER"
# Ubuntu creates passwordless test accounts in a locked state by default. OpenSSH
# rejects locked accounts before public-key authentication, so unlock the fixture
# account while keeping PasswordAuthentication disabled in all test daemons.
sudo passwd -d "$TEST_USER" >/dev/null
ssh-keygen -q -t ed25519 -N '' -f "$KEY"
ssh-keygen -q -t rsa -b 3072 -N '' -f "$RSA_KEY"
sudo install -d -m 0700 -o "$TEST_USER" -g "$TEST_USER" "$SSH_DIR"
sudo install -m 0600 -o "$TEST_USER" -g "$TEST_USER" "$KEY.pub" "$SSH_DIR/authorized_keys"
sudo tee -a "$SSH_DIR/authorized_keys" <"$RSA_KEY.pub" >/dev/null
sudo chown "$TEST_USER:$TEST_USER" "$SSH_DIR/authorized_keys"
sudo chmod 0600 "$SSH_DIR/authorized_keys"
echo "$TEST_USER ALL=(ALL) NOPASSWD:ALL" | sudo tee /etc/sudoers.d/kaduox-ci >/dev/null
sudo chmod 0440 /etc/sudoers.d/kaduox-ci

mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
cat >"$HOME/.ssh/config" <<EOF
Host 127.0.0.1
  User $TEST_USER
  IdentityFile $KEY
  IdentitiesOnly yes
EOF
chmod 600 "$HOME/.ssh/config"

start_sshd jump "$JUMP_PORT" >/dev/null
start_sshd target "$TARGET_PORT" >/dev/null
start_sshd rsa-host "$RSA_HOST_PORT" rsa >/dev/null

echo '[integration] fixture OpenSSH client baseline'
fixture_output="$(ssh -F /dev/null -i "$KEY" -p "$TARGET_PORT" \
  -o BatchMode=yes \
  -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  "$TEST_USER@127.0.0.1" printf fixture-ok)" || fail 'native OpenSSH client could not authenticate to target fixture'
[[ "$fixture_output" == 'fixture-ok' ]] || fail "unexpected fixture output: $fixture_output"

echo '[integration] direct exec'
exec_output="$(run_kssh exec -- printf integration-ok)"
[[ "$exec_output" == 'integration-ok' ]] || fail "unexpected exec output: $exec_output"

echo '[integration] RSA server host-key verification path'
rsa_host_output="$("$KSSH" 127.0.0.1 --port "$RSA_HOST_PORT" --user "$TEST_USER" --identity "$KEY" --host-key insecure exec -- printf rsa-host-ok)"
[[ "$rsa_host_output" == 'rsa-host-ok' ]] || fail "RSA host-key server returned: $rsa_host_output"

echo '[integration] sudo privilege switch'
sudo_output="$(run_kssh exec --as-user root -- id -u | tr -d '\r\n')"
[[ "$sudo_output" == '0' ]] || fail "sudo user switch returned: $sudo_output"

echo '[integration] ProxyCommand transport'
proxy_output="$("$KSSH" 127.0.0.1 --port "$TARGET_PORT" --user "$TEST_USER" --identity "$KEY" --host-key insecure --proxy-command 'nc %h %p' exec -- printf proxy-ok)"
[[ "$proxy_output" == 'proxy-ok' ]] || fail "ProxyCommand returned: $proxy_output"

echo '[integration] ProxyJump transport'
jump_output="$("$KSSH" 127.0.0.1 --port "$TARGET_PORT" --user "$TEST_USER" --identity "$KEY" --host-key insecure -J "$TEST_USER@127.0.0.1:$JUMP_PORT" exec -- printf jump-ok)"
[[ "$jump_output" == 'jump-ok' ]] || fail "ProxyJump returned: $jump_output"

echo '[integration] SSH agent authentication'
eval "$(ssh-agent -s)" >/dev/null
ssh-add "$KEY" >/dev/null
agent_output="$("$KSSH" 127.0.0.1 --port "$TARGET_PORT" --user "$TEST_USER" --host-key insecure exec -- printf agent-ok)"
[[ "$agent_output" == 'agent-ok' ]] || fail "agent authentication returned: $agent_output"

echo '[integration] direct RSA private-key signing is blocked'
if "$KSSH" 127.0.0.1 --port "$TARGET_PORT" --user "$TEST_USER" --identity "$RSA_KEY" --host-key insecure exec -- true >"$WORK/rsa-direct.log" 2>&1; then
  fail 'direct RSA private-key authentication unexpectedly succeeded'
fi
grep -q 'direct RSA private-key authentication is disabled' "$WORK/rsa-direct.log" || {
  cat "$WORK/rsa-direct.log" >&2
  fail 'direct RSA private-key rejection did not explain the security policy'
}

echo '[integration] RSA authentication through external SSH agent'
ssh-add -D >/dev/null
ssh-add "$RSA_KEY" >/dev/null
rsa_agent_output="$("$KSSH" 127.0.0.1 --port "$TARGET_PORT" --user "$TEST_USER" --host-key insecure exec -- printf rsa-agent-ok)"
[[ "$rsa_agent_output" == 'rsa-agent-ok' ]] || fail "RSA agent authentication returned: $rsa_agent_output"
ssh-add -D >/dev/null
ssh-add "$KEY" >/dev/null

echo '[integration] agent forwarding'
forwarded_output="$("$KSSH" 127.0.0.1 --port "$TARGET_PORT" --user "$TEST_USER" --host-key insecure -A exec -- sh -lc "ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -p $JUMP_PORT $TEST_USER@127.0.0.1 printf forwarded-agent")"
[[ "$forwarded_output" == 'forwarded-agent' ]] || fail "agent forwarding returned: $forwarded_output"

echo '[integration] SFTP upload/download'
printf 'sftp-roundtrip\n' >"$WORK/upload.txt"
run_kssh upload "$WORK/upload.txt" "/home/$TEST_USER/upload.txt"
run_kssh download "/home/$TEST_USER/upload.txt" "$WORK/download.txt"
cmp "$WORK/upload.txt" "$WORK/download.txt"

echo '[integration] privileged staged upload'
printf 'privileged-upload\n' >"$WORK/privileged.conf"
run_kssh upload "$WORK/privileged.conf" /etc/kaduox-ci-integration.conf --as-user root --mode 0640
sudo cmp "$WORK/privileged.conf" /etc/kaduox-ci-integration.conf
mode="$(stat -c '%a' /etc/kaduox-ci-integration.conf)"
[[ "$mode" == '640' ]] || fail "privileged upload mode is $mode"

echo '[integration] sync dry-run, apply, preserve, delete'
mkdir -p "$WORK/sync-local/nested"
printf 'alpha\n' >"$WORK/sync-local/a.txt"
printf 'beta\n' >"$WORK/sync-local/nested/b.txt"
dry_run_output="$(run_kssh sync "$WORK/sync-local" "/home/$TEST_USER/sync" --dry-run 2>&1)"
grep -q 'upload' <<<"$dry_run_output" || fail 'sync dry-run did not report uploads'
run_kssh exec -- test ! -e "/home/$TEST_USER/sync" || fail 'dry-run mutated remote tree'
run_kssh sync "$WORK/sync-local" "/home/$TEST_USER/sync"
run_kssh exec -- grep -qx alpha "/home/$TEST_USER/sync/a.txt"
run_kssh exec -- grep -qx beta "/home/$TEST_USER/sync/nested/b.txt"
run_kssh exec -- sh -lc "printf stale\\n > /home/$TEST_USER/sync/stale.txt"
run_kssh sync "$WORK/sync-local" "/home/$TEST_USER/sync" --size-only
run_kssh exec -- test -e "/home/$TEST_USER/sync/stale.txt" || fail 'non-delete sync removed remote-only file'
run_kssh sync "$WORK/sync-local" "/home/$TEST_USER/sync" --size-only --delete
run_kssh exec -- test ! -e "/home/$TEST_USER/sync/stale.txt" || fail '--delete did not remove remote-only file'

echo '[integration] TCP forwarding'
mkdir -p "$WORK/http"
printf 'forwarding-ok\n' >"$WORK/http/payload.txt"
python3 -m http.server "$HTTP_PORT" --bind 127.0.0.1 --directory "$WORK/http" >"$WORK/http.log" 2>&1 &
http_pid=$!
PIDS+=("$http_pid")
wait_for_port 127.0.0.1 "$HTTP_PORT" || fail 'HTTP fixture did not start'

run_kssh -L "127.0.0.1:$LOCAL_FORWARD_PORT:127.0.0.1:$HTTP_PORT" tunnel >"$WORK/local-forward.log" 2>&1 &
local_pid=$!
PIDS+=("$local_pid")
wait_for_port 127.0.0.1 "$LOCAL_FORWARD_PORT" || fail 'local forward did not listen'
[[ "$(curl -fsS "http://127.0.0.1:$LOCAL_FORWARD_PORT/payload.txt")" == 'forwarding-ok' ]] || fail 'local forward payload mismatch'
stop_pid "$local_pid"

run_kssh -D "127.0.0.1:$SOCKS_PORT" tunnel >"$WORK/socks-forward.log" 2>&1 &
socks_pid=$!
PIDS+=("$socks_pid")
wait_for_port 127.0.0.1 "$SOCKS_PORT" || fail 'SOCKS5 forward did not listen'
[[ "$(curl -fsS --socks5-hostname "127.0.0.1:$SOCKS_PORT" "http://127.0.0.1:$HTTP_PORT/payload.txt")" == 'forwarding-ok' ]] || fail 'SOCKS5 payload mismatch'
stop_pid "$socks_pid"

run_kssh -R "127.0.0.1:$REMOTE_FORWARD_PORT:127.0.0.1:$HTTP_PORT" tunnel >"$WORK/remote-forward.log" 2>&1 &
remote_pid=$!
PIDS+=("$remote_pid")
wait_for_port 127.0.0.1 "$REMOTE_FORWARD_PORT" || fail 'remote forward did not listen'
[[ "$(curl -fsS "http://127.0.0.1:$REMOTE_FORWARD_PORT/payload.txt")" == 'forwarding-ok' ]] || fail 'remote forward payload mismatch'
stop_pid "$remote_pid"

echo '[integration] all OpenSSH integration checks passed'
