#!/usr/bin/env bash
set -euo pipefail

SWAPFILE=/mnt/swapfile
SIZE=8G

# fallocate fails with "Text file busy" on a swap file that is still active,
# so release and remove any existing one before creating it.
sudo swapoff "$SWAPFILE" 2>/dev/null || true
sudo rm -f "$SWAPFILE"

sudo fallocate -l "$SIZE" "$SWAPFILE"
sudo chmod 600 "$SWAPFILE"
sudo mkswap "$SWAPFILE"
sudo swapon "$SWAPFILE"

swapon --show
free -h
