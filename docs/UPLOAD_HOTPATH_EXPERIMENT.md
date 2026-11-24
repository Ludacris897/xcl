# Texture upload hot-path experiment

This is a **draft** experiment based on world-streaming logs; it does not claim
that the current game crash at `game_scarlett_master.exe+0x23CF683` is fixed.
All changes are isolated from the working `fixes-wip` and performance baseline
`experiment/xg-copy-validation-20261123` branches.

## Two independent optimizations

1. **Fence and event reuse (always enabled on this branch).** The old
   `texture::wait` allocated a D3D12 fence and a Win32 event for each batch.
   A bounded four-entry, per-thread cache now reuses both objects, with an
   increasing completion target per queue. GPU waits are still performed for
   every upload, so staging buffers remain alive until their copy finishes.
   Device and queue COM references are retained while cached, avoiding an
   address-reuse ambiguity. Thread exit/cache eviction closes events.
2. **Shared staging buffer (default OFF).** With
   `XCL_COALESCE_UPLOADS=1`, the batch allocator copies multiple textures
   into one 512-byte-aligned upload heap allocation and submits the same
   resource barriers/copies. Without the flag, the original per-texture
   upload-buffer implementation runs. A batch exceeding 512 MiB of
   **D3D12-aligned** staging data, or an unallocatable shared buffer, falls
   back to the original implementation. Individual subresources are checked
   before copying and remain individually accounted for in error reporting.

The new logger reports `XCL PERF texture fence waits` and
`XCL PERF coalesced upload`, alongside existing detile/GPU-batch timing.
All diagnostics require `XCL_PERF=1`.

## Controlled comparison

Each of the following is a **single PowerShell line**. Run one trial at a
time, saving `xcl.log` and `report.json` separately after each run.

**Build the draft hot-path branch**:

```powershell
cd D:\Dev\xcl; git fetch https://github.com/Ludacris897/xcl.git experiment/streaming-hotpath-20261123; git switch --detach FETCH_HEAD; cargo test -p xcl --lib --locked -- --test-threads=1; if($LASTEXITCODE -ne 0){throw 'Tests failed'}; cargo build --release --workspace --locked
```

**Control: old staging-buffer path, new cached fence/events**:

```powershell
$env:XCL_PERF='1'; $env:XCL_ACTIVITY='1'; $env:XCL_WATCH_GAP_KIB='1024'; $env:XCL_TEXTURE_BATCH_MIB='256'; Remove-Item Env:XCL_COALESCE_UPLOADS -ErrorAction SilentlyContinue; .\target\release\xcl.exe run 'D:\Games\GTA VI\GTA6.Xbox.v1.0.2.1'
```

**Experiment: same build, enable coalescing**:

```powershell
$env:XCL_COALESCE_UPLOADS='1'; .\target\release\xcl.exe run 'D:\Games\GTA VI\GTA6.Xbox.v1.0.2.1'
```

**Analyze the current log (with adjacent report.json)**:

```powershell
python tools/analyze-xcl-perf.py "$env:LOCALAPPDATA\XCL\RockstarGamesInc.28872443D20A7\logs\xcl.log" --json
```

Compare matched runs on the same driver and caching state. Use the same
`XCL_WATCH_GAP_KIB` and `XCL_TEXTURE_BATCH_MIB` values. Loading-screen
present rates are **not** equivalent to in-world FPS. Judge results by
detile time, GPU batch+wait, staging allocation counts, visual correctness,
crash offset, loading progress, and regression tests.

## Known limitations

- GPU command-list allocators are **still** created per batch; the new cache
  removes only fence/event churn.
- The coalesced path hasn't been validated on the user's RTX 3090.
- A device removal, GPU timeout, or driver-specific layout incompatibility
  may still occur. Revert the flag first to isolate it; then revert the branch.
- BCPack decoding is a separate experimental PR. Do **not** enable it during
  performance A/B tests until the upload changes are independently evaluated.
