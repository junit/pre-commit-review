use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::{sample_process_tree, terminate_linux_tracked};

pub const PRODUCTION_PROCESS_TREE_RSS_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_RESOURCE_SAMPLE_INTERVAL_MS: u64 = 100;
const MAX_TRACKED_PROCESSES: usize = 4_096;
const STATE_AVAILABLE: u8 = 0;
const STATE_LIMIT_EXCEEDED: u8 = 1;
const STATE_UNAVAILABLE: u8 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceAccountingStatus {
    Available,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResourceError {
    pub code: &'static str,
    pub message: String,
}

impl ProviderResourceError {
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: "process-tree-rss-accounting-unavailable",
            message: message.into(),
        }
    }

    #[cfg(feature = "test-fixture")]
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "process-tree-rss-policy-invalid",
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProviderResourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProviderResourceError {}

#[cfg(feature = "test-fixture")]
#[derive(Debug, Clone, Copy)]
enum SamplerMode {
    Platform,
    Unavailable,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderResourcePolicy {
    maximum_rss_bytes: u64,
    sample_interval: Duration,
    #[cfg(feature = "test-fixture")]
    mode: SamplerMode,
}

impl ProviderResourcePolicy {
    pub(crate) fn production() -> Self {
        Self {
            maximum_rss_bytes: PRODUCTION_PROCESS_TREE_RSS_LIMIT_BYTES,
            sample_interval: Duration::from_millis(MAX_RESOURCE_SAMPLE_INTERVAL_MS),
            #[cfg(feature = "test-fixture")]
            mode: SamplerMode::Platform,
        }
    }

    #[cfg(feature = "test-fixture")]
    pub fn for_test(
        maximum_rss_bytes: u64,
        sample_interval: Duration,
    ) -> Result<Self, ProviderResourceError> {
        Self::new(maximum_rss_bytes, sample_interval, SamplerMode::Platform)
    }

    #[cfg(feature = "test-fixture")]
    pub fn unavailable_for_test(sample_interval: Duration) -> Result<Self, ProviderResourceError> {
        Self::new(1, sample_interval, SamplerMode::Unavailable)
    }

    #[cfg(feature = "test-fixture")]
    fn new(
        maximum_rss_bytes: u64,
        sample_interval: Duration,
        mode: SamplerMode,
    ) -> Result<Self, ProviderResourceError> {
        if maximum_rss_bytes == 0 || maximum_rss_bytes > PRODUCTION_PROCESS_TREE_RSS_LIMIT_BYTES {
            return Err(ProviderResourceError::invalid(
                "process-tree RSS limit must be positive and no greater than production",
            ));
        }
        if sample_interval < Duration::from_millis(1)
            || sample_interval > Duration::from_millis(MAX_RESOURCE_SAMPLE_INTERVAL_MS)
        {
            return Err(ProviderResourceError::invalid(
                "process-tree RSS sample interval must be between 1 and 100 milliseconds",
            ));
        }
        Ok(Self {
            maximum_rss_bytes,
            sample_interval,
            mode,
        })
    }

