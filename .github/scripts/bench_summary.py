"""Prints Criterion's results in target/criterion as a Markdown table.

Used by .github/workflows/benchmarks.yml for the job summary. A benchmark
with a "base" baseline and a "change" estimate gets the base time and the
change; one without them gets this run's time only.
"""

import json
import re
import sys
from pathlib import Path

root = Path(sys.argv[1] if len(sys.argv) > 1 else "target/criterion")


def load(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def typical(estimates):
    # What Criterion prints as the time: the slope when it fitted one.
    return (estimates.get("slope") or estimates["mean"])["point_estimate"]


def duration(ns):
    for unit, scale in (("s", 1e9), ("ms", 1e6), ("µs", 1e3)):
        if ns >= scale:
            return f"{ns / scale:.3g} {unit}"
    return f"{ns:.3g} ns"


rows = []
for info_path in root.glob("**/new/benchmark.json"):
    bench_dir = info_path.parent.parent
    new = load(bench_dir / "new" / "estimates.json")
    if new is None:
        continue
    base = load(bench_dir / "base" / "estimates.json")
    change = load(bench_dir / "change" / "estimates.json")
    name = load(info_path)["full_id"]
    if base is not None and change is not None:
        mean = change["mean"]
        ci = mean["confidence_interval"]
        delta = (
            f"{mean['point_estimate']:+.1%} "
            f"({ci['lower_bound']:+.1%} to {ci['upper_bound']:+.1%})"
        )
        rows.append((name, duration(typical(base)), duration(typical(new)), delta))
    else:
        rows.append((name, "", duration(typical(new)), ""))

# dos/dir_listing/50 before dos/dir_listing/218.
rows.sort(key=lambda row: [int(p) if p.isdigit() else p for p in re.split(r"(\d+)", row[0])])

if not rows:
    print("No benchmark results found.")
    sys.exit(0)

compared = any(row[1] for row in rows)
if compared:
    print("| Benchmark | Base | PR | Change (95% CI) |")
    print("|---|---|---|---|")
    for row in rows:
        print("| " + " | ".join(row) + " |")
else:
    print("| Benchmark | Time |")
    print("|---|---|")
    for name, _, time, _ in rows:
        print(f"| {name} | {time} |")
