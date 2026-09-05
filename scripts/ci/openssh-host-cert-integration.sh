#!/usr/bin/env bash
set -euo pipefail

KSSH="${KSSH:-target/debug/kssh}"
TEST_USER="${KADUOX_CERT_TEST_USER:-kaduox-cert-ci}"
PORT="${KADUOX_CERT_PORT:-40230}"
WORK="${RUNNER_TEMP:-/tmp}/kaduox-ssh-host-cert"
CLIENT_KEY="$WORK/client_ed25519"
HOST_KEY="$WORK/ssh_host_ed25519_key"
CA_KEY="$WORK/host_ca_ed25519"
CONFIG="$WORK/sshd.conf"
PID_FILE="$WORK/sshd.pid"
LOG="$WORK/sshd.log"
LOCAL_HOME="$WORK/home"

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
  echo "host certificate integration failure: $*" >&2
  if [[ -f "$LOG" ]]; then
    echo "--- $LOG ---" >&2
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

hash_known_hosts_file() {
  local path="$1"
  ssh-keygen -q -H -f "$path"
  rm -f "$path.old"
}

if [[ ! -x "$KSSH" ]]; then
  fail "kssh binary not found at $KSSH"
fi

rm -rf "$WORK"
mkdir -p "$WORK" "$LOCAL_HOME/.ssh"
chmod 700 "$WORK" "$LOCAL_HOME/.ssh"

if id "$TEST_USER" >/dev/null 2>&1; then
  sudo userdel -r "$TEST_USER" >/dev/null 2>&1 || true
fi
sudo useradd -m -s /bin/bash "$TEST_USER"
sudo passwd -d "$TEST_USER" >/dev/null

ssh-keygen -q -t ed25519 -N '' -f "$CLIENT_KEY"
ssh-keygen -q -t ed25519 -N '' -f "$HOST_KEY"
ssh-keygen -q -t ed25519 -N '' -f "$CA_KEY"
ssh-keygen -q -s "$CA_KEY" -I kaduox-host-cert -h -n 127.0.0.1 -V -5m:+1h "$HOST_KEY.pub"

REMOTE_SSH_DIR="/home/$TEST_USER/.ssh"
sudo install -d -m 0700 -o "$TEST_USER" -g "$TEST_USER" "$REMOTE_SSH_DIR"
sudo install -m 0600 -o "$TEST_USER" -g "$TEST_USER" "$CLIENT_KEY.pub" "$REMOTE_SSH_DIR/authorized_keys"

cat >"$CONFIG" <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey $HOST_KEY
HostCertificate $HOST_KEY-cert.pub
PidFile $PID_FILE
AuthorizedKeysFile .ssh/authorized_keys
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PermitRootLogin no
UsePAM no
AllowUsers $TEST_USER
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
Subsystem sftp internal-sftp
LogLevel VERBOSE
EOF

sudo mkdir -p /run/sshd
sudo /usr/sbin/sshd -t -f "$CONFIG"
sudo /usr/sbin/sshd -f "$CONFIG" -E "$LOG"
wait_for_port || fail "sshd did not start on port $PORT"

CA_PUBLIC="$(cat "$CA_KEY.pub")"
HOST_PUBLIC="$(cat "$HOST_KEY.pub")"
printf '@cert-authority [127.0.0.1]:%s %s\n' "$PORT" "$CA_PUBLIC" >"$WORK/known-good"
printf '@cert-authority [localhost]:%s %s\n' "$PORT" "$CA_PUBLIC" >"$WORK/known-bad-principal"
{
  printf '@cert-authority [127.0.0.1]:%s %s\n' "$PORT" "$CA_PUBLIC"
  printf '@revoked [127.0.0.1]:%s %s\n' "$PORT" "$CA_PUBLIC"
} >"$WORK/known-revoked-ca"
printf '@revoked [127.0.0.1]:%s %s\n' "$PORT" "$HOST_PUBLIC" >"$WORK/known-revoked-host"

cp "$WORK/known-good" "$WORK/known-hashed-good"
cp "$WORK/known-revoked-ca" "$WORK/known-hashed-revoked-ca"
cp "$WORK/known-revoked-host" "$WORK/known-hashed-revoked-host"
hash_known_hosts_file "$WORK/known-hashed-good"
hash_known_hosts_file "$WORK/known-hashed-revoked-ca"
hash_known_hosts_file "$WORK/known-hashed-revoked-host"

grep -q '^@cert-authority |1|' "$WORK/known-hashed-good" || fail 'ssh-keygen -H did not hash @cert-authority fixture'
grep -q '^@revoked |1|' "$WORK/known-hashed-revoked-ca" || fail 'ssh-keygen -H did not hash @revoked CA fixture'
grep -q '^@revoked |1|' "$WORK/known-hashed-revoked-host" || fail 'ssh-keygen -H did not hash @revoked host fixture'
chmod 600 "$WORK"/known-*

