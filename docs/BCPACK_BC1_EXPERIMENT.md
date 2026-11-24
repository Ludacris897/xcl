# Opt-in BCPack BC1 / DirectStorage experiment

**This is not a gameplay fix until validated on real request traces.** XCL's
default behavior is unchanged. This branch depends on the experimental
[bcpack-rs](https://github.com/Ahmet-Dedeler/bcpack-rs) clean-room decoder
(MIT OR Apache-2.0; pinned revision `c51ec521cbe6869136f29f5d157342374ef5cf33`).
The crate derives part of its tables from the public MIT-licensed
Microsoft `silicon-rans` work. See the upstream notices.

## Run

```powershell
cargo build --release -p xcl --features bcpack-bc1 --locked
$env:XCL_BCPACK_BC1='1'
$env:XCL_ACTIVITY='1'
$env:XCL_PERF='1'
.\target\release\xcl.exe run 'D:\Games\GTA VI\GTA6.Xbox.v1.0.2.1'
Remove-Item Env:XCL_BCPACK_BC1,Env:XCL_PERF,Env:XCL_ACTIVITY -ErrorAction SilentlyContinue
```

Start with the BC1 experiment **off** (no `XCL_BCPACK_BC1`) on the same
build to establish a baseline. Only after comparing logs enable it.
Never use `XCL_DSTORAGE_BCPACK_FILL=1` during a correctness comparison.
This option produces artificial placeholder texture bytes, not decoded data.

## Supported and rejected

- Only mode **BC1** (mode number 1), no swizzle, destination size a multiple
  of eight bytes, up to 32 MiB output per request.
- File/memory requests and physical-page destinations use the existing XCL
  request machinery. Single-stage BCPack supported; two-stage zlib+BCPack
  requires `IntermediateSize` to be provided.
- BC3, BC5, BC6H, BC7 and swizzles remain refused.
- A failure in entropy decoding, output length, or dependency's unfitted BC1
  endpoint table still returns the original decompression error.
- The output is BC1 **blocks**, not decoded RGBA pixels. Existing Xbox tiled
  layouts must still match the destination contract.

The third-party decoder's BC1 endpoint/index codebooks are fitted rather
than proven bit-for-bit against a hardware oracle. A successful decode is
**not** proof that all texels are correct. Verify on synthetic examples and
on user-owned assets with independent checks before accepting the change.

## Compare

Save each run's `report.json` and `xcl.log`. Compare:
- BC1 success logs, original BCPack refused counters, DirectStorage failures
- first crash offset and time into the run
- actual world rendering and texture corruption, not just loading progress
- `XCL_PERF` CPU detile, write-watch, and GPU-wait timing
- game-side shader/pipeline failures

An FPS increase during loading does not prove an in-world frame-rate gain.
