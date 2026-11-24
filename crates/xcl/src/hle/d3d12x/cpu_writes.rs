//! Write32/64BitValueBottomOfPipeX into memory the GPU does not see: done by the CPU once the GPU
//! has finished the work before them.
//!
//! A console title marks its GPU progress with bottom-of-pipe writes into its own memory and
//! waits on them (a retail game: plain `VirtualAlloc` memory, outside XCL's unified memory).
//! The PC GPU cannot write there; dropped, the title waited for ever after its second frame.
//! Here each queue gets a fence of XCL's: after a submission whose lists carry such writes the
//! queue signals it, and a worker thread does the writes, in submission order, when the GPU is
//! there – what bottom of pipe means.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::CreateEventW;

/// A bottom-of-pipe write into memory outside unified memory, applied by the CPU once the
/// queue's fence reaches the submission that recorded it.
#[derive(Clone)]
pub(super) enum Write {
    /// Immediate dwords to store starting at `va`.
    Immediate(usize, Vec<u32>),
    /// A 64-bit value resolved by the GPU into a persistently mapped READBACK buffer slot
    /// (`src`), copied atomically to `va` once the queue's fence completes. Holding `_buf` keeps
    /// the mapping alive until the worker finishes.
    ReadbackU64 { va: usize, src: usize, _buf: ID3D12Resource },
}

struct Job {
    device: ID3D12Device,
    fence: ID3D12Fence,
    value: u64,
    writes: Vec<Write>,
}

// SAFETY: the fence is a free-threaded COM object; the writes are plain data
unsafe impl Send for Job {}

/// queue → its fence and the last value signalled
struct QueueFence {
    device: ID3D12Device,
    fence: ID3D12Fence,
    value: u64,
    sender: mpsc::Sender<Job>,
}

unsafe impl Send for QueueFence {}

static FENCES: Mutex<Option<HashMap<usize, QueueFence>>> = Mutex::new(None);

/// A bounded wait interval, not permission to publish incomplete GPU work.
const WAIT_MS: u32 = 10_000;

/// Each queue has its own FIFO worker. A blocked graphics fence must not
/// prevent an independent compute or copy queue from publishing its completion.
fn worker(queue: usize) -> std::io::Result<mpsc::Sender<Job>> {
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::Builder::new().name(format!("xcl-cpu-writes-{queue:x}")).spawn(move || unsafe {
        let event = CreateEventW(None, false, false, None).ok();
        for job in rx {
            loop {
                match crate::gpu::fence::wait(&job.fence, job.value, &job.device, event, WAIT_MS) {
                    Ok(()) => {
                        for w in &job.writes {
                            match w {
                                Write::Immediate(va, words) => write_now(*va, words),
                                Write::ReadbackU64 { va, src, .. } => {
                                    let value = (*(*src as *const AtomicU64)).load(Ordering::Acquire);
                                    write_u64_now(*va, value);
                                }
                            }
                        }
                        break;
                    }
                    Err(crate::gpu::fence::FenceError::Timeout(_)) => {
                        crate::xlog_warn!("D3D12.X: CPU writeback queue {queue:#x}, fence {} still pending; completion values withheld", job.value);
                        // Preserve FIFO and resource ownership while work is in flight.
                    }
                    Err(e) => {
                        crate::xlog_error!("D3D12.X: CPU writeback queue {queue:#x}, fence {}: {e}; completion values withheld", job.value);
                        // On a non-timeout API failure the GPU may still own the
                        // readback resources; retain this job rather than free them.
                        if !matches!(e, crate::gpu::fence::FenceError::Removed(_)) {
                            std::mem::forget(job);
                        }
                        break;
                    }
                }
            }
        }
        if let Some(event) = event { let _ = CloseHandle(event); }
    })?;
    Ok(tx)
}

/// Drop the sender when the title releases its queue. Already enqueued jobs
/// retain their own fence/device/resources until completion.
pub(super) fn forget_queue(queue: usize) {
    if let Some(m) = FENCES.lock().unwrap().as_mut() { m.remove(&queue); }
}

