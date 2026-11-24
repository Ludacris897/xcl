#!/usr/bin/env python3
"""Summarize XCL runtime activity and optional XCL_PERF=1 telemetry.

This script does not access game files or modify caches. Activity frames are *not*
a gameplay FPS benchmark: world loading can deliberately present no frames.
"""
import argparse
import json
import re
import statistics
from pathlib import Path

ACTIVITY = re.compile(
    r"activity \((\d+) s\): (\d+) presents, (\d+) submits, (\d+) draws, "
    r"(\d+) dispatches, .*?DirectStorage (\d+) requests, (\d+) MB, (\d+) failed"
)
SPARSE = re.compile(
    r"sparse windows timing: write-watch (\d+) ms \((\d+) dirty pages\), "
    r"lock wait (\d+) ms, chunk scan (\d+) ms, upload (\d+) ms \((\d+) runs\)"
)
TEXTURE = re.compile(
    r"XCL PERF texture refresh: (\d+) calls, (\d+) ms total, lock (\d+) ms, "
    r"write-watch (\d+) ms, texture selection (\d+) ms, CPU detile (\d+) ms, "
    r"GPU batch\+wait (\d+) ms \((\d+) batches, (\d+) MiB staged\)"
)
CRASH = re.compile(r"unhandled exception (0x[0-9a-fA-F]+) in ([^ ]+) \(([^)]+)\)")


def summarize(text):
    periods = []
    sparse_last = None
    texture_last = None
    crash = None
    for line in text.splitlines():
        if match := ACTIVITY.search(line):
            seconds, *counts = map(int, match.groups())
            periods.append((seconds, *counts))
        if match := SPARSE.search(line):
            sparse_last = list(map(int, match.groups()))
        if match := TEXTURE.search(line):
            texture_last = list(map(int, match.groups()))
        if match := CRASH.search(line):
            crash = {"code": match.group(1), "module_offset": match.group(2), "operation": match.group(3)}
    totals = [sum(p[i] for p in periods) for i in range(1, 8)]
    seconds = sum(p[0] for p in periods)
    result = {
        "activity_intervals": len(periods),
        "activity_seconds": seconds,
        "activity_totals": dict(zip(("presents", "submits", "draws", "dispatches", "dstorage_requests", "dstorage_megabytes", "dstorage_failures"), totals)),
        "present_rate_over_activity_seconds_not_gameplay_fps": round(totals[0] / seconds, 2) if seconds else None,
        "dstorage_failed_fraction": round(totals[6] / totals[4], 5) if totals[4] else None,
        "crash": crash,
    }
    if periods:
        fps = sorted(p[1] / p[0] for p in periods)
        result["activity_present_rate_per_interval"] = {
            "p50": round(statistics.median(fps), 2),
            "p95": round(fps[min(len(fps) - 1, int(0.95 * len(fps)))], 2),
            "max": round(max(fps), 2),
            "intervals_with_zero_presents": sum(p[1] == 0 for p in periods),
        }
    if sparse_last:
        result["sparse_cumulative"] = dict(zip(
            ("write_watch_ms", "dirty_pages", "lock_wait_ms", "chunk_scan_ms", "upload_ms", "upload_runs"),
            sparse_last,
        ))
    if texture_last:
        labels = ("refresh_calls", "total_ms", "lock_ms", "write_watch_ms", "select_ms", "cpu_detile_ms", "gpu_batch_wait_ms", "batches", "staged_mib")
        result["texture_cumulative"] = dict(zip(labels, texture_last))
        ranked = sorted(
            ((name, result["texture_cumulative"][name]) for name in
             ("lock_ms", "write_watch_ms", "select_ms", "cpu_detile_ms", "gpu_batch_wait_ms")),
            key=lambda pair: pair[1], reverse=True,
        )
        result["texture_ranked_hotspots_ms"] = ranked
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path, help="xcl.log from one run")
    parser.add_argument("--json", action="store_true", help="print JSON instead of a short report")
    args = parser.parse_args()
    result = summarize(args.log.read_text(encoding="utf-8", errors="replace"))
    if args.json:
        print(json.dumps(result, indent=2))
        return
    a = result["activity_totals"]
    print(f"Activity: {result['activity_intervals']} intervals / {result['activity_seconds']} seconds")
    print(f"Presents: {a['presents']} ({result['present_rate_over_activity_seconds_not_gameplay_fps']} per second; NOT in-world FPS)")
    print(f"GPU: {a['submits']} submits / {a['draws']} draws / {a['dispatches']} dispatches")
    print(f"DirectStorage: {a['dstorage_requests']} requests, {a['dstorage_failures']} failures ({result['dstorage_failed_fraction']})")
    if "texture_ranked_hotspots_ms" in result:
        print("XCL_PERF texture hot spots (cumulative milliseconds):")
        for name, ms in result["texture_ranked_hotspots_ms"]:
            print(f"  {name:22s} {ms:>10}")
    else:
        print("No XCL_PERF texture telemetry; set XCL_PERF=1 for the next run.")
    if result["crash"]:
        print("Crash:", result["crash"])


if __name__ == "__main__":
    main()
