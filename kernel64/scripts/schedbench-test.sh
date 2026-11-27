#!/bin/sh
# schedbench-test.sh - the threads and scheduling benchmark (K67, K95;
# journal entry 116), run on 1, 2 and 4 CPUs.
#
#   usage: schedbench-test.sh IMAGE DISK LOG SCHEDBENCH
#
# SCHEDBENCH is schedbench.c (next to this script) built for x86_64. Each
# boot runs `with programs schedbench all N`. This checks that every
# measurement ran and that every program ended cleanly - never how fast
# they were: a busy host or an emulated CPU is slower, and that is not a
# failure. The numbers go to LOG.summary, a table for the journal, and
# are printed at the end.

set -u
IMAGE=$1
DISK=$2
LOG=$3
SCHEDBENCH=$4
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}

fail() {
    echo "FAIL: $1 (log: $LOG.$cpus)"
    exit 1
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

SUMMARY="$LOG.summary"
: > "$SUMMARY"
for cpus in 1 2 4; do
    rm -f "$DISK"
    "$TFSTOOL" mkfs "$DISK" 1024 > /dev/null && truncate -s 4M "$DISK" || fail "cannot make the disk"
    "$TFSTOOL" put "$DISK" schedbench "$SCHEDBENCH" > /dev/null || fail "cannot put schedbench on the disk"

    # ~~ waits up to 90 s for the line just entered: the benchmark takes a
    # few seconds under KVM, and several times that emulated.
    steps="$(keys 'echo == bench ==') ret ~ $(keys "with programs schedbench all $cpus") ret ~~ ~~ ~~"
    steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"

    echo "Booting x86_64 with $cpus CPU(s) for the threads and scheduling benchmark (log: $LOG.$cpus)..."
    DISK="$DISK" SMP=$cpus KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG.$cpus" 40

    tr -d '\r' < "$LOG.$cpus" > "$LOG.$cpus.text"
    T="$LOG.$cpus.text"
    grep -aq '^== end ==$' "$T" || fail "the steps did not all run"
    for what in "on $cpus CPUs" "yield [0-9]+ ns each" "handoff [0-9]+ ns one way" "create [0-9]+ us each" \
        "spawn [0-9]+ us each" "wake median [0-9]+ us, 99% [0-9]+ us, worst [0-9]+ us" "scale 1 thread [0-9]+ ms" \
        "scale 8 threads [0-9]+ ms" "fair done" "nice done" "pinned (skipped|2 threads)" \
        "loaded wake, [0-9]+ busy at nice 0: median" "loaded wake, [0-9]+ busy at nice 10: median" "done in [0-9]+ ms"; do
        grep -aEq "schedbench: $what" "$T" || fail "no \"schedbench: $what\""
    done
    [ "$(grep -acE 'schedbench: busy 2000 ms, [0-9]+ ms cpu' "$T")" -eq 8 ] || fail "not all eight busy programs (fair, nice) reported"
    grep -aEq 'schedbench: (create|spawn) failed' "$T" && fail "a thread or a program could not be started"
    grep -aq '"schedbench" exited with code 0' "$T" || fail "schedbench did not exit with code 0"
    grep -aE '"schedbench" (killed|ended by signal)' "$T" && fail "a schedbench program was killed"
    grep -aiE 'panic|page fault in kernel' "$T" && fail "the kernel failed"

    # Priorities and affinity (K20, journal entry 124) as behaviour, with
    # generous limits: on one or two CPUs the two programs at nice 0 keep
    # them, so the two at nice 10 get under a quarter of what they get;
    # pinned to one CPU, two busy threads get at most three quarters of
    # what they get free. (On four CPUs all four busy programs run.)
    nice_section=$(sed -n '/schedbench: fair done/,/schedbench: nice done/p' "$T")
    nice0=$(echo "$nice_section" | grep -aEo 'busy 2000 ms, [0-9]+ ms cpu at nice 0' | awk '{s += $4} END {print s + 0}')
    nice10=$(echo "$nice_section" | grep -aEo 'busy 2000 ms, [0-9]+ ms cpu at nice 10' | awk '{s += $4} END {print s + 0}')
    if [ $cpus -le 2 ]; then
        [ $((nice10 * 4)) -lt "$nice0" ] || fail "at nice 10 two programs got $nice10 ms against $nice0 at nice 0: priorities did not hold"
    fi
    if [ $cpus -ge 2 ]; then
        pinned=$(grep -aEo 'pinned 2 threads to CPU 0: [0-9]+ ms cpu in 1000 ms; free: [0-9]+' "$T" | grep -Eo '[0-9]+' | sed -n 3p)
        free=$(grep -aEo 'pinned 2 threads to CPU 0: [0-9]+ ms cpu in 1000 ms; free: [0-9]+' "$T" | grep -Eo '[0-9]+' | sed -n 5p)
        [ $((pinned * 4)) -le $((free * 3)) ] || fail "pinned to CPU 0 two threads got $pinned ms, free $free ms: the affinity did not hold"
    fi

    # One column of the table: this boot's numbers.
    v() { grep -aEo "schedbench: $1" "$T" | head -1 | grep -Eo "[0-9]+(\.[0-9]+)?" | sed -n "${2:-1}p"; }
    cpu_shares=$(grep -aEo 'busy 2000 ms, [0-9]+ ms cpu' "$T" | awk '{print $4}' | tr '\n' ' ' | sed 's/ $//')
    {
        echo "cpus=$cpus"
        echo "yield_ns=$(v 'yield [0-9]+')"
        echo "handoff_ns=$(v 'handoff [0-9]+')"
        echo "create_us=$(v 'create [0-9]+')"
        echo "spawn_us=$(v 'spawn [0-9]+')"
        echo "wake_us=$(v 'wake median [0-9]+ us, 99% [0-9]+ us, worst [0-9]+' 1)/$(v 'wake median [0-9]+ us, 99% [0-9]+ us, worst [0-9]+' 3)/$(v 'wake median [0-9]+ us, 99% [0-9]+ us, worst [0-9]+' 4)"
        echo "scale1_ms=$(v 'scale 1 thread [0-9]+' 2)"
        echo "scale4=$(grep -aEo 'scale 4 threads [0-9]+ ms, [0-9]+\.[0-9]+ times' "$T" | grep -Eo '[0-9]+\.[0-9]+')"
        echo "scale8=$(grep -aEo 'scale 8 threads [0-9]+ ms, [0-9]+\.[0-9]+ times' "$T" | grep -Eo '[0-9]+\.[0-9]+')"
        echo "fair_cpu_ms=$(echo "$cpu_shares" | cut -d' ' -f1-4)"
        echo "nice_cpu_ms=$(echo "$nice_section" | grep -aEo 'busy 2000 ms, [0-9]+ ms cpu at nice [0-9]+' | awk '{printf "%s%s(%s) ", sep, $4, $9}' | sed 's/ $//')"
        echo "pinned_ms=$(grep -aEo 'pinned 2 threads to CPU 0: [0-9]+ ms cpu in 1000 ms; free: [0-9]+' "$T" | grep -Eo '[0-9]+' | awk 'NR==3{p=$1} NR==5{f=$1} END {print (p == "" ? "-" : p "/" f)}')"
        echo "loaded0=$(grep -aEo 'loaded wake, [0-9]+ busy at nice 0: median [0-9]+ us, worst [0-9]+' "$T" | grep -Eo '[0-9]+' | awk 'NR==3{m=$1} NR==4{w=$1} END {print m "/" w}')"
        echo "loaded10=$(grep -aEo 'loaded wake, [0-9]+ busy at nice 10: median [0-9]+ us, worst [0-9]+' "$T" | grep -Eo '[0-9]+' | awk 'NR==3{m=$1} NR==4{w=$1} END {print m "/" w}')"
    } >> "$SUMMARY"
done

# The table, one column per number of CPUs.
python3 -I - "$SUMMARY" "${TAAJ_ACCEL:-tcg}" <<'EOF'
import sys
runs, run = [], {}
for line in open(sys.argv[1]):
    key, _, value = line.strip().partition("=")
    if key == "cpus" and run:
        runs.append(run); run = {}
    run[key] = value
runs.append(run)
rows = [("CPUs", "cpus", ""), ("yield (a switch)", "yield_ns", " ns"), ("hand-off, one way", "handoff_ns", " ns"),
        ("thread made and joined", "create_us", " us"), ("program started and waited for", "spawn_us", " us"),
        ("wake-to-run: median/99%/worst", "wake_us", " us"), ("arithmetic, 1 thread", "scale1_ms", " ms"),
        ("... on 4 threads: speed-up", "scale4", "x"), ("... on 8 threads: speed-up", "scale8", "x"),
        ("4 busy programs, 2 s: CPU each", "fair_cpu_ms", " ms"),
        ("same, two at nice 10: CPU each (nice)", "nice_cpu_ms", " ms"),
        ("2 threads pinned to CPU 0 / free: CPU", "pinned_ms", " ms"),
        ("wake, every CPU busy at nice 0: median/worst", "loaded0", " us"),
        ("wake, every CPU busy at nice 10: median/worst", "loaded10", " us")]
print(f"schedbench ({sys.argv[2]}):")
print("| " + " | ".join(r[0] for r in rows[:1]) + " | " + " | ".join(f"{r['cpus']}" for r in runs) + " |")
print("|---|" + "---|" * len(runs))
for name, key, unit in rows[1:]:
    print(f"| {name} | " + " | ".join(f"{r.get(key, '?')}{unit}" for r in runs) + " |")
EOF
echo "the threads and scheduling benchmark ran on 1, 2 and 4 CPUs; every measurement printed, every program ended cleanly"