/// The dwords at `va`, if the memory is still there and writable.
unsafe fn write_now(va: usize, words: &[u32]) {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_READWRITE};
    let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
    if VirtualQuery(va as *const c_void, &mut m, std::mem::size_of_val(&m)) == 0 || m.State != MEM_COMMIT {
        return;
    }
    if m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) == 0 || (m.BaseAddress as usize + m.RegionSize) < va + 4 * words.len() {
        return;
    }
    for (i, &w) in words.iter().enumerate() {
        (*((va + 4 * i) as *const AtomicU32)).store(w, Ordering::Release);
    }
}

/// An 8-byte aligned 64-bit value at `va`, if the memory is still there and writable.
unsafe fn write_u64_now(va: usize, value: u64) {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_READWRITE};
    let Some(end) = va.checked_add(8) else { return };
    let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
    if VirtualQuery(va as *const c_void, &mut m, std::mem::size_of_val(&m)) == 0 || m.State != MEM_COMMIT {
        return;
    }
    if m.Protect & PAGE_GUARD != 0 || m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) == 0 || (m.BaseAddress as usize).saturating_add(m.RegionSize) < end {
        return;
    }
    (*(va as *const AtomicU64)).store(value, Ordering::Release);
}

/// Whether `[va, va + bytes)` is memory the CPU can write for the GPU later.
pub(super) fn writable(va: usize, bytes: usize) -> bool {
    crate::hle::kernelx::lazy::ensure(va, bytes);
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_READWRITE};
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let found = unsafe { VirtualQuery(va as *const c_void, &mut m, std::mem::size_of_val(&m)) } != 0;
    found
        && m.State == MEM_COMMIT
        && m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) != 0
        && m.BaseAddress as usize + m.RegionSize >= va + bytes
}

