#!/usr/bin/env bash
# Tests that tools/run-lock.sh stops the Windows processes its jobs start:
# a child left running when the job ends or is signalled, and the reap
# subcommand, which stops emulators no job holds a token for. Needs Git
# Bash on Windows with powershell.exe; elsewhere it skips.
#
#   bash tools/run-lock-tree-test.sh [SCRATCH_DIR]
#
# The probe is a copy of PING.EXE named rust-dos-rltest-<pid>.exe. Every
# check looks at that image name only, and reap runs with a pattern that
# matches only the probe, so no other process is touched. Exits 1 if a
# check failed.

set -u
here=$(cd "$(dirname "$0")" && pwd)
work=${1:-$(mktemp -d)}/run-lock-tree-test.$$
mkdir -p "$work/m"

if ! command -v powershell.exe > /dev/null 2>&1 || [ ! -r "/proc/$$/winpid" ]; then
    echo "skipped: needs Git Bash on Windows with powershell.exe"
    exit 0
fi

export RUST_DOS_RUN_LOCK_DIR=$work/lock
export RUST_DOS_RUN_LOCK_POLL=0.1
export RUST_DOS_RUN_LOCK_REAP_PATTERN='rust-dos-rltest-*.exe'
unset RUST_DOS_RUN_LOCK_INSIDE
HEAVY=$RUST_DOS_RUN_LOCK_DIR/heavy

img=rust-dos-rltest-$$.exe
probe=$work/m/$img
cp /c/Windows/System32/PING.EXE "$probe" || exit 1

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

# Waits up to 15 s for a condition.
until_true() {
    local i=0
    while ! eval "$1"; do
        i=$((i + 1))
        [ "$i" -lt 300 ] || return 1
        sleep 0.05
    done
}

probe_running() { tasklist //FI "IMAGENAME eq $img" //NH 2> /dev/null | tr -d "\r" | grep -qi "^$img"; }
probe_gone() { ! probe_running; }

# Starts the probe under a job: JOB_CMD is run by sh, and the probe runs
# detached from it, the way a benchmark script starts an emulator.
probe_job() { # SUFFIX
    echo "\"$probe\" -n 300 127.0.0.1 > /dev/null 2>&1 & $1"
}

# Stops the probes left by this test, by PID, once the checks are done.
cleanup_probes() {
    tasklist //FI "IMAGENAME eq $img" //FO CSV //NH 2> /dev/null |
        tr -d '\r' | grep -i "^\"$img\"" | cut -d, -f2 | tr -d '"' |
        while read -r p; do
            taskkill //F //PID "$p" < /dev/null > /dev/null 2>&1
        done
}

cd "$work/m" || exit 1

# 1. A job signalled with TERM leaves no child running.
rlb heavy t-sig 0 -- sh -c "$(probe_job 'sleep 1000')" > /dev/null 2>&1 &
s=$!
until_true probe_running
check "the probe runs under the job" 'probe_running'
kill -TERM "$s"
wait "$s"
check "the signalled script exits with 143" '[ $? -eq 143 ]'
check "TERM to the job stops the probe" 'probe_gone'

# 2. A job that ends on its own leaves no child running.
rl heavy t-end 0 -- sh -c "$(probe_job 'true')" > /dev/null 2>&1
check "the job exits 0" '[ $? -eq 0 ]'
check "a job that ends stops the probe it left" 'probe_gone'

# 3. A probe under a live job is owned: reap keeps it and status shows it.
rlb heavy t-keep 0 -- sh -c "$(probe_job 'exec sleep 1000')" > /dev/null 2>&1 &
s=$!
until_true probe_running
rl status > status.txt 2>&1
check "status lists the probe with its job" 'grep -q "emulator: $img .*job t-keep" status.txt'
rl reap > /dev/null 2>&1
check "reap keeps the probe of a live job" 'probe_running'
# 4. Once the job is gone (its script and shell killed, so no cleanup
#    ran), the probe has no token: reap stops it.
set -- "$HEAVY"/t-keep.*
jobpid=$(head -1 "$1" | awk '{print $4}')
kill -9 "$s" 2> /dev/null
wait "$s" 2> /dev/null
kill -9 "$jobpid" 2> /dev/null
rl reap > reap.txt 2>&1
check "reap stops a probe whose job no longer holds a token" 'probe_gone'
check "reap says which emulator it stopped" 'grep -q "stopping $img" reap.txt'

# 5. A probe started outside any job is an orphan: reap stops it.
"$probe" -n 300 127.0.0.1 > /dev/null 2>&1 &
until_true probe_running
rl reap > /dev/null 2>&1
check "reap stops a probe no job owns" 'probe_gone'

cleanup_probes
cd / || exit 1
rm -rf "$work"
echo "$passed passed, $failed failed on $(uname -s), bash $BASH_VERSION"
[ "$failed" -eq 0 ]
