#! /usr/bin/env bash
set -eou pipefail

host="root@192.168.64.204"
opts="-i keys/arch-boot -o Port=11838 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ControlPath=".tmp/cm-%r@%h:%p""

mkdir -p .tmp

# Start (or ensure) a master connection
ssh $opts -o ControlMaster=auto -o ControlPersist=10m -Nf $host

trap 'ssh $opts -O exit $host 2>/dev/null' EXIT

# Reuse it for commands (no re-auth)
ssh $opts "$host" 'uname -a'
ssh $opts "$host" 'id'

ssh $opts $host "bash -s" < ./setup.sh

# ssh $opts -O exit $host
