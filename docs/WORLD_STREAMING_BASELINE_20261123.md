# World-streaming performance baseline (two XCL runs)

**Evidence:** two user-provided `xcl.log`/ `report.json` pairs. Both were
recorded before the new XCL_PERF / XCL_WATCH_GAP_KIB instrumentation. These
are diagnostic observations, not reproducible gameplay FPS measurements.
No game assets, shader binaries or personally identifying paths are included.

| Metric | Earlier run | Later run |
|---|---:|---:|
| Last logged elapsed time | ~371.1 s | ~338.6 s |
| Final crash | `game_scarlett_master.exe+0x23CF683` | same |
| Reported D3D12 presents | 2,548 | 1,950 |
| Unique shaders seen | 19,240 | 19,241 |
| Translated pipelines | 282 | 337 |
| Failed pipelines | 8 | 8 |
| Refused BCPack BC1 requests | 2,224 | 2,069 |
| Refused BCPack BC5 requests | 108 | 107 |
| Refused BCPack BC6H requests | 1 | 1 |
| Refused BCPack BC7 requests | 209 | 196 |
| Write-watch cost at latest sampled checkpoint | 14,493 ms (8,192 submits) | 6,232 ms (4,096 submits) |
| Sparse upload cost at that checkpoint | 390 ms | 213 ms |
| Dirty pages at checkpoint | 1,867,126 | 1,608,012 |

The write-watch counters are **cumulative** and were captured at different
submission counts, so they are not directly comparable as per-frame values.
The shorter run does not prove an FPS improvement.

At the later run's final crash, BC1 refusals accounted for 2,069 of 2,373
total BCPack refusals (~87.2%). That motivates the separate opt-in BC1
decoder branch, but it does not prove that any particular failed request
caused the game's fatal path.

Both runs had the same **deliberate game-side zero-address write** and
the same dominant unimplemented console graphics command-list slots:
`#142`, `#126`, and `#75`. Those slots' ABI and semantics need
investigation, not success stubs. The GPU reports 32-lane waves, unlike
the console's 64-lane programs, with potential correctness implications.

## Next test matrix

1. **New profiler**, default `XCL_WATCH_GAP_KIB=1024`, default 256 MiB
   texture batch, `XCL_PERF=1`, `XCL_ACTIVITY=1`.
2. Same build and options, change **only** `XCL_WATCH_GAP_KIB` to 64.
3. Same build and options, change gap to 0. Compare the number of calls,
   GiB scanned, maximum call duration, total write-watch milliseconds and
   time to the same world-load milestone or crash.
4. Keep best gap, then compare `XCL_TEXTURE_BATCH_MIB` 128, 256, 512.
5. Separately try the **experimental** BC1 decoder branch with the opt-in
   compile flag and runtime environment flag. Do not combine the decoder
   experiment with the first performance comparisons.

Use `python tools/analyze-xcl-perf.py path/to/xcl.log --json` with
`report.json` alongside the log. Cleanly exit each run and preserve
separate log/report pairs; avoid overwriting the baseline.
