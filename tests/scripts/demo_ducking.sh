#!/bin/bash

# Audio Ducking Demo Script
# This script demonstrates the audio ducking feature with different scenarios

set -e

AUDIO_DIR="../audio"
CONFIG="./ducking_demo_config.json"
TOPIC="audio/demo"

echo "=========================================="
echo "Audio Ducking Feature Demo"
echo "=========================================="
echo ""
echo "This demo uses different frequency tones to represent different voice types:"
echo "  - Music (200Hz): Low continuous drone"
echo "  - Narration (800Hz): Mid-range tone"
echo "  - Dialog (1200Hz): Higher tone"
echo "  - Effects (600Hz): Mid-low tone"
echo ""
echo "Ducking Rules (from config):"
echo "  1. Narration ducks Music+Effects to 15% over 2 seconds"
echo "  2. Dialog ducks Music+Effects+Narration to 5% over 1 second"
echo ""
echo "Press Ctrl+C to stop at any time"
echo ""
read -p "Press Enter to start the demo..."
echo ""

# Kill any existing instances
pkill -9 -f "mqttaudio" 2>/dev/null || true
sleep 1

# Start mqttaudio with ducking config
echo "Starting mqttaudio with ducking configuration..."
cargo run --release -- --server localhost --topic "$TOPIC" --config "$CONFIG" &
DAEMON_PID=$!
sleep 2

echo ""
echo "=========================================="
echo "SCENARIO 1: Simple Ducking"
echo "=========================================="
echo "Listen for: Music volume decreasing when narration starts"
echo ""

echo "[1/4] Playing continuous music (200Hz) at 70% volume..."
mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/music_200hz.wav",
    "voice": "music",
    "volume": 0.7,
    "loop": true
  }
}'
sleep 3

echo "[2/4] Starting narration (800Hz) - music should duck to 15% over 2 seconds..."
mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/narration_800hz.wav",
    "voice": "narration",
    "volume": 0.8
  }
}'
sleep 4

echo "[3/4] Narration ended - music should restore to 70% over 2 seconds..."
sleep 3

echo "[4/4] Playing another narration to verify ducking works consistently..."
mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/narration_800hz.wav",
    "voice": "narration",
    "volume": 0.8
  }
}'
sleep 5

echo ""
echo "=========================================="
echo "SCENARIO 2: Multiple Rules (Deeper Duck)"
echo "=========================================="
echo "Listen for: Music ducks further when dialog starts during narration"
echo ""

echo "[1/4] Music still playing (200Hz)..."
sleep 1

echo "[2/4] Starting narration (800Hz) - music ducks to 15%..."
mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/narration_800hz.wav",
    "voice": "narration",
    "volume": 0.8
  }
}'
sleep 1.5

echo "[3/4] Starting dialog (1200Hz) mid-narration - music should duck further to 5% over 1 second..."
echo "      (This is the critical test for smooth mid-fade transitions!)"
mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/dialog_1200hz.wav",
    "voice": "dialog",
    "volume": 0.9
  }
}'
sleep 3

echo "[4/4] Both narration and dialog ended - music restoring to 70%..."
sleep 3

echo ""
echo "=========================================="
echo "SCENARIO 3: Effects + Ducking"
echo "=========================================="
echo "Listen for: Both music and effects ducking together"
echo ""

echo "[1/5] Music still playing..."
sleep 1

echo "[2/5] Adding effects (600Hz) at 60% volume..."
mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/effects_600hz.wav",
    "voice": "effects",
    "volume": 0.6,
    "loop": true
  }
}'
sleep 2

echo "[3/5] Starting narration - both music and effects should duck to 15%..."
mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/narration_800hz.wav",
    "voice": "narration",
    "volume": 0.8
  }
}'
sleep 4

echo "[4/5] Narration ended - music and effects restoring..."
sleep 2

echo "[5/5] Effects will continue (no stop command yet)..."
sleep 1

echo ""
echo "=========================================="
echo "SCENARIO 4: Rapid Transitions"
echo "=========================================="
echo "Listen for: Smooth fading with quick voice changes"
echo ""

echo "[1/4] Music playing..."
sleep 1

echo "[2/4] Quick succession: Narration -> Dialog -> Narration..."
mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/narration_800hz.wav",
    "voice": "narration",
    "volume": 0.8
  }
}'
sleep 1

mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/dialog_1200hz.wav",
    "voice": "dialog",
    "volume": 0.9
  }
}'
sleep 1

mosquitto_pub -t "$TOPIC" -m '{
  "command": "play",
  "message": {
    "file": "'"$AUDIO_DIR"'/narration_800hz.wav",
    "voice": "narration",
    "volume": 0.8
  }
}'
sleep 4

echo "[3/4] All voices ended - music restoring..."
sleep 2

echo "[4/4] Music will continue (no stop command yet)..."
sleep 1

echo ""
echo "=========================================="
echo "Demo Complete!"
echo "=========================================="
echo ""
echo "Key things to listen for:"
echo "  ✓ Smooth fades (no clicks, pops, or glitches)"
echo "  ✓ Correct ducking levels (15% for narration, 5% for dialog)"
echo "  ✓ Proper restoration when voices end"
echo "  ✓ Smooth transitions when rules change mid-fade"
echo ""

# Clean up
echo "Stopping mqttaudio daemon..."
kill $DAEMON_PID 2>/dev/null
wait $DAEMON_PID 2>/dev/null || true

echo "Demo finished!"
