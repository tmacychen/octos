#!/usr/bin/env bash
# Deploy crew + app-skill binaries to the Cloud Mac Mini.
# Usage: ./scripts/deploy.sh
set -euo pipefail

REMOTE="cloud@69.194.3.128"
REMOTE_PW="zjsgf128"
SCP="sshpass -p $REMOTE_PW scp -o PubkeyAuthentication=no"
SSH="sshpass -p $REMOTE_PW ssh -o PubkeyAuthentication=no $REMOTE"
REMOTE_BIN="/Users/cloud/.cargo/bin"
PLIST="io.ominix.crew-serve"

BINARIES=(crew news_fetch deep-search deep_crawl send_email account_manager)

echo "==> Building release binaries..."
cargo build --release -p crew-cli --features telegram,whatsapp,feishu,twilio,api
cargo build --release -p news_fetch -p deep-search -p deep-crawl -p send-email -p account-manager

echo "==> Signing binaries locally..."
for bin in "${BINARIES[@]}"; do
    codesign -s - "target/release/$bin" 2>/dev/null || true
done

echo "==> Uploading binaries to remote..."
for bin in "${BINARIES[@]}"; do
    echo "    $bin"
    $SCP "target/release/$bin" "$REMOTE:/tmp/${bin}.new"
done

echo "==> Stopping launchd service..."
$SSH "launchctl unload ~/Library/LaunchAgents/${PLIST}.plist 2>/dev/null || true"
sleep 1
$SSH "pkill -f 'crew serve' 2>/dev/null || true; pkill -f 'crew gateway' 2>/dev/null || true"
sleep 1

echo "==> Replacing binaries on remote..."
for bin in "${BINARIES[@]}"; do
    $SSH "mv /tmp/${bin}.new ${REMOTE_BIN}/${bin} && codesign --force -s - ${REMOTE_BIN}/${bin}"
done

echo "==> Cleaning stale skill dirs (bootstrap recreates them)..."
for skill in news deep-search deep-crawl send-email account-manager; do
    $SSH "rm -rf /Users/cloud/.crew/skills/${skill}" 2>/dev/null || true
done

echo "==> Starting launchd service..."
$SSH "launchctl load ~/Library/LaunchAgents/${PLIST}.plist"

echo "==> Done! Verifying..."
sleep 2
$SSH "launchctl list | grep crew || echo 'WARNING: service not found'"
echo "Deploy complete."
