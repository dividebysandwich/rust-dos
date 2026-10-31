#!/bin/sh
# One rust-dos job at a time on a Windows machine shared by several agents
# or sessions, across every checkout and worktree: a rust-dos.exe run, or
# a cargo build, test or clippy, since either one disturbs a benchmark
# running alongside it. Runs in Git Bash (MSYS); it looks for processes
# with tasklist and PowerShell, so it is for Windows only.
#
#   sh tools/runlock-windows.sh acquire NAME [max_wait_s]
#   sh tools/runlock-windows.sh release NAME
#   sh tools/runlock-windows.sh status
#
# The lock lives in RUST_DOS_RUNLOCK_DIR (default ~/.rust-dos-runlock), not
# in the checkout, so every checkout on the machine shares it. Use the
# script from any checkout, but give every caller the same directory.
#
# acquire waits its turn (polling every 2 s), up to max_wait_s seconds
# (default 540, under a 10-minute tool timeout). Waiters take turns
# in the order they first asked: each acquire leaves a ticket in
# the queue directory, and only the oldest ticket may take a free lock, so a
# holder that releases and acquires again goes to the back of the line.
# It exits 0 once NAME holds the lock, or 3 on timeout: run it again within
# 2 minutes to keep your place (a ticket not renewed for 2 minutes is
# dropped). Acquire right before the cargo command or emulator starts and
# release right after it ends, even when it fails. Don't hold the lock
# while reading results or editing code.
#
# Rules, from clashes that happened:
#
# - Every job takes the lock: each cargo build, test or clippy, and each
#   emulator run, whatever its exe is called, including the ones a
#   background script, a monitor or a benchmark loop starts. Chain it:
#   `sh tools/runlock-windows.sh acquire NAME && cargo ...`. Never `;`, which
#   runs the job when acquire timed out.
# - NAME is yours alone: your agent's name, or NAME-<pid> in a script.
#   There is no re-entry: an acquire under the holder's own name waits
#   like any other, so two scripts sharing a name can't both run. Release
#   before you acquire again.
# - A copied script carries its lock name. Before you run another
#   agent's bench or session script, read how it locks. A script that
#   takes the lock itself (a bench script that acquires as bench-<pid>)
#   must not be wrapped in your own acquire, or it waits for you forever.
# - Before an emulator ends, check by process ID, or by command line for a
#   renamed exe, that it has exited, and only then release.
# - A task stopped while its acquire waits used to leave the acquire
#   running, which took the lock later and held it with no job. acquire
#   now checks every 2 s that the shell that called it is alive, and
#   leaves the line (exit 4) when it isn't, so stopping a waiting task is
#   safe. A job already holding the lock still has to release. A caller
#   from outside MSYS (Python's subprocess) can't be watched and skips
#   the check, so a bench script's acquire behaves as before.
# - Don't delete the lock directory or release a lock under someone
#   else's name. A stale lock is taken over by acquire on its own. The
#   one exception: the coordinator that stopped an agent may release a
#   hold that agent's orphaned acquire took, once no job of it runs.
# - A measurement counts only if no other emulator (any exe started with
#   --debug-server or --config) and no cargo or rustc ran during it. Check
#   by command line at the start and the end of each run, not only for
#   rust-dos.exe in tasklist, and retake a disturbed run.
#
# A lock is stale, and is taken over, when it is older than 5 minutes and
# a waiter has seen nothing of rust-dos's run for 60 s in a row (so the
# gap between two chained commands doesn't count), or older than 4 hours
# whatever runs. What counts: a rust-dos.exe, a cargo or rustc, or any
# process started with --debug-server or --config (an emulator copied
# under another name). The usual cause is a job whose task was stopped
# after it took the lock and before it released.

DIR=${RUST_DOS_RUNLOCK_DIR:-$HOME/.rust-dos-runlock}
mkdir -p "$DIR" || exit 2
LOCK=$DIR/rust-dos-run.lock
QUEUE=$DIR/rust-dos-run.queue
QUEUE_GRACE=120
cmd=$1
name=$2

running() {
    tasklist 2>/dev/null | grep -qi -E '^(rust-dos|cargo|rustc)\.exe' && return 0
    powershell -NoProfile -Command "Get-CimInstance Win32_Process | Where-Object { \$_.CommandLine -match '\.exe\"? .*--(debug-server|config)' -and \$_.Name -notmatch '^(powershell|pwsh|bash|sh|python[0-9.]*)\.exe$' } | Select-Object -First 1 -Expand ProcessId" 2>/dev/null | grep -q '[0-9]'
}
# The hold's age. A lock directory whose `since` isn't there yet, or is
# half written, was made a moment ago by an acquire still filling it in:
# its age comes from the directory's own time instead, which is the
# moment it was made. (It used to count from 0, so a waiter that looked
# in that moment saw a hold decades old and took it over.) An acquire
# that died before writing `since` still ages that way and is taken
# over in time.
age() {
    s=$(cat "$LOCK/since" 2>/dev/null)
    case $s in
    '' | *[!0-9]*) s=$(stat -c %Y "$LOCK" 2>/dev/null || date +%s) ;;
    esac
    echo $(( $(date +%s) - s ))
}

