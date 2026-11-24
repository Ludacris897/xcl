//! Non-blocking pipeline creation. Translating a console shader takes milliseconds; compiling
//! the HLSL with DXC takes 0.2 s to seconds, and a title creates thousands of pipelines while
//! it loads. When a pipeline's DXIL is not in the disk cache yet, the title gets a stand-in
//! pipeline (shaders that draw nothing) at once and the real one is built on a worker; binding
//! (`SetPipelineState`, console list replay, the two-pass lookups) uses the real one as soon
//! as it is there. A graphics pipeline draws nothing until then; a compute pipeline is waited
//! for at its first bind (a skipped dispatch would corrupt what it computes).
//! `XCL_ASYNC_PSO=0` builds every pipeline in the call, as before.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock, RwLock};

struct Pending {
    /// the build, until a worker – or a bind that cannot wait for the queue – takes it
    job: Mutex<Option<Job>>,
    /// the build went to the workers (a lazy one goes on its first `resolve`)
    sent: std::sync::atomic::AtomicBool,
    /// the real pipeline state (raw pointer), 0 while building or when building failed
    real: AtomicUsize,
    done: Mutex<bool>,
    cv: Condvar,
    compute: bool,
    /// the stand-in was released before the real pipeline was built
    orphan: AtomicBool,
    /// Counted in IN_FLIGHT while this stand-in is live and unfinished.
    tracked: AtomicBool,
}

/// stand-in handed to the title → its real pipeline
static PENDING: RwLock<Option<HashMap<usize, Arc<Pending>>>> = RwLock::new(None);
/// Number of still-live stand-ins whose jobs have not completed. Unlike scanning
/// every job and locking its Condvar state, this is O(1) on activity/report paths.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

fn stop_tracking(p: &Pending) {
    if p.tracked.swap(false, Ordering::AcqRel) {
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The build is done (`real`: its pipeline, 0 = failed).
fn complete(p: &Pending, real: usize) {
    if real == 0 {
        FAILED.fetch_add(1, Ordering::Relaxed);
    } else {
        BUILT.fetch_add(1, Ordering::Relaxed);
    }
    let orphan = {
        let mut done = p.done.lock().unwrap();
        let orphan = p.orphan.load(Ordering::Acquire);
        if !orphan {
            p.real.store(real, Ordering::Release);
        }
        *done = true;
        stop_tracking(p);
        p.cv.notify_all();
        orphan
    };
    // outside `done`: the Release override takes PENDING (`counts` takes PENDING, then `done`)
    if orphan {
        release(real);
    }
}
static BUILT: AtomicUsize = AtomicUsize::new(0);
static FAILED: AtomicUsize = AtomicUsize::new(0);

pub fn enabled() -> bool {
    std::env::var("XCL_ASYNC_PSO").map_or(true, |v| v != "0")
}

/// Builds the real pipeline: its raw pointer, 0 = failed.
pub type Job = Box<dyn FnOnce() -> usize + Send>;

fn queue() -> &'static Mutex<mpsc::Sender<Arc<Pending>>> {
    static Q: OnceLock<Mutex<mpsc::Sender<Arc<Pending>>>> = OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Arc<Pending>>();
        let rx = Arc::new(Mutex::new(rx));
        // translation recurses deeply on big programs; DXC itself runs on its own pool
        let n = crate::gpu::recomp::dxc::compile_threads();
        for k in 0..n {
            let rx = rx.clone();
            let _ = std::thread::Builder::new().name(format!("xcl-pso-{k}")).stack_size(64 << 20).spawn(move || loop {
                let next = rx.lock().unwrap().recv();
                let Ok(p) = next else { return };
                // (None: a bind took it and builds it itself)
                let job = p.job.lock().unwrap().take();
                if let Some(job) = job {
                    complete(&p, job());
                }
            });
        }
        Mutex::new(tx)
    })
}

/// Release a pipeline state XCL owns (raw pointer, 0 = none).
pub fn release(p: usize) {
    if p != 0 {
        unsafe { drop(<windows::Win32::Graphics::Direct3D12::ID3D12PipelineState as windows::core::Interface>::from_raw(p as *mut std::ffi::c_void)) };
    }
}

/// `standin` went to the title; `job` builds the real pipeline on a worker.
pub fn submit(standin: usize, compute: bool, job: Job) {
    submit_as(standin, compute, job, true);
}

/// Lazy pipeline creation (`XCL_LAZY_PSO=0`: off): a title's pipelines with console shaders get
/// their stand-in when made and are built when first used – the first bind of a graphics one
/// sends it to the workers, a compute one is built at its first bind. A retail game makes
/// ~51 000 pipelines in its first 30 s and uses few; each one built stays in the driver
/// (~400 KB), and the process ran out of commit.
pub fn lazy() -> bool {
    static L: OnceLock<bool> = OnceLock::new();
    *L.get_or_init(|| enabled() && std::env::var("XCL_LAZY_PSO").map_or(true, |v| v != "0"))
}

