#!/usr/bin/env bash
# Keeps rust-dos performance measurements clean on a machine shared by
# several agents or sessions, across every checkout and worktree. Builds,
# tests and emulator runs may run side by side; a measurement runs alone.
# Runs in plain bash on Linux and macOS and in Git Bash (MSYS) on Windows.
#
#   bash tools/run-lock.sh heavy OWNER [MAX_WAIT_S] -- CMD...
#   bash tools/run-lock.sh measure OWNER [MAX_WAIT_S] -- CMD...
#   bash tools/run-lock.sh status
#   bash tools/run-lock.sh clear OWNER
#
# heavy waits while a measurement runs or waits, then runs CMD. Any number
# of heavy jobs run at once.
#
# measure waits for its turn among the measurements, blocks new heavy
# jobs from the moment it asks, waits for the running heavy jobs to end,
# then runs CMD alone. Measurements go in the order they first asked.
#
# Either mode releases when CMD ends, however it ends, and exits with
# CMD's exit code. A TERM, INT or HUP sent to the script sends TERM to
# CMD, waits for it to end, and exits with 128 + the signal's number. The
# hold lasts as long as CMD: a script that starts an emulator in the
# background must wait for it before it returns.
#
# status lists the measurement holder, the running heavy jobs with their
# command and age, and the measurements in line. clear removes OWNER's
# hold, heavy tokens and place in line; use it only after OWNER's worker
# has stopped.
#
# OWNER is a short tag such as issue-123 (letters, digits, `.`, `_`, `-`).
# The lock lives in RUST_DOS_RUN_LOCK_DIR (default ~/.rust-dos-run-lock),
# outside the checkout, so every checkout on the machine shares it. Give
# every caller the same directory.
#
# Waits last MAX_WAIT_S seconds (default 540, under a 10-minute tool
# timeout; 0 waits forever), polling every RUST_DOS_RUN_LOCK_POLL seconds
# (default 2). Exit codes of the script itself:
#
#   2  bad usage
#   3  timed out, CMD did not run. A measurement keeps its place in line
#      if run again under the same OWNER within 2 minutes
#      (RUST_DOS_RUN_LOCK_QUEUE_GRACE), and its place blocks new heavy
#      jobs until then.
#   4  the shell that called the script died during the wait, CMD did not
#      run. A stopped task thus can't take the lock later for nobody. A
#      caller from outside MSYS (Python's subprocess) shows as PPID 1,
#      can't be watched, and skips this check.
#   5  called from inside another run-lock job
#
# CMD can exit with any code, 3 and 4 too. The script says on stderr when
# it gave up without running CMD.
#
# Rules, from clashes that happened:
#
# - Every job takes the lock: each cargo build, test or clippy, and each
#   emulator run, whatever its exe is called, including the ones a
#   background script, a monitor or a benchmark loop starts.
# - Never call one mode inside the other, or either one inside itself:
#   the inner call waits for a measurement that waits for the outer call,
#   forever. The script refuses (exit 5) when it can tell, through
#   RUST_DOS_RUN_LOCK_INSIDE in CMD's environment. A job that loses its
#   environment on the way, such as one started over ssh, deadlocks
#   instead, so don't rely on the check.
# - A copied script carries its lock. Before you run another agent's
#   bench or session script, read how it locks. A script that takes the
#   lock itself must not be wrapped in your own call.
# - OWNER is yours alone: your agent's name, or NAME-<pid> in a script.
#   There is no re-entry: a measure under the holder's own name waits
#   like any other, so two scripts sharing a name can't both run.
# - Don't delete anything in the lock directory, or clear a worker that
#   is still running. Waiters take over what killed jobs leave behind.
# - Stop an emulator, and check by process ID (or by command line for a
#   renamed exe) that it has exited, before CMD returns.
# - A measurement counts only if no other emulator (any exe started with
#   --debug-server or --config) and no cargo or rustc ran during it, also
#   ones nobody registered here. Check by command line at the start and
#   the end of each run, not only for rust-dos.exe in tasklist, and
#   retake a disturbed run.
#
# How holds end when their job was killed: each heavy token and measure
# hold records the script's process ID and CMD's. Any waiter removes a
# record once both processes are gone. On MSYS these are MSYS process
# IDs, which `kill -0` understands and Windows tools don't; the record
# also has the script's Windows process ID, checked with tasklist before
# a hold counts as dead, for a caller from another MSYS install. A record
# older than 4 hours (RUST_DOS_RUN_LOCK_MAX_AGE) is removed even if its
# process seems alive, in case the ID was reused, so a longer job loses
# its hold. A killed job's children may outlive both processes; the check
# by command line above catches those.

set -u