    pub(crate) fn interval_ms(self) -> u64 {
        u64::try_from(self.sample_interval.as_millis()).unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderProcessScope {
    #[cfg(windows)]
    root_pid: u32,
    #[cfg(unix)]
    process_group_id: i32,
    #[cfg(windows)]
    job_handle: isize,
}

impl ProviderProcessScope {
    #[cfg(unix)]
    pub(crate) fn for_unix(_root_pid: u32, process_group_id: i32) -> Self {
        Self { process_group_id }
    }

    #[cfg(windows)]
    pub(crate) fn for_windows(root_pid: u32, job_handle: isize) -> Self {
        Self {
            root_pid,
            job_handle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderResourceSnapshot {
    pub peak_rss_bytes: u64,
    pub sample_interval_ms: u64,
    pub accounting: ResourceAccountingStatus,
    pub limit_exceeded: bool,
}

struct SharedState {
    peak_rss_bytes: AtomicU64,
    state: AtomicU8,
    stop: AtomicBool,
}

struct ProcessTreeSampler {
    scope: ProviderProcessScope,
    #[cfg(target_os = "linux")]
    tracked: std::collections::BTreeMap<u32, linux::TrackedProcess>,
}

impl ProcessTreeSampler {
    fn new(scope: ProviderProcessScope) -> Self {
        Self {
            scope,
            #[cfg(target_os = "linux")]
            tracked: std::collections::BTreeMap::new(),
        }
    }

    fn sample(&mut self) -> Result<SampleResult, ProviderResourceError> {
        sample_process_tree(self)
    }

    fn terminate(&self) -> Result<(), ProviderResourceError> {
        terminate_process_scope(self.scope);
        #[cfg(target_os = "linux")]
        terminate_linux_tracked(&self.tracked)?;
        Ok(())
    }
}

pub(crate) struct ProviderResourceMonitor {
    state: Arc<SharedState>,
    sample_interval_ms: u64,
    thread: Option<JoinHandle<()>>,
}

impl ProviderResourceMonitor {
    pub(crate) fn start(
        scope: ProviderProcessScope,
        policy: ProviderResourcePolicy,
    ) -> Result<Self, ProviderResourceError> {
        #[cfg(feature = "test-fixture")]
        if matches!(policy.mode, SamplerMode::Unavailable) {
            return Err(ProviderResourceError::unavailable(
                "process-tree RSS accounting was disabled by the test policy",
            ));
        }

        let mut sampler = ProcessTreeSampler::new(scope);
        let first = match sampler.sample() {
            Ok(SampleResult::Bytes(bytes)) => bytes,
            Ok(SampleResult::Exited) => {
                return Err(ProviderResourceError::unavailable(
                    "provider exited before process-tree RSS accounting started",
                ))
            }
            Err(error) => return Err(error),
        };
        let limit_exceeded = first > policy.maximum_rss_bytes;
        let state = Arc::new(SharedState {
            peak_rss_bytes: AtomicU64::new(bounded_peak(first, policy.maximum_rss_bytes)),
            state: AtomicU8::new(if limit_exceeded {
                STATE_LIMIT_EXCEEDED
            } else {
                STATE_AVAILABLE
            }),
            stop: AtomicBool::new(false),
        });
        if limit_exceeded {
            sampler.terminate()?;
        }
        let thread_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name("provider-rss-monitor".to_string())
            .spawn(move || {
                let mut next_sample = Instant::now() + policy.sample_interval;
                loop {
                    let now = Instant::now();
                    if next_sample > now {
                        thread::park_timeout(next_sample - now);
                    }
                    if thread_state.stop.load(Ordering::Acquire) {
                        if sampler.terminate().is_err() {
                            thread_state
                                .state
                                .store(STATE_UNAVAILABLE, Ordering::Release);
                        }
                        return;
                    }
                    match sampler.sample() {
                        Ok(SampleResult::Bytes(bytes)) => {
                            update_peak(
                                &thread_state.peak_rss_bytes,
                                bounded_peak(bytes, policy.maximum_rss_bytes),
                            );
                            if bytes <= policy.maximum_rss_bytes {
                                next_sample += policy.sample_interval;
                                continue;
                            }
                            thread_state
                                .state
                                .store(STATE_LIMIT_EXCEEDED, Ordering::Release);
                            if sampler.terminate().is_err() {
                                thread_state
                                    .state
                                    .store(STATE_UNAVAILABLE, Ordering::Release);
                            }
                            return;
                        }
                        Ok(SampleResult::Exited) => return,
                        Err(_) => {
                            thread_state
                                .state
                                .store(STATE_UNAVAILABLE, Ordering::Release);
                            let _ = sampler.terminate();
                            return;
                        }
                    }
                }
            })
            .map_err(|error| {
                ProviderResourceError::unavailable(format!(
                    "cannot start process-tree RSS monitor: {error}"
                ))
            })?;
        Ok(Self {
            state,
            sample_interval_ms: policy.interval_ms(),
            thread: Some(thread),
        })
    }

    pub(crate) fn snapshot(&self) -> ProviderResourceSnapshot {
        let state = self.state.state.load(Ordering::Acquire);
        ProviderResourceSnapshot {
            peak_rss_bytes: self.state.peak_rss_bytes.load(Ordering::Acquire),
            sample_interval_ms: self.sample_interval_ms,
            accounting: if state == STATE_UNAVAILABLE {
                ResourceAccountingStatus::Unavailable
            } else {
                ResourceAccountingStatus::Available
            },
            limit_exceeded: state == STATE_LIMIT_EXCEEDED,
        }
    }

    pub(crate) fn stop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

impl Drop for ProviderResourceMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn bounded_peak(observed: u64, maximum: u64) -> u64 {
    observed.min(maximum.saturating_add(1))
}

fn update_peak(peak: &AtomicU64, observed: u64) {
    let mut current = peak.load(Ordering::Acquire);
    while observed > current {
        match peak.compare_exchange_weak(current, observed, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

enum SampleResult {
    Bytes(u64),
    Exited,
}

#[cfg(unix)]
fn terminate_process_scope(scope: ProviderProcessScope) {
    unsafe {
        libc::killpg(scope.process_group_id, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn terminate_process_scope(scope: ProviderProcessScope) {
    use std::ffi::c_void;
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;

    unsafe {
        TerminateJobObject(scope.job_handle as *mut c_void, 1);
    }
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_scope(_scope: ProviderProcessScope) {}

#[cfg(unix)]
fn unix_process_group_exists(process_group_id: i32) -> Result<bool, ProviderResourceError> {
    if unsafe { libc::killpg(process_group_id, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(ProviderResourceError::unavailable(format!(
            "cannot inspect the provider process group: {error}"
        ))),
    }
}

#[cfg(target_os = "macos")]
fn sample_process_tree(
    sampler: &mut ProcessTreeSampler,
) -> Result<SampleResult, ProviderResourceError> {
    use std::ffi::c_void;
    use std::mem::{size_of, MaybeUninit};

    let scope = sampler.scope;

    const PROC_PGRP_ONLY: u32 = 2;
    const PROC_PIDTASKINFO: i32 = 4;

    #[repr(C)]
    struct ProcTaskInfo {
        virtual_size: u64,
        resident_size: u64,
        total_user: u64,
        total_system: u64,
        threads_user: u64,
        threads_system: u64,
        policy: i32,
        faults: i32,
        pageins: i32,
        cow_faults: i32,
        messages_sent: i32,
        messages_received: i32,
        syscalls_mach: i32,
        syscalls_unix: i32,
        context_switches: i32,
        thread_count: i32,
        running_threads: i32,
        priority: i32,
    }

    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_listpids(
            process_type: u32,
            type_info: u32,
            buffer: *mut c_void,
            buffer_size: i32,
        ) -> i32;
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            argument: u64,
            buffer: *mut c_void,
            buffer_size: i32,
        ) -> i32;
    }

    let required = unsafe {
        proc_listpids(
            PROC_PGRP_ONLY,
            scope.process_group_id as u32,
            std::ptr::null_mut(),
            0,
        )
    };
    if required <= 0 {
        return if unix_process_group_exists(scope.process_group_id)? {
            Err(ProviderResourceError::unavailable(
                "cannot enumerate the macOS provider process group",
            ))
        } else {
            Ok(SampleResult::Exited)
        };
    }
    let pid_size = size_of::<i32>();
    let required_count = usize::try_from(required).unwrap_or(usize::MAX) / pid_size;
    let capacity = required_count
        .saturating_add(32)
        .clamp(32, MAX_TRACKED_PROCESSES);
    let mut pids = vec![0_i32; capacity];
    let bytes = unsafe {
        proc_listpids(
            PROC_PGRP_ONLY,
            scope.process_group_id as u32,
            pids.as_mut_ptr().cast(),
            i32::try_from(pids.len().saturating_mul(pid_size)).unwrap_or(i32::MAX),
        )
    };
    if bytes <= 0 {
        return if unix_process_group_exists(scope.process_group_id)? {
            Err(ProviderResourceError::unavailable(
                "cannot enumerate the macOS provider process group",
            ))
        } else {
            Ok(SampleResult::Exited)
        };
    }
    let count = usize::try_from(bytes).unwrap_or(usize::MAX) / pid_size;
    if count > pids.len() || count >= MAX_TRACKED_PROCESSES {
        return Err(ProviderResourceError::unavailable(
            "process tree exceeds the accounting process limit",
        ));
    }
    let mut total = 0_u64;
    let mut observed = 0_usize;
    for pid in pids.into_iter().take(count).filter(|pid| *pid > 0) {
        let mut task = MaybeUninit::<ProcTaskInfo>::zeroed();
        let returned = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDTASKINFO,
                0,
                task.as_mut_ptr().cast(),
                size_of::<ProcTaskInfo>() as i32,
            )
        };
        if returned != size_of::<ProcTaskInfo>() as i32 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(ProviderResourceError::unavailable(
                    "cannot read macOS process RSS",
                ));
            }
            continue;
        }
        let task = unsafe { task.assume_init() };
        total = total
            .checked_add(task.resident_size)
            .ok_or_else(|| ProviderResourceError::unavailable("process-tree RSS overflow"))?;
        observed += 1;
    }
    if observed == 0 {
        return if unix_process_group_exists(scope.process_group_id)? {
            Ok(SampleResult::Bytes(0))
        } else {
            Ok(SampleResult::Exited)
        };
    }
    Ok(SampleResult::Bytes(total))
}

#[cfg(windows)]
fn sample_process_tree(
    sampler: &mut ProcessTreeSampler,
) -> Result<SampleResult, ProviderResourceError> {
    use std::ffi::c_void;
    use std::mem::{size_of, MaybeUninit};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA};
    use windows_sys::Win32::System::JobObjects::{
        JobObjectBasicProcessIdList, QueryInformationJobObject,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    let scope = sampler.scope;

    let mut capacity = 64_usize;
    loop {
        let header_bytes = size_of::<u32>() * 2;
        let buffer_bytes = header_bytes.saturating_add(capacity.saturating_mul(size_of::<usize>()));
        let mut buffer = vec![0_u8; buffer_bytes];
        let mut returned = 0_u32;
        let success = unsafe {
            QueryInformationJobObject(
                scope.job_handle as *mut c_void,
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr().cast(),
                u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                &mut returned,
            )
        };
        if success == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_MORE_DATA as i32)
                && capacity < MAX_TRACKED_PROCESSES
            {
                capacity = capacity.saturating_mul(2).min(MAX_TRACKED_PROCESSES);
                continue;
            }
            return Err(ProviderResourceError::unavailable(format!(
                "cannot enumerate the Windows provider job: {error}"
            )));
        }
        let assigned = u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) as usize;
        let listed = u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) as usize;
        if assigned > MAX_TRACKED_PROCESSES || listed > capacity {
            return Err(ProviderResourceError::unavailable(
                "process tree exceeds the accounting process limit",
            ));
        }
        if listed == 0 {
            return Ok(SampleResult::Exited);
        }
        let mut total = 0_u64;
        for index in 0..listed {
            let offset = header_bytes + index * size_of::<usize>();
            let pid = usize::from_ne_bytes(
                buffer[offset..offset + size_of::<usize>()]
                    .try_into()
                    .unwrap(),
            );
            let process = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
                    0,
                    u32::try_from(pid).unwrap_or(u32::MAX),
                )
            };
            if process.is_null() {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                    continue;
                }
                return Err(ProviderResourceError::unavailable(format!(
                    "cannot open a Windows provider process: {error}"
                )));
            }
            let mut counters = MaybeUninit::<PROCESS_MEMORY_COUNTERS>::zeroed();
            let success = unsafe {
                K32GetProcessMemoryInfo(
                    process,
                    counters.as_mut_ptr(),
                    size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                )
            };
            unsafe { CloseHandle(process) };
            if success == 0 {
                return Err(ProviderResourceError::unavailable(
                    "cannot read Windows process RSS",
                ));
            }
            let counters = unsafe { counters.assume_init() };
            total = total
                .checked_add(counters.WorkingSetSize as u64)
                .ok_or_else(|| ProviderResourceError::unavailable("process-tree RSS overflow"))?;
        }
        if total == 0 && assigned > 0 {
            return Err(ProviderResourceError::unavailable(format!(
                "Windows process-tree RSS accounting returned no readable processes for root {}",
                scope.root_pid
            )));
        }
        return Ok(SampleResult::Bytes(total));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn sample_process_tree(
    _sampler: &mut ProcessTreeSampler,
) -> Result<SampleResult, ProviderResourceError> {
    Err(ProviderResourceError::unavailable(
        "process-tree RSS accounting is unavailable on this platform",
    ))
}
