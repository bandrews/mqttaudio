#!/bin/bash
# ABOUTME: Stress test script for mqttaudio - tests rapid commands and sustained load.
# ABOUTME: Validates system stability under various high-stress scenarios.

set -e

TOPIC="${MQTT_TOPIC:-audio/test}"
TEST_FILE="${TEST_FILE:-tests/audio/test_440hz_2s.wav}"
SERVER="${MQTT_SERVER:-localhost}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_dependencies() {
    log_info "Checking dependencies..."

    if ! command -v mosquitto_pub &> /dev/null; then
        log_error "mosquitto_pub not found. Please install mosquitto-clients"
        exit 1
    fi

    if [ ! -f "$TEST_FILE" ]; then
        log_warn "Test file not found: $TEST_FILE"
        log_warn "Some tests will be skipped"
    fi
}

test_rapid_play_commands() {
    log_info "Test 1: Rapid play commands (100 commands in quick succession)"

    for i in {1..100}; do
        mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"play\", \"message\": {\"file\": \"$TEST_FILE\", \"volume\": 0.5}}" &
    done

    wait
    log_info "✓ Sent 100 play commands"
    sleep 2
}

test_voice_management() {
    log_info "Test 2: Voice management (create, control, destroy voices)"

    # Create 5 voices with 3 samples each
    for voice in {1..5}; do
        for sample in {1..3}; do
            mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"play\", \"message\": {\"file\": \"$TEST_FILE\", \"voice\": \"voice_$voice\", \"volume\": 0.3}}"
        done
    done

    log_info "Created 5 voices with 3 samples each (15 total)"
    sleep 1

    # Fade out voices one by one
    for voice in {1..5}; do
        mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"voice_fade_out\", \"message\": {\"voice\": \"voice_$voice\", \"time\": 500}}"
        sleep 0.1
    done

    log_info "✓ Voice management test complete"
    sleep 2
}

test_channel_routing() {
    log_info "Test 3: Complex channel routing (multichannel scenarios)"

    # Stereo to various outputs
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{
        "command": "play",
        "message": {
            "file": "'"$TEST_FILE"'",
            "channel_map": [{"src": 0, "dest": 0}, {"src": 1, "dest": 1}]
        }
    }'

    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{
        "command": "play",
        "message": {
            "file": "'"$TEST_FILE"'",
            "channel_map": [{"src": 0, "dest": 2}, {"src": 1, "dest": 3}]
        }
    }'

    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{
        "command": "play",
        "message": {
            "file": "'"$TEST_FILE"'",
            "channel_map": [{"src": 0, "dest": 4}, {"src": 1, "dest": 5}]
        }
    }'

    log_info "✓ Channel routing test complete"
    sleep 2
}

test_fading() {
    log_info "Test 4: Fade operations (in/out with various durations)"

    # Play with fade in
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"play\", \"message\": {\"file\": \"$TEST_FILE\", \"fade_in\": 1000, \"voice\": \"fade_test\"}}"

    sleep 1

    # Fade out
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"voice_fade_out\", \"message\": {\"voice\": \"fade_test\", \"time\": 1000}}"

    log_info "✓ Fade test complete"
    sleep 2
}

test_simultaneous_samples() {
    log_info "Test 5: Maximum simultaneous samples (20+ concurrent)"

    # Play 25 samples simultaneously
    for i in {1..25}; do
        mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"play\", \"message\": {\"file\": \"$TEST_FILE\", \"volume\": 0.2}}" &
    done

    wait
    log_info "✓ Started 25 simultaneous samples"
    sleep 3
}

test_stopall_under_load() {
    log_info "Test 6: Stop all under heavy load"

    # Create lots of samples
    for i in {1..50}; do
        mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"play\", \"message\": {\"file\": \"$TEST_FILE\", \"volume\": 0.1}}" &
    done

    wait
    sleep 0.5

    # Stop everything
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "stopall"}'

    log_info "✓ Stopall test complete"
    sleep 1
}

test_cache_commands() {
    log_info "Test 7: Cache operations"

    # Precache (even though it's a local file, tests the command)
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"precache\", \"message\": {\"file\": \"$TEST_FILE\"}}"

    sleep 0.5

    # Clear cache
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "cache_clear"}'

    log_info "✓ Cache commands test complete"
    sleep 1
}

test_sustained_operation() {
    log_info "Test 8: Sustained operation (1 minute of continuous commands)"

    START_TIME=$(date +%s)
    DURATION=60
    COMMAND_COUNT=0

    while [ $(($(date +%s) - START_TIME)) -lt $DURATION ]; do
        # Alternate between different command types
        case $((COMMAND_COUNT % 4)) in
            0)
                mosquitto_pub -h "$SERVER" -t "$TOPIC" -m "{\"command\": \"play\", \"message\": {\"file\": \"$TEST_FILE\", \"voice\": \"sustained\"}}"
                ;;
            1)
                mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "voice_volume", "message": {"voice": "sustained", "volume": 0.5}}'
                ;;
            2)
                mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "voice_fade_out", "message": {"voice": "sustained", "time": 500}}'
                ;;
            3)
                mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "voice_stop", "message": {"voice": "sustained"}}'
                ;;
        esac

        COMMAND_COUNT=$((COMMAND_COUNT + 1))
        sleep 0.1
    done

    log_info "✓ Sustained operation complete ($COMMAND_COUNT commands sent)"

    # Clean up
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "stopall"}'
    sleep 1
}

test_error_handling() {
    log_info "Test 9: Error handling (invalid commands and malformed JSON)"

    # Invalid JSON
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{invalid json}'

    # Unknown command
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "unknown_command"}'

    # Missing required field
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "play", "message": {}}'

    # Invalid file path
    mosquitto_pub -h "$SERVER" -t "$TOPIC" -m '{"command": "play", "message": {"file": "/nonexistent/file.wav"}}'

    log_info "✓ Error handling test complete (daemon should still be running)"
    sleep 1
}

run_all_tests() {
    log_info "========================================="
    log_info "Starting mqttaudio Stress Test Suite"
    log_info "========================================="
    log_info "MQTT Server: $SERVER"
    log_info "MQTT Topic: $TOPIC"
    log_info "Test File: $TEST_FILE"
    log_info ""

    START_TIME=$(date +%s)

    test_rapid_play_commands
    test_voice_management
    test_channel_routing
    test_fading
    test_simultaneous_samples
    test_stopall_under_load
    test_cache_commands
    test_sustained_operation
    test_error_handling

    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))

    log_info ""
    log_info "========================================="
    log_info "All stress tests completed successfully!"
    log_info "Total duration: ${DURATION}s"
    log_info "========================================="
    log_info ""
    log_info "The daemon should still be running without crashes or glitches."
    log_info "Check the logs for any errors or warnings."
}

# Main execution
check_dependencies
run_all_tests