DIR=${RUST_DOS_RUN_LOCK_DIR:-$HOME/.rust-dos-run-lock}
LOCK=$DIR/measure
HEAVY=$DIR/heavy
QUEUE=$DIR/queue
POLL=${RUST_DOS_RUN_LOCK_POLL:-2}
MAX_AGE=${RUST_DOS_RUN_LOCK_MAX_AGE:-14400}
QUEUE_GRACE=${RUST_DOS_RUN_LOCK_QUEUE_GRACE:-120}
# A lock directory with no hold in it yet may belong to a script between
# its mkdir and its next line. After this long it belongs to a dead one.
FILL_GRACE=60

me=$$.$RANDOM$RANDOM
winpid=0
[ -r "/proc/$$/winpid" ] && read -r winpid < "/proc/$$/winpid"

hold=
token=
ticket=
keep_ticket=
job=
waited=
owner=
cmd=
since=0
r_owner=

log() { echo "run-lock: $*" >&2; }

usage() {
    echo "usage: run-lock.sh heavy|measure OWNER [MAX_WAIT_S] -- CMD..." >&2
    echo "       run-lock.sh status" >&2
    echo "       run-lock.sh clear OWNER" >&2
    exit 2
}

valid_owner() {
    case $1 in
    '' | *[!A-Za-z0-9._-]*) usage ;;
    esac
}

mtime() { stat -c %Y "$1" 2>/dev/null || stat -f %m "$1" 2>/dev/null; }

# alive PID [WINPID]. A Windows process ID must still name a bash, since
# Windows hands out freed IDs again quickly.
alive() {
    if [ "${1:-0}" != 0 ] && kill -0 "$1" 2>/dev/null; then
        return 0
    fi
    [ "${2:-0}" != 0 ] || return 1
    tasklist //FI "PID eq $2" //FO CSV //NH 2>/dev/null |
        grep -Eqi "^\"(ba)?sh\.exe\",\"$2\""
}

# Each record is written whole and renamed into place, so nobody reads a
# half-written owner or time, and a heavy token never shows up empty.
write_record() { # FILE JOB_PID
    local tmp=$DIR/tmp.$me
    { echo "$since $$ $winpid $2 $owner"; echo "$cmd"; } > "$tmp" &&
        mv -f "$tmp" "$1" 2>/dev/null && return 0
    rm -f "$tmp"
    return 1
}

read_record() { # FILE, into the r_ variables; false if it has gone
    r_since= r_pid=0 r_win=0 r_job=0 r_owner= r_cmd=
    {
        read -r r_since r_pid r_win r_job r_owner
        read -r r_cmd
    } 2>/dev/null < "$1"
    [ -n "$r_since" ]
}

record_stale() { # NOW, after read_record
    [ $(($1 - r_since)) -gt "$MAX_AGE" ] && return 0
    alive "$r_pid" "$r_win" && return 1
    alive "$r_job" && return 1
    return 0
}

# Takes the hold away from whoever has it. The file's name is unique to
# one hold, so of everyone who saw this hold, exactly one rename works:
# the slower ones fail, also when a new hold has taken the lock since
# they looked. The directory then stays until the winner's own rename
# frees the lock in one step.
remove_hold() { # FILE
    mv "$1" "$LOCK/claim.$me" 2>/dev/null || return 1
    local dead=$DIR/dead.$me.$RANDOM
    if mv "$LOCK" "$dead" 2>/dev/null; then
        rm -rf "$dead"
    else
        rm -rf "$LOCK"
    fi
}

# Looks at a lock directory that isn't ours, and takes over a dead hold.
# Sets held_by for the messages.
check_hold() {
    local f age now dead
    now=$(date +%s)
    held_by=
    set -- "$LOCK"/hold.*
    f=$1
    if [ -e "$f" ]; then
        read_record "$f" || return 0
        age=$((now - r_since))
        if record_stale "$now"; then
            # tools/run-lock-test.sh plays a slow waiter with this.
            sleep "${RUST_DOS_RUN_LOCK_TEST_DELAY:-0}"
            log "taking over the hold of $r_owner (age ${age}s)"
            remove_hold "$f"
        else
            held_by="$r_owner (age ${age}s)"
        fi
        return 0
    fi
    # No hold file: a script is between mkdir and writing it, or one died
    # there, or someone is removing the hold. The directory's own time
    # gives the age. Only the first in line acts on it, and a new hold can
    # only come from the first in line, so the look below stays true
    # unless the script that made this directory was merely stuck.
    age=$(mtime "$LOCK") || return 0
    age=$((now - age))
    held_by="a hold being set up (age ${age}s)"
    [ "$age" -gt "$FILL_GRACE" ] || return 0
    [ -z "$first" ] || [ "$first" = "$ticket" ] || return 0
    dead=$DIR/dead.$me.$RANDOM
    mv "$LOCK" "$dead" 2>/dev/null || return 0
    set -- "$dead"/hold.*
    if [ -e "$1" ]; then
        mv "$dead" "$LOCK"
        return 0
    fi
    log "taking over a hold left half written (age ${age}s)"
    rm -rf "$dead"
}

