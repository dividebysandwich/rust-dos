#!/usr/bin/env bash
# Tests tools/run-lock.sh against a lock directory of its own.
#
#   bash tools/run-lock-test.sh [SCRATCH_DIR] [RACE_ROUNDS]
#
# SCRATCH_DIR defaults to a new folder under TMPDIR. Exits 1 if a check
# failed. It takes a few minutes, mostly the race rounds (default 100).

set -u
here=$(cd "$(dirname "$0")" && pwd)
work=${1:-$(mktemp -d)}/run-lock-test.$$
rounds=${2:-100}
mkdir -p "$work"

export RUST_DOS_RUN_LOCK_DIR=$work/lock
export RUST_DOS_RUN_LOCK_POLL=0.1
unset RUST_DOS_RUN_LOCK_INSIDE
LOCK=$RUST_DOS_RUN_LOCK_DIR/measure
HEAVY=$RUST_DOS_RUN_LOCK_DIR/heavy
QUEUE=$RUST_DOS_RUN_LOCK_DIR/queue

passed=0
failed=0
rl() { "$BASH" "$here/run-lock.sh" "$@"; }
# For background jobs: $! is then the script's own process.
rlb() { exec "$BASH" "$here/run-lock.sh" "$@"; }

ok() {
    passed=$((passed + 1))
    echo "ok: $1"
}

bad() {
    failed=$((failed + 1))
    echo "FAILED: $1"
}

check() { # NAME CONDITION
    if eval "$2"; then ok "$1"; else bad "$1 [$2]"; fi
}

# Waits for a condition, 15 s at most.
until_true() {
    local i=0
    while ! eval "$1"; do
        i=$((i + 1))
        [ "$i" -lt 300 ] || return 1
        sleep 0.05
    done
}

reset() {
    cd "$work" || exit 1
    rm -rf "$RUST_DOS_RUN_LOCK_DIR" m
    mkdir -p "$work/m"
    cd m || exit 1
}

empty() { [ -z "$(ls -A "$1" 2>/dev/null)" ]; }
free() { [ ! -e "$LOCK" ] && empty "$HEAVY" && empty "$QUEUE"; }

# job NAME: marks its start, waits for the file go-NAME, marks its end.
job() { echo "touch started-$1; until [ -e go-$1 ]; do sleep 0.02; done; touch ended-$1"; }

# A record of a process that no longer exists.
dead_pid() {
    sleep 0 &
    local p=$!
    wait "$p"
    echo "$p"
}

record() { # SINCE PID OWNER
    printf '%s %s 0 0 %s\ncmd\n' "$1" "$2" "$3"
}

reset
rlb heavy a 30 -- sh -c "touch started-a; until [ -e started-b ]; do sleep 0.02; done" &
pa=$!
rlb heavy b 30 -- sh -c "touch started-b; until [ -e started-a ]; do sleep 0.02; done" &
pb=$!
until_true '[ -e started-a ] && [ -e started-b ]'
check "heavy jobs overlap" '[ $? -eq 0 ]'
wait "$pa" "$pb"
check "heavy tokens are gone after the jobs" free

reset
rlb heavy h 30 -- sh -c "$(job h)" &
ph=$!
until_true '[ -e started-h ]'
rlb measure m 30 -- touch started-m &
pm=$!
sleep 0.6
check "measure waits for running heavy" '[ ! -e started-m ]'
rl status > status.txt
check "status shows the heavy job with its command" 'grep -q "^heavy: h, .*touch started-h" status.txt'
check "status shows the measure holder" 'grep -q "^measure: m, .*waiting for heavy jobs" status.txt'
touch go-h
wait "$ph"
wait "$pm"
check "measure runs after heavy ends" '[ -e started-m ]'

reset
rlb heavy h1 30 -- sh -c "$(job h1)" &
p1=$!
until_true '[ -e started-h1 ]'
rlb measure m 30 -- sh -c "$(job m)" &
pm=$!
until_true '[ -e "$LOCK" ]'
rlb heavy h2 30 -- touch started-h2 &
p2=$!
sleep 0.6
check "new heavy waits behind a waiting measure" '[ ! -e started-h2 ]'
touch go-h1
until_true '[ -e started-m ]'
sleep 0.4
check "new heavy waits while a measure runs" '[ ! -e started-h2 ]'
touch go-m
wait "$p1" "$pm" "$p2"
check "heavy runs after the measure" '[ -e started-h2 ]'

