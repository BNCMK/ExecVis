#!/usr/bin/env bash
# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: install.sh
#  script_path: execviz-rs/packaging/install.sh
#  module_name: install
#  version: 0.53.1
#  description: !/usr/bin/env bash Installs the binary, the page, and a service account.
#  kind: script
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: install
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# Installs the binary, the page, and a service account.
#
# Deliberately small and readable: an install script for a tool that watches
# other programs should be something an operator can read in full before running.
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
STATE="${STATE:-/var/lib/execviz}"

[ "$(id -u)" -eq 0 ] || { echo "run as root, or set PREFIX to somewhere writable"; exit 1; }

# Ask the machine whether it can run this, before installing anything.
#
# An install that succeeds and then does not work leaves an operator with a
# mystery instead of a message. The binary answers this about itself, so the
# check and the thing being installed can never disagree.
if [ -x target/release/execviz ]; then
  echo "checking whether this machine can run the recorder..."
  if target/release/execviz doctor > /tmp/execviz-doctor.json 2>/dev/null; then
    echo "  it can. Installing the collector, the map and the recorder."
    INSTALL_FLOOR=1
  else
    echo
    echo "  It cannot, and here is why:"
    python3 - <<'PY' 2>/dev/null || cat /tmp/execviz-doctor.json
import json
d = json.load(open("/tmp/execviz-doctor.json"))
for c in d["checks"]:
    if not c["ok"]:
        print(f"    {c['check']}: found {c['found']}")
        print(f"      {c['fix']}")
PY
    echo
    echo "  The collector, the map and the adapters do NOT need any of that and"
    echo "  will be installed anyway. Only the recorder is being left out."
    INSTALL_FLOOR=0
  fi
fi

command -v ssh-keygen >/dev/null || \
  echo "note: ssh-keygen is absent, so SSH-key sign-in will not work (password and API keys still will)"

install -Dm755 target/release/execviz "$PREFIX/bin/execviz"
install -Dm644 ui.html "$PREFIX/share/execviz/ui.html"
install -Dm644 README.md "$PREFIX/share/doc/execviz/README.md"

id execviz >/dev/null 2>&1 || useradd --system --create-home --home-dir "$STATE" execviz
install -d -o execviz -g execviz -m 0750 "$STATE"

if [ -d /etc/systemd/system ]; then
  install -Dm644 packaging/execviz.service /etc/systemd/system/execviz.service
  echo "systemd unit installed; enable with: systemctl enable --now execviz"
fi

echo
echo "installed $("$PREFIX/bin/execviz" 2>&1 | head -1)"
echo
echo "next, create an account; until one exists, access is open and the server reports it:"
echo "  execviz account $STATE/capture.db create \$USER --password '...'"
echo
echo "and put it behind TLS or an SSH tunnel: over plain HTTP the password and"
echo "the capture both cross the network in the clear."