# Drops tickets of waiters that died or stopped renewing, and sets first
# to the ticket at the head of the line. A ticket left by a timeout is
# empty: it has no process to check and lasts for the grace period.
sweep_queue() {
    local f m p w now
    now=$(date +%s)
    first=
    for f in "$QUEUE"/*; do
        [ -e "$f" ] || continue
        m=$(mtime "$f") || continue
        p=
        read -r p w 2>/dev/null < "$f"
        if [ $((now - m)) -gt "$QUEUE_GRACE" ]; then
            rm -f "$f"
        elif [ -n "$p" ] && ! alive "$p" "$w"; then
            rm -f "$f"
        elif [ -z "$first" ]; then
            first=${f##*/}
        fi
    done
}

# Counts the running heavy jobs, dropping the tokens of killed ones.
heavy_running() {
    local f now
    now=$(date +%s)
    running=0
    for f in "$HEAVY"/*; do
        [ -e "$f" ] || continue
        read_record "$f" || continue
        if record_stale "$now"; then
            log "dropping the heavy token of $r_owner: its job is gone"
            rm -f "$f"
        else
            running=$((running + 1))
        fi
    done
    [ "$running" -gt 0 ]
}

# One failed pass of a wait: leaves if the caller died or time ran out,
# otherwise sleeps.
wait_turn() { # REASON
    if [ "$caller" != 1 ] && ! alive "$caller" "$caller_win"; then
        log "the caller of $mode $owner is gone: leaving the line"
        exit 4
    fi
    if [ "$max_wait" -gt 0 ] && [ $(($(date +%s) - started)) -ge "$max_wait" ]; then
        if [ -n "$ticket" ]; then
            keep_ticket=1
            log "still $1; run it again within ${QUEUE_GRACE}s to keep your place in line"
        else
            log "still $1"
        fi
        exit 3
    fi
    [ -n "$waited" ] || log "waiting: $1"
    waited=1
    sleep "$POLL"
}

# Runs in the background so that a signal interrupts `wait` at once. Such
# a job would get /dev/null as stdin, so it is handed ours.
run_job() { # FILE CMD...
    local file=$1
    shift
    { : <&0; } 2>/dev/null || exec 0< /dev/null
    since=$(date +%s)
    export RUST_DOS_RUN_LOCK_INSIDE=$mode
    "$@" <&0 &
    job=$!
    write_record "$file" "$job"
    wait "$job"
    exit $?
}

run_heavy() {
    token=$HEAVY/$owner.$me
    # Register first, then check: a measurement that takes the lock after
    # this check sees the token and waits for it.
    while :; do
        since=$(date +%s)
        write_record "$token" 0
        sweep_queue
        [ -z "$first" ] && [ ! -d "$LOCK" ] && break
        rm -f "$token"
        if [ -d "$LOCK" ]; then
            check_hold
            [ -d "$LOCK" ] || continue
            wait_turn "behind the measurement of $held_by"
        else
            wait_turn "behind ${first#*-}'s waiting measurement"
        fi
    done
    run_job "$token" "$@"
}

new_ticket() {
    local t
    t=$(date +%s%N)
    # macOS's date has no %N and prints it as is.
    case $t in
    *N) t=${t%N}000000000 ;;
    esac
    ticket=$t-$owner
}

run_measure() {
    local f b warned=
    # Keep the place in line from an earlier call that timed out.
    for f in "$QUEUE"/*; do
        [ -e "$f" ] || continue
        b=${f##*/}
        if [ "${b#*-}" = "$owner" ] && [ ! -s "$f" ]; then
            ticket=$b
        fi
    done
    [ -n "$ticket" ] || new_ticket
    while :; do
        [ -e "$QUEUE/$ticket" ] || new_ticket
        echo "$$ $winpid" > "$QUEUE/$ticket"
        sweep_queue
        if [ -n "$hold" ] && [ ! -e "$hold" ]; then
            log "lost the hold while waiting for heavy jobs"
            hold=
        fi
        if [ -z "$hold" ] && [ "$first" = "$ticket" ] && mkdir "$LOCK" 2>/dev/null; then
            since=$(date +%s)
            hold=$LOCK/hold.$me
            write_record "$hold" 0 || hold=
        fi
        if [ -n "$hold" ]; then
            heavy_running || break
            wait_turn "waiting for $running heavy job(s) to end"
        elif [ -d "$LOCK" ]; then
            check_hold
            [ -d "$LOCK" ] || continue
            if [ -z "$warned" ] && [ "$r_owner" = "$owner" ]; then
                log "the lock is already held under the name $owner: there is no re-entry"
                warned=1
            fi
            wait_turn "held by $held_by"
        else
            wait_turn "free, with ${first#*-} ahead of you in line"
        fi
    done
    rm -f "$QUEUE/$ticket"
    ticket=
    run_job "$hold" "$@"
}

