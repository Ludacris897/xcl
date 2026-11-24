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
WRITE_WATCH = re.compile(
    r"XCL PERF GetWriteWatch: (\d+) calls, (\d+) GiB of ranges scanned "
    r"cumulatively, slowest call (\d+) us \((\d+) KiB region\), "
    r"(\d+) failed calls, gap merge threshold (\d+) KiB"
)



def summarize(text):
    periods = []
    sparse_last = None
    texture_last = None
    watch_last = None
    crash = None
    for line in text.splitlines():
        if match := ACTIVITY.search(line):
            seconds, *counts = map(int, match.groups())
            periods.append((seconds, *counts))
        if match := SPARSE.search(line):
            sparse_last = list(map(int, match.groups()))
        if match := TEXTURE.search(line):
            texture_last = list(map(int, match.groups()))
        if match := WRITE_WATCH.search(line):
            watch_last = list(map(int, match.groups()))
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
    if watch_last:
        result["get_write_watch_cumulative"] = dict(zip(
            ("calls", "scanned_gib", "slowest_call_us", "slowest_region_kib", "failures", "gap_kib"),
            watch_last,
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


def summarize_report(report):
    """Compact end-of-run counters; diagnostics and filenames remain local."""
    d3d = report.get("d3d12", {})
    bc = report.get("dstorage", {}).get("bcpack_refused", {})
    shaders = report.get("shaders", {})
    out = {
        "crash": report.get("crash", "").split(" rax=")[0],
        "presents": d3d.get("presents"),
        "failed_pipelines": d3d.get("failed_pipelines"),
        "translated_pipelines": d3d.get("translated_pipelines"),
        "background_pipelines": d3d.get("background_pipelines"),
        "unique_shaders": shaders.get("unique"),
        "dxil_cache": d3d.get("dxil_cache"),
        "bcpack_refused": {
            mode: {
                "requests": info.get("requests", 0),
                "destination_mib": round(info.get("destination_bytes", 0) / (1 << 20), 2),
            } for mode, info in sorted(bc.items())
        },
        "console_stub_calls": {
            entry.get("function", ""): entry.get("calls", 0)
            for entry in report.get("called_stubs", [])
        },
    }
    return out


def load_report(log_path, explicitly):
    path = explicitly if explicitly is not None else log_path.with_name("report.json")
    if not path.is_file():
        if explicitly is not None:
            raise FileNotFoundError(path)
        return None
    return summarize_report(json.loads(path.read_text(encoding="utf-8")))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path, help="xcl.log from one run")
    parser.add_argument("--json", action="store_true", help="print JSON instead of a short report")
    parser.add_argument("--report", type=Path, help="report.json (default: alongside the log)")
    args = parser.parse_args()
    result = summarize(args.log.read_text(encoding="utf-8", errors="replace"))
    report = load_report(args.log, args.report)
    if report:
        result["end_of_run_report"] = report
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
    if "get_write_watch_cumulative" in result:
        w = result["get_write_watch_cumulative"]
        print(f"GetWriteWatch: {w['calls']} calls; slowest {w['slowest_call_us']} us on {w['slowest_region_kib']} KiB; {w['failures']} failed calls; gap {w['gap_kib']} KiB")
    if report:
        print(f"End-of-run report: {report['presents']} presents; {report['translated_pipelines']} translated / {report['failed_pipelines']} failed pipelines; {report['unique_shaders']} unique shaders")
        print("BCPack refused:", report["bcpack_refused"])
        print("Top console stubs:", sorted(report["console_stub_calls"].items(), key=lambda x: x[1], reverse=True)[:5])
    if result["crash"]:
        print("Crash:", result["crash"])
    elif report and report["crash"]:
        print("Crash:", report["crash"])


if __name__ == "__main__":
    main()
