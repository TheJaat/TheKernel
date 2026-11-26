#!/bin/sh
# schedbench-test.sh - the threads and scheduling benchmark (K67, K95;
# journal entry NNN), run on 1, 2 and 4 CPUs.
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
        "scale 8 threads [0-9]+ ms" "fair done" "done in [0-9]+ ms"; do
        grep -aEq "schedbench: $what" "$T" || fail "no \"schedbench: $what\""
    done
    [ "$(grep -acE 'schedbench: busy 2000 ms, [0-9]+ ms cpu' "$T")" -eq 4 ] || fail "not all four busy programs reported"
    grep -aEq 'schedbench: (create|spawn) failed' "$T" && fail "a thread or a program could not be started"
    grep -aq '"schedbench" exited with code 0' "$T" || fail "schedbench did not exit with code 0"
    grep -aE '"schedbench" (killed|ended by signal)' "$T" && fail "a schedbench program was killed"
    grep -aiE 'panic|page fault in kernel' "$T" && fail "the kernel failed"

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
        echo "fair_cpu_ms=$cpu_shares"
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
        ("4 busy programs, 2 s: CPU each", "fair_cpu_ms", " ms")]
print(f"schedbench ({sys.argv[2]}):")
print("| " + " | ".join(r[0] for r in rows[:1]) + " | " + " | ".join(f"{r['cpus']}" for r in runs) + " |")
print("|---|" + "---|" * len(runs))
for name, key, unit in rows[1:]:
    print(f"| {name} | " + " | ".join(f"{r.get(key, '?')}{unit}" for r in runs) + " |")
EOF
echo "the threads and scheduling benchmark ran on 1, 2 and 4 CPUs; every measurement printed, every program ended cleanly"
