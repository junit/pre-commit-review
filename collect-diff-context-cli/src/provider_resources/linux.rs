use super::{
    unix_process_group_exists, ProcessTreeSampler, ProviderResourceError, SampleResult,
    MAX_TRACKED_PROCESSES,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(super) struct TrackedProcess {
    start_time: u64,
    pidfd: OwnedFd,
}

pub(super) fn sample_process_tree(
    sampler: &mut ProcessTreeSampler,
) -> Result<SampleResult, ProviderResourceError> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(ProviderResourceError::unavailable(
            "cannot determine the Linux memory page size",
        ));
    }
    discard_stale_processes(sampler)?;
    track_process_group(sampler)?;
    track_descendants(sampler)?;

    if sampler.tracked.is_empty() {
        return if unix_process_group_exists(sampler.scope.process_group_id)? {
            Err(ProviderResourceError::unavailable(
                "Linux provider process group exists but cannot be accounted",
            ))
        } else {
            Ok(SampleResult::Exited)
        };
    }

    sum_tracked_rss(sampler, page_size as u64)
}

fn discard_stale_processes(sampler: &mut ProcessTreeSampler) -> Result<(), ProviderResourceError> {
    let known = sampler.tracked.keys().copied().collect::<Vec<_>>();
    for pid in known {
        let Some(process) = sampler.tracked.get(&pid) else {
            continue;
        };
        if !tracked_process_is_current(pid, process)? {
            sampler.tracked.remove(&pid);
        }
    }
    Ok(())
}

fn track_process_group(sampler: &mut ProcessTreeSampler) -> Result<(), ProviderResourceError> {
    let processes = fs::read_dir("/proc").map_err(|error| {
        ProviderResourceError::unavailable(format!(
            "cannot enumerate Linux processes for RSS accounting: {error}"
        ))
    })?;
    for process in processes {
        let process = process.map_err(|error| {
            ProviderResourceError::unavailable(format!(
                "cannot enumerate Linux processes for RSS accounting: {error}"
            ))
        })?;
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let candidate = match fs::read_to_string(process.path().join("stat")) {
            Ok(stat) => stat,
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::PermissionDenied
                ) =>
            {
                continue;
            }
            Err(error) => {
                return Err(ProviderResourceError::unavailable(format!(
                    "cannot read Linux process identity: {error}"
                )))
            }
        };
        let Some(candidate) = parse_process_stat(&candidate) else {
            continue;
        };
        if candidate.process_group_id != sampler.scope.process_group_id {
            continue;
        }
        let Some((stat, pidfd)) = bind_process(pid)? else {
            continue;
        };
        if stat.process_group_id == sampler.scope.process_group_id {
            track_process(sampler, pid, stat.start_time, pidfd)?;
        }
    }
    Ok(())
}

fn track_descendants(sampler: &mut ProcessTreeSampler) -> Result<(), ProviderResourceError> {
    let mut pending = sampler
        .tracked
        .iter()
        .map(|(&pid, process)| (pid, process.start_time))
        .collect::<Vec<_>>();
    let mut expanded = BTreeSet::new();
    while let Some((pid, start_time)) = pending.pop() {
        if !expanded.insert((pid, start_time)) {
            continue;
        }
        let Some(parent) = sampler.tracked.get(&pid) else {
            continue;
        };
        if parent.start_time != start_time || !tracked_process_is_current(pid, parent)? {
            sampler.tracked.remove(&pid);
            continue;
        }

        let children = read_child_process_ids(pid)?;
        let Some(parent) = sampler.tracked.get(&pid) else {
            continue;
        };
        if !tracked_process_is_current(pid, parent)? {
            return Err(ProviderResourceError::unavailable(
                "Linux parent process identity changed during descendant accounting",
            ));
        }

        for child in children {
            let Some((stat, pidfd)) = bind_process(child)? else {
                continue;
            };
            if stat.parent_pid != pid {
                return Err(ProviderResourceError::unavailable(
                    "Linux child process identity changed during descendant accounting",
                ));
            }
            let start_time = stat.start_time;
            track_process(sampler, child, start_time, pidfd)?;
            pending.push((child, start_time));
        }
    }
    Ok(())
}

fn read_child_process_ids(pid: u32) -> Result<Vec<u32>, ProviderResourceError> {
    let tasks = match fs::read_dir(format!("/proc/{pid}/task")) {
        Ok(tasks) => tasks,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(ProviderResourceError::unavailable(format!(
                "cannot enumerate Linux process tasks: {error}"
            )))
        }
    };
    let mut process_ids = Vec::new();
    for task in tasks {
        let task = task.map_err(|error| {
            ProviderResourceError::unavailable(format!(
                "cannot enumerate Linux process tasks: {error}"
            ))
        })?;
        let children = match fs::read_to_string(task.path().join("children")) {
            Ok(children) => children,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(ProviderResourceError::unavailable(format!(
                    "cannot enumerate Linux process children: {error}"
                )))
            }
        };
        for child in children.split_whitespace() {
            process_ids.push(child.parse::<u32>().map_err(|_| {
                ProviderResourceError::unavailable("Linux child process id is malformed")
            })?);
            if process_ids.len() > MAX_TRACKED_PROCESSES {
                return Err(ProviderResourceError::unavailable(
                    "process tree exceeds the accounting process limit",
                ));
            }
        }
    }
    Ok(process_ids)
}