show_record() { # LABEL NOW
    local gone=
    alive "$r_pid" "$r_win" || alive "$r_job" || gone=" (process gone, the next waiter clears it)"
    echo "$1: $r_owner, pid $r_pid, $(($2 - r_since))s: $r_cmd$gone"
}

status() {
    local f b now
    now=$(date +%s)
    set -- "$LOCK"/hold.*
    if [ -e "$1" ] && read_record "$1"; then
        [ "$r_job" != 0 ] || r_cmd="waiting for heavy jobs before: $r_cmd"
        show_record measure "$now"
    elif [ -d "$LOCK" ]; then
        echo "measure: a hold being set up or removed"
    else
        echo "measure: free"
    fi
    for f in "$HEAVY"/*; do
        [ -e "$f" ] || continue
        read_record "$f" && show_record heavy "$now"
    done
    for f in "$QUEUE"/*; do
        [ -e "$f" ] || continue
        b=${f##*/}
        t=${b%%-*}
        if [ -s "$f" ]; then
            echo "waiting: ${b#*-}, $((now - ${t%?????????}))s"
        else
            echo "waiting: ${b#*-}, $((now - ${t%?????????}))s, timed out, place kept"
        fi
    done
}

clear_owner() {
    local f b
    for f in "$HEAVY"/*; do
        [ -e "$f" ] || continue
        read_record "$f" && [ "$r_owner" = "$1" ] && rm -f "$f"
    done
    for f in "$QUEUE"/*; do
        [ -e "$f" ] || continue
        b=${f##*/}
        [ "${b#*-}" = "$1" ] && rm -f "$f"
    done
    set -- "$1" "$LOCK"/hold.*
    if [ -e "$2" ] && read_record "$2" && [ "$r_owner" = "$1" ]; then
        remove_hold "$2"
    fi
    return 0
}

cleanup() {
    rm -f "$DIR/tmp.$me"
    [ -z "$token" ] || rm -f "$token"
    if [ -n "$hold" ]; then
        local try
        # On Windows a rename fails while someone has the file open.
        for try in 1 2 3 4 5; do
            remove_hold "$hold" || [ -e "$hold" ] || break
            [ -e "$hold" ] || break
            sleep 0.1
        done
    fi
    if [ -n "$ticket" ]; then
        if [ -n "$keep_ticket" ]; then
            : > "$QUEUE/$ticket"
        else
            rm -f "$QUEUE/$ticket"
        fi
    fi
}

on_signal() { # NUMBER
    if [ -n "$job" ]; then
        kill -TERM "$job" 2>/dev/null
        wait "$job" 2>/dev/null
    fi
    exit $((128 + $1))
}

mode=${1:-}
[ $# -gt 0 ] && shift

case $mode in
heavy | measure)
    [ $# -ge 3 ] || usage
    owner=$1
    shift
    valid_owner "$owner"
    max_wait=540
    if [ "$1" != -- ]; then
        max_wait=$1
        shift
        case $max_wait in
        '' | *[!0-9]*) usage ;;
        esac
    fi
    [ "${1:-}" = -- ] && [ $# -ge 2 ] || usage
    shift
    if [ -n "${RUST_DOS_RUN_LOCK_INSIDE:-}" ]; then
        log "$mode $owner called from inside a $RUST_DOS_RUN_LOCK_INSIDE job:" \
            "it would deadlock, so CMD did not run"
        exit 5
    fi
    cmd=$*
    started=$(date +%s)
    # The shell that runs this script. If it dies during a wait, nobody
    # is left to use the job's result.
    caller=$PPID
    caller_win=0
    [ -r "/proc/$caller/winpid" ] && read -r caller_win < "/proc/$caller/winpid"
    mkdir -p "$HEAVY" "$QUEUE" || exit 2
    trap cleanup EXIT
    trap 'on_signal 1' HUP
    trap 'on_signal 2' INT
    trap 'on_signal 15' TERM
    "run_$mode" "$@"
    ;;
status)
    [ $# -eq 0 ] || usage
    mkdir -p "$HEAVY" "$QUEUE" || exit 2
    status
    ;;
clear)
    [ $# -eq 1 ] || usage
    valid_owner "$1"
    mkdir -p "$HEAVY" "$QUEUE" || exit 2
    clear_owner "$1"
    ;;
*)
    usage
    ;;
esac
