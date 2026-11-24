# Performance-branch run comparison: new access violation at +0x4E598AE

These numbers come from two user-provided `xcl.log` and `report.json`
pairs, recorded before the *experimental* hotpath branch. The reports
do not include a Git commit hash, so the precise executable revision cannot
be independently established from these artifacts alone.

| Metric | Previous log | Latest log |
|---|---:|---:|
| Crash time | 337.785 s | 391.818 s |
| Crash module+offset | `game_scarlett_master.exe+0x23CF683` | `game_scarlett_master.exe+0x4E598AE` |
| Fault | write to null (engine's intentional failure path) | read at address `0x57` (likely invalid object pointer) |
| D3D12 presented frames | 1,784 | 3,580 |
| Translated pipelines | 343 | 367 |
| Failed pipelines | 8 | 8 |
| Deferred/background pipelines: built/failed/pending | 343 / 0 / 58,413 | 367 / 0 / 58,389 |
| BCPack refusals | 2,378 | 2,401 |
| BC1 refusals | 2,070 | 2,100 |

At the 4,096-refresh checkpoint:

| Profiler | Earlier | Latest |
|---|---:|---:|
| Refresh cumulative ms | 140,755 | 23,768 |
| CPU source-read+detile cumulative ms | 123,802 | 14,492 |
| Write-watch cumulative ms | 6,639 | 5,816 |
| GPU batch+wait cumulative ms | 8,893 | 2,798 |
| Amount staged | 14,016 MiB | 4,822 MiB |

**Important:** this is not an equal-work comparison: staged texture volume
differs by 2.9×. Thus the 5.9× cumulative-refresh reduction is not a
normalized FPS, bandwidth, or per-byte speedup.

By the 8,192-refresh checkpoint of the new run:
- CPU source-read+detile: **28,286 ms** total (includes snapshot and detile)
- Write-watch: **12,138 ms**
- GPU batch+wait: **5,073 ms**
- GetWriteWatch: **510,822 calls**, **11,767 GiB** cumulative address ranges
  queried; slowest observed **1,558 us** on a 268,736 KiB region; **0** failures
- Staged GPU texture data: **9,618 MiB**

Near the latest crash:
- about **40.4 GiB?** No: XCL logs **40,357 MB committed** (units as logged);
  local GPU video-memory usage **13,842 MB**, nonlocal **4,648 MB**
- a `Wait64BitValueX` wait times out **at 390.619s** (before the crash),
  requesting value `0xe00` while memory contains zero
- the crash itself occurs at **391.818s**; another wait timeout follows at
  **392.631s**
- both XG `CopyFromSubresource` calls have plausible corrected pitches,
  without a logged refusal in this run

## Investigation priorities

1. Disassemble `+0x4E598AE`, `+0x4E5985E`, and `+0x4E5969E`.
   The register dump includes `RDI=0`, but without the faulting instruction
   the object being dereferenced is not yet identified.
2. Inspect the `Wait64BitValueX` producer/waiter contract to determine
   whether the zero value is an unsupported GPU signal or a symptom of the
   failing worker.
3. Split the 28,286 ms CPU 'detile' counter into **source memory snapshot**
   and **actual swizzle/detile** phases (instrumented on the hotpath branch).
4. Measure memory retained by the ~58,389 deferred shader jobs instead of
   treating it as 58k separate driver compiles (instrumented on hotpath).
5. Test different `XCL_WATCH_GAP_KIB` values *on the already-built
   performance branch* first, before enabling fence or coalesced staging changes.

The standalone BC1 decoder is on a separate experimental branch and remains
opt-in. It must be tested for actual image correctness; an improvement in the
refused counter alone is insufficient.