cat >"$LOCAL_HOME/.ssh/config" <<EOF
Host cert-good
  HostName 127.0.0.1
  Port $PORT
  User $TEST_USER
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  UserKnownHostsFile $WORK/known-good
  StrictHostKeyChecking yes

Host cert-hashed-good
  HostName 127.0.0.1
  Port $PORT
  User $TEST_USER
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  UserKnownHostsFile $WORK/known-hashed-good
  StrictHostKeyChecking yes

Host cert-bad-principal
  HostName localhost
  Port $PORT
  User $TEST_USER
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  UserKnownHostsFile $WORK/known-bad-principal
  StrictHostKeyChecking yes

Host cert-revoked-ca
  HostName 127.0.0.1
  Port $PORT
  User $TEST_USER
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  UserKnownHostsFile $WORK/known-revoked-ca
  StrictHostKeyChecking yes

Host cert-hashed-revoked-ca
  HostName 127.0.0.1
  Port $PORT
  User $TEST_USER
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  UserKnownHostsFile $WORK/known-hashed-revoked-ca
  StrictHostKeyChecking yes

Host plain-revoked
  HostName 127.0.0.1
  Port $PORT
  User $TEST_USER
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  UserKnownHostsFile $WORK/known-revoked-host
  StrictHostKeyChecking no

Host plain-hashed-revoked
  HostName 127.0.0.1
  Port $PORT
  User $TEST_USER
  IdentityFile $CLIENT_KEY
  IdentitiesOnly yes
  UserKnownHostsFile $WORK/known-hashed-revoked-host
  StrictHostKeyChecking no
EOF
chmod 600 "$LOCAL_HOME/.ssh/config"

run_kssh() {
  HOME="$LOCAL_HOME" "$KSSH" "$@"
}

echo '[host-cert] trusted CA certificate succeeds'
probe_output="$(run_kssh cert-good probe)" || fail 'trusted host certificate was rejected'
grep -q '^host-key-verification: known-hosts$' <<<"$probe_output" || {
  echo "$probe_output" >&2
  fail 'trusted host certificate was not reported as known trust'
}

echo '[host-cert] hashed trusted CA certificate succeeds'
hashed_probe_output="$(run_kssh cert-hashed-good probe)" || fail 'hashed trusted host certificate was rejected'
grep -q '^host-key-verification: known-hosts$' <<<"$hashed_probe_output" || {
  echo "$hashed_probe_output" >&2
  fail 'hashed trusted host certificate was not reported as known trust'
}

echo '[host-cert] principal mismatch fails closed'
if run_kssh cert-bad-principal probe >"$WORK/bad-principal.log" 2>&1; then
  fail 'certificate with wrong hostname principal unexpectedly succeeded'
fi
grep -qi 'does not authorize hostname' "$WORK/bad-principal.log" || {
  cat "$WORK/bad-principal.log" >&2
  fail 'principal mismatch did not fail at certificate hostname validation'
}

echo '[host-cert] revoked signing CA fails closed'
if run_kssh cert-revoked-ca probe >"$WORK/revoked-ca.log" 2>&1; then
  fail 'certificate signed by an @revoked CA unexpectedly succeeded'
fi
grep -qi 'signing CA is marked @revoked' "$WORK/revoked-ca.log" || {
  cat "$WORK/revoked-ca.log" >&2
  fail 'revoked CA was not rejected by certificate trust policy'
}

echo '[host-cert] hashed revoked signing CA fails closed'
if run_kssh cert-hashed-revoked-ca probe >"$WORK/hashed-revoked-ca.log" 2>&1; then
  fail 'certificate signed by a hashed @revoked CA unexpectedly succeeded'
fi
grep -qi 'signing CA is marked @revoked' "$WORK/hashed-revoked-ca.log" || {
  cat "$WORK/hashed-revoked-ca.log" >&2
  fail 'hashed revoked CA was not rejected by certificate trust policy'
}

echo '[host-cert] revoked plain host key overrides insecure policy'
if run_kssh plain-revoked --host-key insecure probe >"$WORK/revoked-host.log" 2>&1; then
  fail '@revoked plain host key unexpectedly succeeded under insecure policy'
fi
grep -qi 'marked @revoked' "$WORK/revoked-host.log" || {
  cat "$WORK/revoked-host.log" >&2
  fail 'plain @revoked host key was not rejected before insecure acceptance'
}

echo '[host-cert] hashed revoked plain host key overrides insecure policy'
if run_kssh plain-hashed-revoked --host-key insecure probe >"$WORK/hashed-revoked-host.log" 2>&1; then
  fail 'hashed @revoked plain host key unexpectedly succeeded under insecure policy'
fi
grep -qi 'marked @revoked' "$WORK/hashed-revoked-host.log" || {
  cat "$WORK/hashed-revoked-host.log" >&2
  fail 'hashed plain @revoked host key was not rejected before insecure acceptance'
}

echo '[host-cert] all host certificate trust checks passed'
