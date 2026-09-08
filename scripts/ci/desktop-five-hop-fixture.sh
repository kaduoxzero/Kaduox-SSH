#!/usr/bin/env bash
set -euo pipefail
# Isolated local fixture; does not edit the system sshd config or any user's keys.
[[ "$(id -u)" == 0 ]] || { echo 'Run as root inside WSL'; exit 1; }
BASE="${1:?pass a temporary fixture parent directory}"
WORK="$(mktemp -d "$BASE/kdx-five-XXXXXX")"
PIDS=()
cleanup() { for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done; }
trap cleanup EXIT
trap 'exit 130' INT TERM
ssh-keygen -q -t ed25519 -N '' -f "$WORK/client"
cp "$WORK/client.pub" "$WORK/authorized_keys"
mkdir -p /run/sshd
for n in 1 2 3 4 5 6; do
  port=$((41250 + n))
  if ss -ltn | awk '{print $4}' | grep -q ":$port$"; then echo "Port $port already occupied"; exit 1; fi
  ssh-keygen -q -t ed25519 -N '' -f "$WORK/host-$n"
  # Fixture configuration is generated in a fresh directory, never /etc/ssh.
  printf '%s\n' "Port $port" "ListenAddress 0.0.0.0" "HostKey $WORK/host-$n" \
    "PidFile $WORK/pid-$n" "AuthorizedKeysFile $WORK/authorized_keys" \
    "StrictModes no" "PubkeyAuthentication yes" "PasswordAuthentication no" \
    "KbdInteractiveAuthentication no" "PermitRootLogin prohibit-password" \
    "UsePAM yes" "AllowUsers root" "AllowTcpForwarding yes" \
    "Subsystem sftp internal-sftp" > "$WORK/sshd-$n.conf"
  /usr/sbin/sshd -t -f "$WORK/sshd-$n.conf"
  /usr/sbin/sshd -f "$WORK/sshd-$n.conf" -E "$WORK/log-$n"
  for attempt in {1..50}; do
    [[ -s "$WORK/pid-$n" ]] && break
    sleep 0.1
  done
  PIDS+=("$(< "$WORK/pid-$n")")
done
echo "FIXTURE_READY=$WORK"
echo "FIXTURE_HOST=$(hostname -I | awk '{print $1}')"
sleep 240