/// `writes` of lists just submitted to `queue`: done when the GPU has finished them.
pub(super) unsafe fn after(queue: *mut c_void, writes: Vec<Write>) {
    if writes.is_empty() || queue.is_null() {
        return;
    }
    let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&queue) else { return };
    {
        let mut g = FENCES.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        let entry = match m.entry(queue as usize) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let mut dev: Option<ID3D12Device> = None;
                if q.GetDevice(&mut dev).is_err() {
                    return;
                }
                let Some(dev) = dev else { return };
                let Ok(fence) = dev.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) else { return };
                let sender = match worker(queue as usize) {
                    Ok(sender) => sender,
                    Err(e) => {
                        crate::xlog_error!("D3D12.X: cannot start CPU writeback worker: {e}");
                        return;
                    }
                };
                v.insert(QueueFence { device: dev, fence, value: 0, sender })
            }
        };
        entry.value += 1;
        if q.Signal(&entry.fence, entry.value).is_err() {
            return;
        }
        let job = Job { device: entry.device.clone(), fence: entry.fence.clone(), value: entry.value, writes };
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| crate::xlog_info!("D3D12.X: bottom-of-pipe writes outside unified memory use per-queue CPU workers after GPU completion ({} in the first submission)", job.writes.len()));
        // Enqueue while the fence sequence is protected so two submitting
        // threads cannot reverse the ordering of their writebacks.
        if let Err(e) = entry.sender.send(job) {
            crate::xlog_error!("D3D12.X: CPU writeback worker disconnected on queue {:#x}", queue as usize);
            std::mem::forget(e.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Dxgi::*;

    /// The value lands after the queue has done what came before it.
    #[test]
    fn writes_land_after_the_queue() {
        unsafe {
            let mut dev: Option<ID3D12Device> = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                let factory: IDXGIFactory4 = CreateDXGIFactory1().unwrap();
                let warp: IDXGIAdapter = factory.EnumWarpAdapter().unwrap();
                D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut dev).expect("WARP D3D12 device");
            }
            let dev = dev.unwrap();
            let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            let cell = Box::new([0u32; 2]);
            let va = cell.as_ptr() as usize;
            assert!(writable(va, 8));
            after(q.as_raw(), vec![Write::Immediate(va, vec![7, 9])]);
            let t = std::time::Instant::now();
            while (*(va as *const AtomicU32)).load(Ordering::Acquire) != 7 {
                assert!(t.elapsed().as_secs() < 5, "never written");
                std::thread::yield_now();
            }
            assert_eq!(*cell, [7, 9]);
        }
    }

    /// A blocked queue must not prevent an independent queue from publishing a completed write.
    #[test]
    fn independent_queue_writeback_is_not_blocked_by_other_queue() {
        unsafe {
            let mut dev: Option<ID3D12Device> = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                let factory: IDXGIFactory4 = CreateDXGIFactory1().unwrap();
                let warp: IDXGIAdapter = factory.EnumWarpAdapter().unwrap();
                D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut dev).expect("WARP D3D12 device");
            }
            let dev = dev.unwrap();
            let desc = D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() };
            let blocked: ID3D12CommandQueue = dev.CreateCommandQueue(&desc).unwrap();
            let ready: ID3D12CommandQueue = dev.CreateCommandQueue(&desc).unwrap();
            let gate: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
            let a = Box::new(AtomicU32::new(0));
            let b = Box::new(AtomicU32::new(0));
            blocked.Wait(&gate, 1).unwrap();
            after(blocked.as_raw(), vec![Write::Immediate(&*a as *const AtomicU32 as usize, vec![17])]);
            after(ready.as_raw(), vec![Write::Immediate(&*b as *const AtomicU32 as usize, vec![23])]);
            let start = std::time::Instant::now();
            while b.load(Ordering::Acquire) == 0 && start.elapsed().as_secs() < 3 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let independent = b.load(Ordering::Acquire) == 23 && a.load(Ordering::Acquire) == 0;
            gate.Signal(1).unwrap();
            let start = std::time::Instant::now();
            while (a.load(Ordering::Acquire) == 0 || b.load(Ordering::Acquire) == 0) && start.elapsed().as_secs() < 5 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let completed = a.load(Ordering::Acquire) == 17 && b.load(Ordering::Acquire) == 23;
            forget_queue(blocked.as_raw() as usize);
            forget_queue(ready.as_raw() as usize);
            // A failed test must not release cells still referenced by an asynchronous job.
            if !completed { std::mem::forget(a); std::mem::forget(b); }
            assert!(completed, "both writebacks must finish after gate signal");
            assert!(independent, "independent queue must publish before blocked queue is released");
        }
    }

    /// A ReadbackU64 write copies the 64-bit value from the mapped readback buffer after the queue finishes.
    #[test]
    fn readback_u64_lands_after_the_queue() {
        unsafe {
            let mut dev: Option<ID3D12Device> = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory4>() else {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                };
                let Ok(warp) = factory.EnumWarpAdapter::<IDXGIAdapter>() else {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                };
                if D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                }
            }
            let dev = dev.unwrap();
            let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            let hp = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
            let rd = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: 8,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            };
            let mut rb: Option<ID3D12Resource> = None;
            dev.CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &rd, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut rb).unwrap();
            let rb = rb.unwrap();
            let mut ptr: *mut c_void = std::ptr::null_mut();
            rb.Map(0, None, Some(&mut ptr)).unwrap();
            let expected = 0x1234_5678_9ABC_DEF0u64;
            (*(ptr as *const AtomicU64)).store(expected, Ordering::Release);

            let dst = Box::new(0u64);
            let va = dst.as_ref() as *const u64 as usize;
            assert!(writable(va, 8));
            after(q.as_raw(), vec![Write::ReadbackU64 { va, src: ptr as usize, _buf: rb }]);
            let t = std::time::Instant::now();
            while (*(va as *const AtomicU64)).load(Ordering::Acquire) != expected {
                assert!(t.elapsed().as_secs() < 5, "readback value never written");
                std::thread::yield_now();
            }
            assert_eq!(*dst, expected);
        }
    }
}