case $cmd in
acquire)
    [ -n "$name" ] || { echo "usage: acquire NAME [max_wait_s]"; exit 2; }
    max=${3:-540}
    waited=0
    # The shell that runs `acquire NAME && job`. If it dies while we wait
    # (its task was stopped), the job will never run: leave the line
    # instead of taking the lock for nobody.
    caller=$PPID
    mkdir -p "$QUEUE"
    # Keep the place in line from an earlier acquire that timed out.
    ticket=
    for f in "$QUEUE"/*; do
        [ -e "$f" ] || continue
        b=${f##*/}
        [ "${b#*-}" = "$name" ] && ticket=$b
    done
    while :; do
        if [ -z "$ticket" ] || [ ! -e "$QUEUE/$ticket" ]; then
            ticket="$(date +%s%N)-$name"
        fi
        # A caller outside MSYS (Python's subprocess, a native program)
        # shows as PPID 1, which can't be watched: skip the check there.
        if [ "$caller" != 1 ] && ! kill -0 "$caller" 2>/dev/null; then
            rm -f "$QUEUE/$ticket"
            echo "the caller of acquire $name is gone: leaving the line"
            exit 4
        fi
        touch "$QUEUE/$ticket"
        # Waiters who stopped asking leave the line.
        now=$(date +%s)
        for f in "$QUEUE"/*; do
            [ -e "$f" ] || continue
            # Another waiter may take its ticket away between the listing
            # and the stat: then there is nothing to check.
            m=$(stat -c %Y "$f" 2>/dev/null) || continue
            [ $((now - m)) -gt $QUEUE_GRACE ] && rm -f "$f"
        done
        first=$(ls "$QUEUE" | sort | head -n 1)
        if [ "$first" = "$ticket" ] && mkdir "$LOCK" 2>/dev/null; then
            # Write each file whole and then rename it into place, so no
            # one reads a half-written owner or time.
            echo "$name" > "$LOCK/owner.new" && mv -f "$LOCK/owner.new" "$LOCK/owner"
            date +%s > "$LOCK/since.new" && mv -f "$LOCK/since.new" "$LOCK/since"
            rm -f "$QUEUE/$ticket"
            echo "lock acquired by $name"
            exit 0
        fi
        if [ -d "$LOCK" ]; then
            owner=$(cat "$LOCK/owner" 2>/dev/null)
            # No re-entry: two scripts sharing a name would both "hold" the
            # lock. An acquire under the holder's own name waits like any
            # other; release first if the lock really is yours.
            if [ "$owner" = "$name" ] && [ "$waited" -eq 0 ]; then
                echo "the lock is already held under the name $name: if it is yours, release it first"
            fi
            a=$(age)
            # How long this waiter has seen the hold idle, in a row; it
            # starts again for a new hold. `running` starts a PowerShell,
            # so look every 10 s only: every waiter does this, and a
            # benchmark may be running.
            hold="$owner $(cat "$LOCK/since" 2>/dev/null)"
            if [ "$a" -le 300 ]; then
                idle_hold=
            elif [ $((waited % 10)) -eq 0 ]; then
                if running; then
                    idle_hold=
                elif [ "$hold" != "$idle_hold" ]; then
                    idle_hold=$hold
                    idle_from=$(date +%s)
                fi
            fi
            idle=0
            [ -n "$idle_hold" ] && idle=$(( $(date +%s) - idle_from ))
            if [ "$idle" -ge 60 ] || [ "$a" -gt 14400 ]; then
                echo "taking over a stale lock from $owner (age ${a}s)"
                rm -rf "$LOCK"
                continue
            fi
            what="held by $owner (age ${a}s)"
        else
            what="free, with ${first#*-} ahead of you in line"
        fi
        if [ "$waited" -ge "$max" ]; then
            echo "still $what; run acquire again within ${QUEUE_GRACE}s to keep your place in line"
            exit 3
        fi
        [ "$waited" -eq 0 ] && echo "waiting: $what"
        sleep 2
        waited=$((waited + 2))
    done
    ;;
release)
    owner=$(cat "$LOCK/owner" 2>/dev/null)
    if [ "$owner" = "$name" ]; then
        rm -rf "$LOCK"
        echo "lock released by $name"
    else
        echo "not releasing: lock held by '${owner:-nobody}', not $name"
        exit 1
    fi
    ;;
status)
    if [ -d "$LOCK" ]; then
        echo "held by $(cat "$LOCK/owner" 2>/dev/null) for $(age)s"
    else
        echo "free"
    fi
    [ -d "$QUEUE" ] && ls "$QUEUE" | sort | sed 's/^[0-9]*-/  waiting: /'
    running && echo "a rust-dos job is running" || echo "no rust-dos job running"
    ;;
*)
    echo "usage: acquire NAME [max_wait_s] | release NAME | status"
    exit 2
    ;;
esac