/// `submit`, sent to the workers only on its first `resolve` (`lazy`).
pub fn submit_lazy(standin: usize, compute: bool, job: Job) {
    submit_as(standin, compute, job, false);
}

fn submit_as(standin: usize, compute: bool, job: Job, send: bool) {
    let p = Arc::new(Pending {
        job: Mutex::new(Some(job)),
        sent: AtomicBool::new(send),
        real: AtomicUsize::new(0),
        done: Mutex::new(false),
        cv: Condvar::new(),
        compute,
        orphan: Default::default(),
        tracked: AtomicBool::new(true),
    });
    {
        let mut state = PENDING.write().unwrap();
        let map = state.get_or_insert_with(HashMap::new);
        // An address should not be reused before Release; avoid a leaked counter if
        // a title nonetheless submits the same stand-in address twice.
        if let Some(replaced) = map.insert(standin, p.clone()) {
            replaced.orphan.store(true, Ordering::Release);
            stop_tracking(&replaced);
        }
        IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::xlog_info!("D3D12.X: pipelines whose shaders are not compiled yet get a stand-in and are built in the background");
    });
    if send {
        let _ = queue().lock().unwrap().send(p);
    }
}

fn entry(pso: usize) -> Option<Arc<Pending>> {
    PENDING.read().unwrap().as_ref()?.get(&pso).cloned()
}

/// Resolve a previously looked-up pipeline without reacquiring the global map lock.
fn resolve_entry(pso: usize, p: &Arc<Pending>) -> usize {
    match p.real.load(Ordering::Acquire) {
        0 => {
            if !p.sent.swap(true, Ordering::AcqRel) {
                let _ = queue().lock().unwrap().send(Arc::clone(p));
            }
            pso
        }
        r => r,
    }
}

/// Resolve a compute pipeline, compiling it on the calling thread if its lazy
/// job has not started, then waiting for completion if a worker claimed it.
fn resolve_wait_entry(pso: usize, p: &Arc<Pending>) -> usize {
    let job = p.job.lock().unwrap().take();
    if let Some(job) = job {
        complete(p, job());
    }
    let mut done = p.done.lock().unwrap();
    while !*done {
        done = p.cv.wait(done).unwrap();
    }
    match p.real.load(Ordering::Acquire) {
        0 => pso,
        r => r,
    }
}

/// The pipeline to use for `pso` now: the real one once built, else `pso` itself.
pub fn resolve(pso: usize) -> usize {
    let Some(p) = entry(pso) else { return pso };
    resolve_entry(pso, &p)
}

/// `resolve`, waiting for a pipeline still being built.
pub fn resolve_wait(pso: usize) -> usize {
    let Some(p) = entry(pso) else { return pso };
    resolve_wait_entry(pso, &p)
}

/// For binding: compute pipelines are waited for, graphics ones are not.
/// Reuse the same lookup – this previously locked and searched PENDING twice
/// on every bind, including on the render thread's fast path.
pub fn resolve_for_bind(pso: usize) -> usize {
    let Some(p) = entry(pso) else { return pso };
    if p.compute {
        resolve_wait_entry(pso, &p)
    } else {
        resolve_entry(pso, &p)
    }
}

/// The title released stand-in `pso` for good: forget it (its address may come back for
/// another object) and return its real pipeline to release (0: none, or still being built –
/// the worker's result is then released when it arrives).
pub fn released(pso: usize) -> usize {
    let Some(p) = PENDING.write().unwrap().as_mut().and_then(|m| m.remove(&pso)) else { return 0 };
    let done = p.done.lock().unwrap();
    stop_tracking(&p);
    if *done {
        p.real.swap(0, Ordering::AcqRel)
    } else {
        // The worker must discard its result if the title releases the stand-in first.
        p.orphan.store(true, Ordering::Release);
        0
    }
}

/// `pso` is a stand-in whose real pipeline is still being built.
pub fn is_pending(pso: usize) -> bool {
    entry(pso).is_some_and(|p| !*p.done.lock().unwrap())
}

/// (built in the background, failed, still building)
pub fn counts() -> (usize, usize, usize) {
    (BUILT.load(Ordering::Relaxed), FAILED.load(Ordering::Relaxed), IN_FLIGHT.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_compute_job_resolves_and_retires_without_worker() {
        // No real D3D12 pipeline is created. A zero result stays the stand-in.
        let standin = usize::MAX - 0x1000;
        let before = counts().2;
        submit_lazy(standin, true, Box::new(|| 0));
        assert!(is_pending(standin));
        assert_eq!(counts().2, before + 1);
        assert_eq!(resolve_for_bind(standin), standin);
        assert!(!is_pending(standin));
        assert_eq!(counts().2, before);
        assert_eq!(released(standin), 0);
        assert_eq!(counts().2, before);
    }

    #[test]
    fn released_lazy_job_does_not_remain_counted() {
        let standin = usize::MAX - 0x2000;
        let before = counts().2;
        submit_lazy(standin, false, Box::new(|| 0));
        assert_eq!(counts().2, before + 1);
        assert_eq!(released(standin), 0);
        assert_eq!(counts().2, before);
    }
}
