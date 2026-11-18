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
# Stops by itself when the debug server goes away: when it hasn't
# answered for MISSES samples in a row, or when another emulator answers
# on its port.
set -u

H="http://${1:?host:port}"
OUT="${2:?out-dir}"
INTERVAL="${3:-5}"
mkdir -p "$OUT"

# The patterns tests/program/gamesuite/machine.rs flags as trouble, plus
# panics.
TROUBLE='Unhandled|Unsupported|Unknown|Invalid opcode|Triple fault|shutdown|panic|not implemented'

# A request can fail while the emulator is busy (a state save, a slow
# frame): the sample is skipped and the next one tried. The watcher stops
# when the server hasn't answered for MISSES samples in a row. The
# emulator it first answers for is the one watched (/api/version's
# instance): one started later on the same port ends the watcher, so its
# samples and log lines don't go into this one's files.
MISSES=6
get() { curl -sf --max-time 10 "$H$1"; }

after_n=0
misses=0
watched=
while :; do
    if ! version=$(get /api/version) ||
        ! instance=$(jq -re .instance <<<"$version" 2>/dev/null); then
        misses=$((misses + 1))
        [ "$misses" -ge "$MISSES" ] && break
        sleep "$INTERVAL"
        continue
    fi
    misses=0
    [ -z "$watched" ] && watched=$instance
    [ "$instance" != "$watched" ] && break
    now=$(date +%s)
    if ! stats=$(get /api/stats) || ! status=$(get /api/status); then
        sleep "$INTERVAL"
        continue
    fi
    # A reply that isn't JSON (cut short) skips the sample too.
    if ! sample=$(jq -cn --argjson s "$stats" --argjson st "$status" --arg t "$now" '{
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
    }' 2>/dev/null); then
        sleep "$INTERVAL"
        continue
    fi
    printf '%s\n' "$sample" >>"$OUT/stats.jsonl"

    # Slow emulation: under 0.9x real time while running, or the host
    # thread saturated. A speed of 0 is the first sample, before anything
    # was measured.
    jq -r --arg t "$now" 'select(.paused != true) |
        select((.speed > 0 and .speed < 0.9) or (.cpu_use != null and .cpu_use > 95)) |
        "\($t) slow: speed=\(.speed) fps=\(.fps) cpu_use=\(.cpu_use) mode=\(.mode)"' \
        <<<"$sample" >>"$OUT/alerts.log"

    if log=$(get "/api/log?after_n=$after_n&limit=1000"); then
        last=$(jq -r '[.lines[].n] | max // empty' <<<"$log")
        [ -n "$last" ] && after_n=$last
        jq -r --arg re "$TROUBLE" '.lines[] | select(.line | test($re)) |
            "\(.t_ms)ms #\(.n) \(.line)"' <<<"$log" >>"$OUT/trouble.log"
    fi

    sleep "$INTERVAL"
done
