#!/usr/bin/env bash
set -euo pipefail

# Configure Docker DNS for building (resolves npmjs.org during build)
# Run after Docker Desktop starts

DOCKER_DIR="$HOME/.docker"
DAEMON_JSON="$DOCKER_DIR/daemon.json"

mkdir -p "$DOCKER_DIR"

# Read existing config if present
if [[ -f "$DAEMON_JSON" ]]; then
    existing_dns=$(python3 -c "import json; print(json.load(open('$DAEMON_JSON')).get('dns', ''))" 2>/dev/null || echo "")
else
    existing_dns=""
fi

if [[ "$existing_dns" == *"8.8.8.8"* ]]; then
    echo "Docker DNS already configured"
    exit 0
fi

cat > "$DAEMON_JSON" << 'EOF'
{
  "dns": ["8.8.8.8", "1.1.1.1"]
}
EOF

echo "Docker DNS configured in $DAEMON_JSON"
echo "Restart Docker Desktop for changes to take effect"