fn sum_tracked_rss(
    sampler: &mut ProcessTreeSampler,
    page_size: u64,
) -> Result<SampleResult, ProviderResourceError> {
    let mut total = 0_u64;
    let mut observed = 0_usize;
    let mut disappeared = Vec::new();
    for (&pid, process) in &sampler.tracked {
        if !tracked_process_is_current(pid, process)? {
            disappeared.push(pid);
            continue;
        }
        let statm = match fs::read_to_string(format!("/proc/{pid}/statm")) {
            Ok(statm) => statm,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                disappeared.push(pid);
                continue;
            }
            Err(error) => {
                return Err(ProviderResourceError::unavailable(format!(
                    "cannot read Linux process RSS: {error}"
                )))
            }
        };
        let resident_pages = statm
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| ProviderResourceError::unavailable("Linux process RSS is malformed"))?;
        if !tracked_process_is_current(pid, process)? {
            disappeared.push(pid);
            continue;
        }
        total = total
            .checked_add(resident_pages.saturating_mul(page_size))
            .ok_or_else(|| ProviderResourceError::unavailable("process-tree RSS overflow"))?;
        observed += 1;
    }
    for pid in disappeared {
        sampler.tracked.remove(&pid);
    }
    if observed == 0 {
        return if !sampler.tracked.is_empty()
            || unix_process_group_exists(sampler.scope.process_group_id)?
        {
            Ok(SampleResult::Bytes(0))
        } else {
            Ok(SampleResult::Exited)
        };
    }
    Ok(SampleResult::Bytes(total))
}

fn track_process(
    sampler: &mut ProcessTreeSampler,
    pid: u32,
    start_time: u64,
    pidfd: OwnedFd,
) -> Result<(), ProviderResourceError> {
    if let Some(process) = sampler.tracked.get(&pid) {
        if process.start_time == start_time {
            return Ok(());
        }
        return Err(ProviderResourceError::unavailable(
            "Linux process identity changed during accounting",
        ));
    }
    if sampler.tracked.len() >= MAX_TRACKED_PROCESSES {
        return Err(ProviderResourceError::unavailable(
            "process tree exceeds the accounting process limit",
        ));
    }
    sampler
        .tracked
        .insert(pid, TrackedProcess { start_time, pidfd });
    Ok(())
}

#[derive(Clone, Copy)]
struct ProcessStat {
    parent_pid: u32,
    process_group_id: i32,
    start_time: u64,
}

fn parse_process_stat(stat: &str) -> Option<ProcessStat> {
    let command_end = stat.rfind(')')?;
    let fields = stat
        .get(command_end + 1..)?
        .split_whitespace()
        .collect::<Vec<_>>();
    Some(ProcessStat {
        parent_pid: fields.get(1)?.parse().ok()?,
        process_group_id: fields.get(2)?.parse().ok()?,
        start_time: fields.get(19)?.parse().ok()?,
    })
}

fn read_process_stat(pid: u32) -> Result<Option<ProcessStat>, ProviderResourceError> {
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ProviderResourceError::unavailable(format!(
                "cannot read Linux process identity: {error}"
            )))
        }
    };
    parse_process_stat(&stat)
        .map(Some)
        .ok_or_else(|| ProviderResourceError::unavailable("Linux process identity is malformed"))
}

fn bind_process(pid: u32) -> Result<Option<(ProcessStat, OwnedFd)>, ProviderResourceError> {
    let Some(pidfd) = open_pidfd(pid)? else {
        return Ok(None);
    };
    let Some(stat) = read_process_stat(pid)? else {
        return Ok(None);
    };
    if pidfd_has_exited(&pidfd)? {
        return Ok(None);
    }
    Ok(Some((stat, pidfd)))
}

fn tracked_process_is_current(
    pid: u32,
    process: &TrackedProcess,
) -> Result<bool, ProviderResourceError> {
    if pidfd_has_exited(&process.pidfd)? {
        return Ok(false);
    }
    let Some(stat) = read_process_stat(pid)? else {
        return Ok(false);
    };
    if stat.start_time != process.start_time {
        return Ok(false);
    }
    Ok(!pidfd_has_exited(&process.pidfd)?)
}

pub(super) fn terminate_linux_tracked(
    tracked: &BTreeMap<u32, TrackedProcess>,
) -> Result<(), ProviderResourceError> {
    for process in tracked.values() {
        signal_pidfd(&process.pidfd)?;
    }
    Ok(())
}

fn open_pidfd(pid: u32) -> Result<Option<OwnedFd>, ProviderResourceError> {
    let pid = libc::pid_t::try_from(pid)
        .map_err(|_| ProviderResourceError::unavailable("Linux process id is out of range"))?;
    let raw_fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0_u32) };
    if raw_fd == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(ProviderResourceError::unavailable(format!(
            "cannot open a stable Linux process handle: {error}"
        )));
    }
    let raw_fd = i32::try_from(raw_fd)
        .map_err(|_| ProviderResourceError::unavailable("Linux process handle is out of range"))?;
    Ok(Some(unsafe { OwnedFd::from_raw_fd(raw_fd) }))
}

fn signal_pidfd(pidfd: &OwnedFd) -> Result<(), ProviderResourceError> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            libc::SIGKILL,
            std::ptr::null::<libc::siginfo_t>(),
            0_u32,
        )
    };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(ProviderResourceError::unavailable(format!(
                "cannot terminate a tracked Linux process: {error}"
            )));
        }
    }
    Ok(())
}

fn pidfd_has_exited(pidfd: &OwnedFd) -> Result<bool, ProviderResourceError> {
    let mut descriptor = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if result == -1 {
        return Err(ProviderResourceError::unavailable(format!(
            "cannot inspect a stable Linux process handle: {}",
            std::io::Error::last_os_error()
        )));
    }
    if descriptor.revents & (libc::POLLNVAL | libc::POLLERR) != 0 {
        return Err(ProviderResourceError::unavailable(
            "stable Linux process handle became invalid",
        ));
    }
    Ok(descriptor.revents & (libc::POLLIN | libc::POLLHUP) != 0)
}
