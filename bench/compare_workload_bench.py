#!/usr/bin/env python3
"""Compare LK workload benchmark TSV results for PR performance gates."""

from __future__ import annotations

import argparse
import csv
import math
import os
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class BenchRow:
    workload: str
    lk_ms: float
    lua_ms: float
    ratio: float
    noise: float
    confidence: str
    status: str
    checksum: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, type=Path, help="Base benchmark TSV")
    parser.add_argument("--head", required=True, type=Path, help="Head benchmark TSV")
    parser.add_argument(
        "--max-geomean-regression",
        type=float,
        default=0.10,
        help="Fail when head/base geomean is greater than 1 + this value",
    )
    parser.add_argument(
        "--warn-workload-regression",
        type=float,
        default=0.10,
        help="List workloads whose head/base ratio is greater than 1 + this value",
    )
    return parser.parse_args()


def read_rows(path: Path) -> dict[str, BenchRow]:
    if not path.exists():
        raise SystemExit(f"Benchmark TSV does not exist: {path}")

    rows: dict[str, BenchRow] = {}
    with path.open(newline="", encoding="utf-8") as file:
        reader = csv.DictReader(file, delimiter="\t")
        required = {"workload", "lk_ms", "lua_ms", "ratio", "noise", "confidence", "status", "checksum"}
        missing = required.difference(reader.fieldnames or [])
        if missing:
            raise SystemExit(f"{path} is missing TSV columns: {', '.join(sorted(missing))}")

        for line_no, raw in enumerate(reader, start=2):
            workload = (raw.get("workload") or "").strip()
            if not workload:
                raise SystemExit(f"{path}:{line_no}: empty workload")
            if workload in rows:
                raise SystemExit(f"{path}:{line_no}: duplicate workload {workload}")
            rows[workload] = BenchRow(
                workload=workload,
                lk_ms=parse_float(path, line_no, "lk_ms", raw["lk_ms"]),
                lua_ms=parse_float(path, line_no, "lua_ms", raw["lua_ms"]),
                ratio=parse_float(path, line_no, "ratio", raw["ratio"]),
                noise=parse_float(path, line_no, "noise", raw["noise"], allow_zero=True),
                confidence=raw["confidence"],
                status=raw["status"],
                checksum=raw["checksum"],
            )

    if not rows:
        raise SystemExit(f"{path} contains no benchmark rows")
    return rows


def parse_float(path: Path, line_no: int, name: str, value: str, allow_zero: bool = False) -> float:
    try:
        parsed = float(value)
    except ValueError as error:
        raise SystemExit(f"{path}:{line_no}: invalid {name}: {value}") from error
    if not math.isfinite(parsed) or parsed < 0 or (parsed == 0 and not allow_zero):
        if allow_zero:
            raise SystemExit(f"{path}:{line_no}: {name} must be a non-negative finite number: {value}")
        raise SystemExit(f"{path}:{line_no}: {name} must be a positive finite number: {value}")
    return parsed


def geomean(values: list[float]) -> float:
    return math.exp(sum(math.log(value) for value in values) / len(values))


def format_percent(value: float) -> str:
    return f"{value * 100:.2f}%"


def append_summary(markdown: str) -> None:
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return
    with open(summary_path, "a", encoding="utf-8") as file:
        file.write(markdown)
        file.write("\n")


def build_markdown(
    geomean_change: float,
    max_geomean_change: float,
    compared: list[tuple[str, BenchRow, BenchRow, float]],
    workload_warnings: list[tuple[str, BenchRow, BenchRow, float]],
    missing: list[str],
    added: list[str],
) -> str:
    status = "PASS" if geomean_change <= max_geomean_change and not missing else "FAIL"
    lines = [
        "## LK workload performance gate",
        "",
        f"Result: **{status}**",
        f"Compared workloads: **{len(compared)}**",
        f"Geomean head/base change: **{geomean_change:.4f}x** ({format_percent(geomean_change - 1)})",
        f"Allowed geomean regression: **{max_geomean_change:.4f}x** ({format_percent(max_geomean_change - 1)})",
        "",
    ]

    if missing:
        lines.extend(["### Missing workloads", "", ", ".join(f"`{name}`" for name in missing), ""])

    if added:
        lines.extend(["### New workloads", "", ", ".join(f"`{name}`" for name in added), ""])

    if workload_warnings:
        lines.extend(
            [
                "### Workloads over warning threshold",
                "",
                "| Workload | Base ratio | Head ratio | Head/base | Base noise | Head noise |",
                "|---|---:|---:|---:|---:|---:|",
            ]
        )
        for name, base_row, head_row, change in workload_warnings:
            lines.append(
                "| "
                f"`{name}` | {base_row.ratio:.3f}x | {head_row.ratio:.3f}x | {change:.4f}x | "
                f"{base_row.noise:.3f} | {head_row.noise:.3f} |"
            )
        lines.append("")

    lines.extend(
        [
            "### All compared workloads",
            "",
            "| Workload | Base ratio | Head ratio | Head/base |",
            "|---|---:|---:|---:|",
        ]
    )
    for name, base_row, head_row, change in sorted(compared):
        lines.append(f"| `{name}` | {base_row.ratio:.3f}x | {head_row.ratio:.3f}x | {change:.4f}x |")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    args = parse_args()
    base_rows = read_rows(args.base)
    head_rows = read_rows(args.head)

    missing = sorted(set(base_rows).difference(head_rows))
    added = sorted(set(head_rows).difference(base_rows))
    common = sorted(set(base_rows).intersection(head_rows))
    if not common:
        raise SystemExit("No common workloads to compare")

    compared: list[tuple[str, BenchRow, BenchRow, float]] = []
    changes: list[float] = []
    for name in common:
        base_row = base_rows[name]
        head_row = head_rows[name]
        change = head_row.ratio / base_row.ratio
        compared.append((name, base_row, head_row, change))
        changes.append(change)

    geomean_change = geomean(changes)
    max_geomean_change = 1.0 + args.max_geomean_regression
    warn_workload_change = 1.0 + args.warn_workload_regression
    workload_warnings = [row for row in compared if row[3] > warn_workload_change]

    markdown = build_markdown(
        geomean_change=geomean_change,
        max_geomean_change=max_geomean_change,
        compared=compared,
        workload_warnings=workload_warnings,
        missing=missing,
        added=added,
    )
    print(markdown)
    append_summary(markdown)

    if missing:
        print(f"Missing workloads in head result: {', '.join(missing)}", file=sys.stderr)
        return 1
    if geomean_change > max_geomean_change:
        print(
            f"Performance regression: geomean head/base {geomean_change:.4f}x exceeds "
            f"allowed {max_geomean_change:.4f}x",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
