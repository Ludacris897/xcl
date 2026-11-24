#!/usr/bin/env python3
"""Tiny dependency-free tests of the opt-in performance-log reader."""
import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("xcl_perf", Path(__file__).with_name("analyze-xcl-perf.py"))
perf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(perf)


class PerfLogTests(unittest.TestCase):
    def test_activity_and_cumulative_texture_phases(self):
        log = """xcl: activity (2 s): 4 presents, 10 submits, 30 draws, 5 dispatches, 2 frame-event waits (0 timed out), 3 frame statistics (0 not flipped); DirectStorage 100 requests, 15 MB, 10 failed
xcl: activity (2 s): 0 presents, 1 submits, 0 draws, 13 dispatches, 0 frame-event waits (0 timed out), 0 frame statistics (0 not flipped); DirectStorage 20 requests, 2 MB, 5 failed
xcl: XCL PERF texture refresh: 128 calls, 100 ms total, lock 3 ms, write-watch 8 ms, texture selection 5 ms, CPU detile 24 ms, GPU batch+wait 56 ms (7 batches, 20 MiB staged)
xcl: sparse windows timing: write-watch 7 ms (240 dirty pages), lock wait 2 ms, chunk scan 10 ms, upload 30 ms (12 runs)
"""
        obj = perf.summarize(log)
        self.assertEqual(obj["activity_seconds"], 4)
        self.assertEqual(obj["activity_totals"]["presents"], 4)
        self.assertEqual(obj["activity_totals"]["dstorage_failures"], 15)
        self.assertEqual(obj["activity_present_rate_per_interval"]["intervals_with_zero_presents"], 1)
        self.assertEqual(obj["texture_ranked_hotspots_ms"][0], ("gpu_batch_wait_ms", 56))
        self.assertEqual(obj["sparse_cumulative"]["dirty_pages"], 240)

    def test_watch_query_and_report_summary(self):
        log = ("XCL PERF GetWriteWatch: 64 calls, 12 GiB of ranges scanned cumulatively, "
               "slowest call 40200 us (4194304 KiB region), 2 failed calls, "
               "gap merge threshold 64 KiB")
        obj = perf.summarize(log)
        self.assertEqual(obj["get_write_watch_cumulative"]["slowest_call_us"], 40200)
        self.assertEqual(obj["get_write_watch_cumulative"]["gap_kib"], 64)
        report = {
            "crash": "unhandled exception 0xc0000005 in game.exe+0x20 rax=0x0",
            "d3d12": {"presents": 12, "failed_pipelines": 3, "translated_pipelines": 20},
            "dstorage": {"bcpack_refused": {"BC1": {"requests": 9, "destination_bytes": 1048576}}},
            "shaders": {"unique": 100},
            "called_stubs": [{"function": "GraphicsList::#142", "calls": 10}],
        }
        r = perf.summarize_report(report)
        self.assertEqual(r["bcpack_refused"]["BC1"]["destination_mib"], 1.0)
        self.assertEqual(r["console_stub_calls"]["GraphicsList::#142"], 10)
        self.assertEqual(r["unique_shaders"], 100)
        self.assertNotIn("rax=", r["crash"])

    def test_empty_log_is_well_defined(self):
        obj = perf.summarize("")
        self.assertEqual(obj["activity_intervals"], 0)
        self.assertIsNone(obj["present_rate_over_activity_seconds_not_gameplay_fps"])
        self.assertIsNone(obj["crash"])


if __name__ == "__main__":
    unittest.main()