reset
rlb measure m1 30 -- sh -c "$(job m1)" &
p1=$!
until_true '[ -e started-m1 ]'
rlb measure m2 30 -- sh -c "$(job m2)" &
p2=$!
until_true '[ "$(ls "$QUEUE" | wc -l)" -eq 1 ]'
rlb heavy h 30 -- touch started-h &
ph=$!
sleep 0.4
rlb measure m3 30 -- sh -c "[ -e ended-m2 ] && touch in-order; touch started-m3" &
p3=$!
until_true '[ "$(ls "$QUEUE" | wc -l)" -eq 2 ]'
rl status > status.txt
check "status shows the measures in line" 'grep -q "^waiting: m2" status.txt && grep -q "^waiting: m3" status.txt'
touch go-m1
until_true '[ -e started-m2 ]'
touch go-m2
wait "$p1" "$p2" "$p3" "$ph"
check "two measures run in the order they asked" '[ -e in-order ]'
check "heavy waits for every measure in line" '[ -e started-h ]'

reset
rl heavy e -- sh -c 'exit 7'
check "heavy passes on the exit code" '[ $? -eq 7 ]'
check "heavy releases after a failed job" free
rl measure e -- sh -c 'exit 9'
check "measure passes on the exit code" '[ $? -eq 9 ]'
check "measure releases after a failed job" free
rl heavy e -- no-such-command-here 2> /dev/null
check "a missing command gives 127" '[ $? -eq 127 ]'
echo from-stdin | rl heavy e -- cat > stdin.txt
check "CMD gets the caller's stdin" 'grep -q from-stdin stdin.txt'

