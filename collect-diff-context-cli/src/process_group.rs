use std::process::{Child, Command};

#[cfg(unix)]
pub(crate) fn configure_process_group(command: &mut Command) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;

    // SAFETY: the pre-exec hook calls only async-signal-safe setpgid.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn configure_process_group(command: &mut Command) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED};

    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED);
    Ok(())
}

#[cfg(unix)]
pub(crate) struct ProcessGroup {
    process_group_id: i32,
}

#[cfg(unix)]
impl ProcessGroup {
    pub(crate) fn attach(child: &mut Child) -> std::io::Result<Self> {
        let process_group_id = i32::try_from(child.id())
            .map_err(|_| std::io::Error::other("process id exceeds i32"))?;
        Ok(Self { process_group_id })
    }

    pub(crate) fn terminate(&self, child: &mut Child) {
        // SAFETY: this group id was created for the child immediately before exec.
        unsafe {
            libc::killpg(self.process_group_id, libc::SIGKILL);
        }
        let _ = child.kill();
    }
}

#[cfg(windows)]
pub(crate) struct ProcessGroup {
    job: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ProcessGroup {
    pub(crate) fn attach(child: &mut Child) -> std::io::Result<Self> {
        use std::ffi::c_void;
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        // SAFETY: handles are checked for null and remain owned until Drop.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &mut information as *mut _ as *mut c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                let _ = child.kill();
                return Err(error);
            }
            if AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                let _ = child.kill();
                return Err(error);
            }
            if let Err(error) = resume_suspended_process(child.id()) {
                CloseHandle(job);
                let _ = child.kill();
                return Err(error);
            }
            Ok(Self { job })
        }
    }

    pub(crate) fn terminate(&self, child: &mut Child) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: self.job is a live Job Object handle owned by this guard.
        unsafe {
            TerminateJobObject(self.job, 1);
        }
        let _ = child.kill();
    }
}

#[cfg(windows)]
fn resume_suspended_process(process_id: u32) -> std::io::Result<()> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    // SAFETY: snapshot and thread handles are checked and closed exactly once.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut available = Thread32First(snapshot, &mut entry) != 0;
        while available {
            if entry.th32OwnerProcessID == process_id {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    let error = std::io::Error::last_os_error();
                    CloseHandle(snapshot);
                    return Err(error);
                }
                let previous_suspend_count = ResumeThread(thread);
                CloseHandle(thread);
                CloseHandle(snapshot);
                if previous_suspend_count == u32::MAX || previous_suspend_count == 0 {
                    return Err(std::io::Error::other(
                        "cannot resume suspended process primary thread",
                    ));
                }
                return Ok(());
            }
            available = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    Err(std::io::Error::other(
        "cannot find suspended process primary thread",
    ))
}

#[cfg(windows)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;

        // SAFETY: self.job is owned by this guard and closed exactly once.
        unsafe {
            CloseHandle(self.job);
        }
    }
}
