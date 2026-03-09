#!/bin/sh
set -eu

LOG=/var/log/bento-guest-install.log
exec >>"$LOG" 2>&1

echo "[bento] guest install start $(date -Iseconds)"

MNT=/run/bento-cidata
SRC_LOWER="$MNT/bento-guestd"
SRC_UPPER="$MNT/BENTO-GUESTD"
CONFIG_SRC="$MNT/bento-guestd.yaml"
CONFIG_ENV_SRC="$MNT/config.env"
TASKS_DIR="$MNT/tasks"
RUN_TASKS_DIR=/run/bento-tasks
SRC=""

if [ -f "$SRC_LOWER" ]; then
  SRC="$SRC_LOWER"
elif [ -f "$SRC_UPPER" ]; then
  SRC="$SRC_UPPER"
else
  echo "[bento] payload not found in CIDATA mount"
  exit 1
fi

if [ ! -f "$CONFIG_SRC" ]; then
  echo "[bento] guestd config payload not found in CIDATA mount"
  exit 1
fi

if [ ! -f "$CONFIG_ENV_SRC" ]; then
  echo "[bento] config.env payload not found in CIDATA mount"
  exit 1
fi

if [ ! -d "$TASKS_DIR" ]; then
  echo "[bento] tasks directory not found in CIDATA mount"
  exit 1
fi

DST=/usr/local/bin/bento-guestd

mkdir -p "$(dirname "$DST")"
mkdir -p "$RUN_TASKS_DIR"

install -m 0755 "$SRC" "$DST"
BENTO_GUESTD_BINARY_CHANGED=1
echo "[bento] installed guest binary to $DST"

export BENTO_CIDATA_MNT="$MNT"
export BENTO_GUESTD_BINARY_CHANGED
export BENTO_TASKS_DIR="$RUN_TASKS_DIR"

set -a
. "$CONFIG_ENV_SRC"
set +a

rm -f "$RUN_TASKS_DIR"/[0-9][0-9]-*.sh 2>/dev/null || true

for source_task in "$TASKS_DIR"/[0-9][0-9]-*.sh; do
  if [ ! -f "$source_task" ]; then
    continue
  fi

  task_name=$(basename "$source_task")
  runtime_task="$RUN_TASKS_DIR/$task_name"
  install -m 0755 "$source_task" "$runtime_task"
done

for task in "$RUN_TASKS_DIR"/[0-9][0-9]-*.sh; do
  if [ ! -f "$task" ]; then
    continue
  fi

  echo "[bento] running task $(basename "$task")"
  "$task"
done

echo "[bento] guest install done $(date -Iseconds)"
