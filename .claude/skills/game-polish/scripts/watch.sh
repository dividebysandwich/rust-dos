#!/usr/bin/env bash
# Samples a running rust-dos debug server in the background while the game
# is played, so performance drops and log trouble are caught without
# polling them by hand.
#
# Usage: watch.sh <host:port> <out-dir> [interval-seconds]
#
# Writes, in <out-dir>:
#   stats.jsonl   one line per sample: wall time, emulated speed, frame
#                 rate, host CPU use, video mode, audio peak and underruns
#   trouble.log   log lines that look like emulator gaps or faults
#   alerts.log    one line for each sample that crossed a threshold
#
# Stops by itself when the debug server goes away.
set -u

H="http://${1:?host:port}"
OUT="${2:?out-dir}"
INTERVAL="${3:-5}"
mkdir -p "$OUT"

# The patterns tests/gamesuite/machine.rs flags as trouble, plus panics.
TROUBLE='Unhandled|Unsupported|Unknown|Invalid opcode|Triple fault|shutdown|panic|not implemented'

after_n=0
while curl -sf "$H/api/version" >/dev/null; do
    now=$(date +%s)
    stats=$(curl -sf "$H/api/stats") || break
    status=$(curl -sf "$H/api/status") || break
    jq -cn --argjson s "$stats" --argjson st "$status" --arg t "$now" '{
        t: ($t | tonumber),
        speed: $s.frame.emulated_speed,
        fps: $s.frame.fps,
        cpu_use: $s.frame.cpu_use,
        mips: $s.mips,
        mode: $st.video.name,
        width: $st.video.width,
        height: $st.video.height,
        cycles_per_ms: $st.cycles_per_ms,
        peak: $st.audio.peak,
        underruns: $st.audio.underruns,
        paused: $st.paused
    }' >>"$OUT/stats.jsonl"

    # Slow emulation: under 0.9x real time while running, or the host
    # thread saturated. A speed of 0 is the first sample, before anything
    # was measured.
    jq -r --arg t "$now" 'select(.paused != true) |
        select((.speed > 0 and .speed < 0.9) or (.cpu_use != null and .cpu_use > 95)) |
        "\($t) slow: speed=\(.speed) fps=\(.fps) cpu_use=\(.cpu_use) mode=\(.mode)"' \
        <<<"$(tail -n1 "$OUT/stats.jsonl")" >>"$OUT/alerts.log"

    log=$(curl -sf "$H/api/log?after_n=$after_n&limit=1000") || break
    last=$(jq -r '[.lines[].n] | max // empty' <<<"$log")
    [ -n "$last" ] && after_n=$last
    jq -r --arg re "$TROUBLE" '.lines[] | select(.line | test($re)) |
        "\(.t_ms)ms #\(.n) \(.line)"' <<<"$log" >>"$OUT/trouble.log"

    sleep "$INTERVAL"
done
