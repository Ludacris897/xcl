# XCL world-loading performance experiments

The console runtime has several **distinct** performance stages: CPU shader
translation, DXC compilation, texture detiling, GPU uploads/waits, write-watch
and sparse VRAM mapping, and DirectStorage. Never infer gameplay FPS from a
loading-screen average.

## Opt-in telemetry

Set `XCL_PERF=1` and `XCL_ACTIVITY=1` before starting a run. Every 64,
128, 256… texture refreshes, the runtime emits cumulative milliseconds in:

- overall refresh (includes waiting on XCL's refresh lock)
- refresh lock wait
- write-watch scan
- choosing changed textures
- CPU detiling into linear texture staging
- GPU batch preparation, execution, and synchronous fence wait
- number of GPU batches and MiB staged

Sparse VRAM already reports write-watch/lock/scan/upload timing separately.
Use `python tools/analyze-xcl-perf.py path/to/xcl.log --json` to get a
structured summary. The last reported counter is a cumulative sample, not
the time of one frame.

## Safe experiment: upload batch budget

`XCL_TEXTURE_BATCH_MIB` controls upload batch size (integer 16..512 MiB).
The **default stays 256 MiB** and invalid settings fall back to it. Larger
batches can amortize GPU waits but can also cause a big staging allocation or
more commit pressure. Smaller batches do the reverse.

On one machine, compare:

| Trial | XCL_TEXTURE_BATCH_MIB | Purpose |
|---|---:|---|
| A | 128 | Lower memory pressure |
| B | 256 | Control: original default |
| C | 512 | Fewer, larger uploads |

Use the exact same source revision and driver; repeat each trial, and distinguish
the cold-cache run from warm-cache runs. Keep `XCL_LAZY_PSO` at its default and
avoid clearing shader caches between just these trials. Record time to the
same loading milestone or failure offset; no screenshots or asset dumps are
needed for an initial comparison.

```powershell
$env:XCL_ACTIVITY='1'
$env:XCL_PERF='1'
$env:XCL_TEXTURE_BATCH_MIB='256'
.\target\release\xcl.exe run 'D:\Games\GTA VI\GTA6.Xbox.v1.0.2.1'
python tools/analyze-xcl-perf.py "$env:LOCALAPPDATA\XCL\RockstarGamesInc.28872443D20A7\logs\xcl.log"
Remove-Item Env:XCL_ACTIVITY,Env:XCL_PERF,Env:XCL_TEXTURE_BATCH_MIB -ErrorAction SilentlyContinue
```

## Interpret before optimizing further

- **GPU batch+wait dominates**: inspect fence scheduling and staging lifetimes.
  Do not remove waits blindly; transient resource lifetimes must remain correct.
- **CPU detiling dominates**: profile per-format and mip-level detiling, and
  evaluate partial-mip uploads rather than detiling every mip again.
- **Write-watch/sparse upload dominates**: examine dirty-region merging and
  sparse tile copy scheduling. The sparse timing counters are already available.
- **Shader/PSO stalls dominate**: profile compilation and the number of
  actually-used shader pipelines. Lazy pipelines can hide graphics compilation
  latency but compute pipelines still wait for correctness.
- **DirectStorage failures dominate**: this is a compatibility problem, not
  just throughput. BCPack BC1 experimentation is maintained on a separate,
  opt-in branch; BC5, BC6H, BC7 remain unsupported.

The previous crash offset `game_scarlett_master.exe+0x23CF683` was a deliberate
fatal path. Moving past it is useful but only a **correctly rendered world**
demonstrates progress toward playability.