signal_case() { # MODE SIGNAL CODE
    reset
    # Under job control, background jobs keep the default handling of INT.
    set -m
    rlb "$1" s 30 -- sleep 60 &
    local p=$!
    set +m
    until_true '[ -n "$(cat "$HEAVY"/* "$LOCK"/hold.* 2>/dev/null | cut -s -d" " -f4 | grep -v "^0$")" ]'
    local jobpid
    jobpid=$(cat "$HEAVY"/* "$LOCK"/hold.* 2>/dev/null | head -n 1 | cut -d" " -f4)
    kill "-$2" "$p"
    wait "$p"
    check "$1 exits with $3 on $2" '[ $? -eq '"$3"' ]'
    check "$1 releases on $2" free
    check "$1 stops CMD on $2" '! kill -0 "$jobpid" 2>/dev/null'
}
signal_case heavy TERM 143
signal_case measure TERM 143
signal_case heavy INT 130
signal_case measure INT 130

if [ -r "/proc/$$/winpid" ]; then
    reset
    rlb heavy n 30 -- ping -n 60 127.0.0.1 > /dev/null &
    p=$!
    until_true '[ -n "$(cut -s -d" " -f4 "$HEAVY"/* 2>/dev/null | grep -v "^0$")" ]'
    sleep 0.5
    native=$(cat "/proc/$(cut -s -d" " -f4 "$HEAVY"/* | head -n 1)/winpid")
    kill -TERM "$p"
    wait "$p"
    check "TERM ends the script around a native Windows program" '[ $? -eq 143 ] && free'
    check "TERM stops the native program" \
        '! tasklist //FI "PID eq $native" //FO CSV //NH | grep -qi "^\"ping"'
fi

# Kills the script and CMD as a stopped task would.
kill_both() { # SCRIPT_PID RECORD
    local jobpid
    jobpid=$(head -n 1 "$2" | cut -d" " -f4)
    kill -9 "$1"
    kill -9 "$jobpid"
    wait "$1" 2> /dev/null
}

reset
rlb heavy k 30 -- sleep 60 2> /dev/null &
p=$!
until_true 'grep -qv " 0 k$" "$HEAVY"/* 2>/dev/null && [ "$(cut -d" " -f4 "$HEAVY"/* | head -n 1)" != 0 ]'
kill_both "$p" "$HEAVY"/k.*
check "a killed heavy leaves a token" '! empty "$HEAVY"'
rl status > status.txt
check "status marks the dead token" 'grep -q "process gone" status.txt'
rl measure m 20 -- touch started-m 2> /dev/null
check "measure takes over a killed heavy token" '[ $? -eq 0 ] && [ -e started-m ] && free'

for waiter in measure heavy; do
    reset
    rlb measure k 30 -- sleep 60 2> /dev/null &
    p=$!
    until_true '[ "$(cut -d" " -f4 "$LOCK"/hold.* 2>/dev/null | head -n 1)" -gt 0 ] 2>/dev/null'
    kill_both "$p" "$LOCK"/hold.*
    rl "$waiter" w 20 -- touch started-w 2> /dev/null
    check "$waiter takes over a killed measure" '[ $? -eq 0 ] && [ -e started-w ] && free'
done

reset
rlb measure k 30 -- sleep 3 2> /dev/null &
p=$!
until_true '[ "$(cut -d" " -f4 "$LOCK"/hold.* 2>/dev/null | head -n 1)" -gt 0 ] 2>/dev/null'
kill -9 "$p"
wait "$p" 2> /dev/null
rl measure w 1 -- touch started-w 2> /dev/null
check "a hold stays while its job outlives the script" '[ $? -eq 3 ] && [ ! -e started-w ]'
rl measure w 20 -- touch started-w 2> /dev/null
check "and is taken over once the job has ended" '[ $? -eq 0 ] && [ -e started-w ] && free'

reset
now=$(date +%s)
mkdir -p "$LOCK" "$HEAVY"
record $((now - 100)) $$ old > "$LOCK/hold.x"
RUST_DOS_RUN_LOCK_MAX_AGE=50 rl measure w 20 -- touch started-w 2> /dev/null
check "the age limit ends a hold whose process seems alive" '[ -e started-w ] && free'
record $((now - 100)) $$ old > "$HEAVY/old.1"
RUST_DOS_RUN_LOCK_MAX_AGE=50 rl measure w 20 -- touch started-w2 2> /dev/null
check "the age limit ends a heavy token too" '[ -e started-w2 ] && free'

reset
mkdir -p "$LOCK"
rl measure w 1 -- touch started-w 2> /dev/null
check "a lock directory just made is respected" '[ $? -eq 3 ] && [ ! -e started-w ]'
rl clear w
touch -t 200001010000 "$LOCK"
rl measure w 20 -- touch started-w 2> /dev/null
check "a half-written lock is taken over by its age" '[ $? -eq 0 ] && [ -e started-w ] && free'

# A waiter judges a hold dead and stalls. A faster one takes over and
# runs. The slow one must then leave the new hold alone.
reset
gone=$(dead_pid)
mkdir -p "$LOCK"
record "$(date +%s)" "$gone" dead > "$LOCK/hold.slow"
RUST_DOS_RUN_LOCK_TEST_DELAY=1.5 rlb heavy slow 30 -- touch started-slow 2> /dev/null &
ps=$!
sleep 0.7
rlb measure fast 30 -- sh -c "$(job fast)" 2> /dev/null &
pf=$!
until_true '[ -e started-fast ]'
sleep 1.5
check "a slow waiter leaves a new hold alone" '[ -e "$LOCK"/hold.* ] && [ ! -e started-slow ]'
touch go-fast
wait "$ps" "$pf"

# Measures and heavy jobs find one dead hold at once, each stalling a
# random moment after judging it dead. Every job checks that no job it
# excludes is inside. Removing a hold that a faster waiter just took
# shows up as two jobs inside.
measure_job='mkdir inside 2>/dev/null || touch double; ls h.* >/dev/null 2>&1 && touch double
sleep 0.03; rmdir inside; touch "done-$0"'
heavy_job='touch h.$0; [ -d inside ] && touch double; sleep 0.03; [ -d inside ] && touch double
rm h.$0; touch "done-$0"'
reset
gone=$(dead_pid)
i=0
while [ "$i" -lt "$rounds" ]; do
    i=$((i + 1))
    rm -f done-*
    mkdir -p "$LOCK"
    record "$(date +%s)" "$gone" dead > "$LOCK/hold.$i"
    pids=
    for w in 1 2 3 4 5; do
        body=$measure_job
        mode=measure
        if [ "$w" -gt 3 ]; then
            body=$heavy_job
            mode=heavy
        fi
        RUST_DOS_RUN_LOCK_TEST_DELAY=0.$((RANDOM % 3))$((RANDOM % 10)) \
            rlb "$mode" "w$w" 60 -- sh -c "$body" "$w" 2> /dev/null &
        pids="$pids $!"
    done
    wait $pids
    [ "$(ls done-* 2>/dev/null | wc -l)" -eq 5 ] || touch missing
done
check "$rounds takeover races: no double hold" '[ ! -e double ]'
check "$rounds takeover races: every waiter ran" '[ ! -e missing ] && free'

reset
rlb measure m1 30 -- sh -c "$(job m1)" &
p1=$!
until_true '[ -e started-m1 ]'
rl measure late 1 -- touch started-late 2> err.txt
check "a timeout exits 3" '[ $? -eq 3 ] && [ ! -e started-late ]'
check "a timeout says what to do" 'grep -q "keep your place" err.txt'
t=$(ls "$QUEUE")
check "a timeout leaves a ticket" '[ -n "$t" ] && [ ! -s "$QUEUE/$t" ]'
rlb measure next 30 -- sh -c "[ -e started-late ] && touch place-kept" &
p2=$!
until_true '[ "$(ls "$QUEUE" | wc -l)" -eq 2 ]'
rlb measure late 30 -- touch started-late &
p3=$!
until_true '[ -s "$QUEUE/$t" ]'
touch go-m1
wait "$p1" "$p2" "$p3"
check "a rerun keeps the place in line" '[ -e place-kept ]'

reset
rlb measure m1 30 -- sh -c "$(job m1)" &
p1=$!
until_true '[ -e started-m1 ]'
RUST_DOS_RUN_LOCK_QUEUE_GRACE=1 rl measure late 1 -- true 2> /dev/null
sleep 2.2
RUST_DOS_RUN_LOCK_QUEUE_GRACE=1 rl measure next 1 -- true 2> /dev/null
check "a place not renewed in time is dropped" '[ "$(ls "$QUEUE" | grep -c late)" -eq 0 ]'
touch go-m1
wait "$p1"

reset
rlb heavy h 30 -- sh -c "$(job h)" &
ph=$!
until_true '[ -e started-h ]'
rl measure m 1 -- touch started-m 2> /dev/null
check "a measure timing out behind heavy exits 3" '[ $? -eq 3 ] && [ ! -e "$LOCK" ]'
check "and keeps its place" '[ -n "$(ls "$QUEUE")" ]'
touch go-h
wait "$ph"
rl clear m
check "clear drops a kept place" free

reset
for waiter in measure heavy; do
    rlb measure m 30 -- sh -c "$(job m)" &
    p=$!
    until_true '[ -e started-m ]'
    "$BASH" -c "\"\$BASH\" '$here/run-lock.sh' $waiter gone 30 -- touch started-gone 2> /dev/null; echo \$? > outer.txt" &
    pc=$!
    sleep 0.5
    # Kill the shell the script was called from. Its child goes on alone.
    kill -9 "$pc"
    wait "$pc" 2> /dev/null
    if [ "$waiter" = measure ]; then
        until_true 'empty "$QUEUE"'
        check "a measure whose caller died leaves the line" '[ $? -eq 0 ]'
    else
        sleep 0.5
    fi
    touch go-m
    wait "$p"
    sleep 0.5
    check "$waiter with a dead caller never runs CMD" '[ ! -e started-gone ]'
    rm -f go-m started-m
done

reset
# The subshell exits at once, so the script starts with PPID 1, as it
# does under Python's subprocess on MSYS.
rlb measure m 30 -- sh -c "$(job m)" &
p=$!
until_true '[ -e started-m ]'
(
    { sleep 0.5; "$BASH" "$here/run-lock.sh" heavy orphan 30 -- touch started-orphan; } > /dev/null 2>&1 &
)
sleep 1.5
touch go-m
wait "$p"
until_true '[ -e started-orphan ]'
check "PPID 1 skips the dead caller check" '[ $? -eq 0 ]'

reset
rl heavy o 30 -- "$BASH" "$here/run-lock.sh" measure o 2 -- touch started-in 2> err.txt
check "measure inside heavy is refused" '[ $? -eq 5 ] && [ ! -e started-in ]'
rl measure o 30 -- "$BASH" "$here/run-lock.sh" heavy o 2 -- touch started-in 2>> err.txt
check "heavy inside measure is refused" '[ $? -eq 5 ] && [ ! -e started-in ]'
rl heavy o 30 -- "$BASH" "$here/run-lock.sh" heavy o 2 -- touch started-in 2>> err.txt
check "heavy inside heavy is refused" '[ $? -eq 5 ] && [ ! -e started-in ]'

reset
rlb measure same 30 -- sh -c "$(job same)" &
p=$!
until_true '[ -e started-same ]'
rl measure same 1 -- touch started-again 2> err.txt
check "no re-entry under the holder's name" '[ $? -eq 3 ] && [ ! -e started-again ]'
check "re-entry gives a warning" 'grep -q "no re-entry" err.txt'
rl clear same
touch go-same
wait "$p"

reset
rlb measure c 30 -- sleep 3 2> /dev/null &
p=$!
until_true '[ -n "$(ls "$LOCK" 2>/dev/null)" ]'
sleep 0.3
rlb heavy c2 30 -- sleep 3 2> /dev/null &
kill -9 "$p"
rl clear c
check "clear removes a hold" '[ ! -e "$LOCK" ]'
wait 2> /dev/null

reset
rl heavy 2> /dev/null
check "usage errors exit 2" '[ $? -eq 2 ]'
rl heavy "bad/name" -- true 2> /dev/null
check "a bad owner exits 2" '[ $? -eq 2 ]'
rl heavy a abc -- true 2> /dev/null
check "a bad wait exits 2" '[ $? -eq 2 ]'

cd / || exit 1
rm -rf "$work"
echo "$passed passed, $failed failed on $(uname -s), bash $BASH_VERSION"
[ "$failed" -eq 0 ]
