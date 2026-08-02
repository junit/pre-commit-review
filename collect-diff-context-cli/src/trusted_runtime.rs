use crate::process_group::{configure_process_group, ProcessGroup};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use tempfile::TempDir;

const COPY_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedRuntimeError {
    pub(crate) code: &'static str,
    message: String,
}

impl TrustedRuntimeError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        let message = message.into().chars().take(500).collect();
        Self { code, message }
    }
}

impl std::fmt::Display for TrustedRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TrustedRuntimeError {}

#[derive(Debug)]
pub(crate) struct PrivateRuntime {
    root: TempDir,
    home: PathBuf,
    temporary: PathBuf,
    target: PathBuf,
    empty_path: PathBuf,
    executable_path: PathBuf,
    executable_sha256: String,
}

impl PrivateRuntime {
    pub(crate) fn create(
        source: &Path,
        expected_sha256: &str,
    ) -> Result<Self, TrustedRuntimeError> {
        validate_sha256(expected_sha256)?;
        let input = File::open(source).map_err(|error| {
            TrustedRuntimeError::new(
                "trusted-runtime-executable-invalid",
                format!("cannot open authorized executable: {error}"),
            )
        })?;
        let metadata = input.metadata().map_err(|error| {
            TrustedRuntimeError::new(
                "trusted-runtime-executable-invalid",
                format!("cannot inspect authorized executable: {error}"),
            )
        })?;
        if !metadata.is_file() || !is_executable(&metadata) {
            return Err(TrustedRuntimeError::new(
                "trusted-runtime-executable-invalid",
                "authorized executable must remain an executable regular file",
            ));
        }

        let root = tempfile::Builder::new()
            .prefix("pre-commit-review-runtime-")
            .tempdir()
            .map_err(runtime_create_error)?;
        set_private_directory(root.path())?;
        let home = create_private_directory(root.path(), "home")?;
        let temporary = create_private_directory(root.path(), "tmp")?;
        let target = create_private_directory(root.path(), "target")?;
        let empty_path = create_private_directory(root.path(), "empty-path")?;
        let executable_path = root.path().join(runtime_executable_name(source));
        let observed_sha256 = copy_and_hash(input, &executable_path)?;
        if observed_sha256 != expected_sha256 {
            return Err(TrustedRuntimeError::new(
                "trusted-runtime-executable-mismatch",
                "authorized executable digest changed before execution",
            ));
        }
        set_executable_permissions(&executable_path)?;

        let runtime = Self {
            root,
            home,
            temporary,
            target,
            empty_path,
            executable_path,
            executable_sha256: expected_sha256.to_string(),
        };
        runtime.verify()?;
        Ok(runtime)
    }

    pub(crate) fn path(&self) -> &Path {
        self.root.path()
    }

    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    pub(crate) fn temporary(&self) -> &Path {
        &self.temporary
    }

    pub(crate) fn target(&self) -> &Path {
        &self.target
    }

    #[allow(dead_code)]
    pub(crate) fn empty_path(&self) -> &Path {
        &self.empty_path
    }

    pub(crate) fn executable_path(&self) -> &Path {
        &self.executable_path
    }

    pub(crate) fn verify(&self) -> Result<(), TrustedRuntimeError> {
        for directory in [&self.home, &self.temporary, &self.target, &self.empty_path] {
            if !directory.is_dir() {
                return Err(TrustedRuntimeError::new(
                    "trusted-runtime-directory-invalid",
                    "private runtime directory changed during execution",
                ));
            }
        }
        let observed_sha256 = hash_file(&self.executable_path)?;
        if observed_sha256 != self.executable_sha256 {
            return Err(TrustedRuntimeError::new(
                "trusted-runtime-executable-mismatch",
                "private executable digest changed during execution",
            ));
        }
        Ok(())
    }
}

impl Drop for PrivateRuntime {
    fn drop(&mut self) {
        #[cfg(not(unix))]
        if let Ok(mut permissions) =
            fs::metadata(&self.executable_path).map(|metadata| metadata.permissions())
        {
            permissions.set_readonly(false);
            let _ = fs::set_permissions(&self.executable_path, permissions);
        }
    }
}

pub(crate) struct ManagedChild {
    child: Option<Child>,
    process_group: ProcessGroup,
}

impl ManagedChild {
    pub(crate) fn spawn(mut command: Command) -> Result<Self, TrustedRuntimeError> {
        configure_process_group(&mut command).map_err(|error| {
            TrustedRuntimeError::new(
                "trusted-runtime-child-configure",
                format!("cannot configure child process group: {error}"),
            )
        })?;
        let mut child = command.spawn().map_err(|error| {
            TrustedRuntimeError::new(
                "trusted-runtime-child-spawn",
                format!("cannot start trusted child: {error}"),
            )
        })?;
        let process_group = ProcessGroup::attach(&mut child).map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            TrustedRuntimeError::new(
                "trusted-runtime-child-attach",
                format!("cannot attach child process group: {error}"),
            )
        })?;
        Ok(Self {
            child: Some(child),
            process_group,
        })
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("managed child is unavailable after it has been reaped")
    }

    pub(crate) fn resource_scope(&self) -> crate::provider_resources::ProviderProcessScope {
        let child = self
            .child
            .as_ref()
            .expect("managed child is unavailable after it has been reaped");
        self.process_group.resource_scope(child.id())
    }

    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>, TrustedRuntimeError> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        let status = child.try_wait().map_err(child_wait_error)?;
        if status.is_some() {
            self.process_group.terminate(child);
            let _ = child.wait();
            self.child = None;
        }
        Ok(status)
    }

    #[allow(dead_code)]
    pub(crate) fn wait(&mut self) -> Result<ExitStatus, TrustedRuntimeError> {
        let mut child = self.child.take().ok_or_else(|| {
            TrustedRuntimeError::new(
                "trusted-runtime-child-reaped",
                "trusted child has already been reaped",
            )
        })?;
        let status = child.wait();
        self.process_group.terminate(&mut child);
        status.map_err(child_wait_error)
    }

    pub(crate) fn terminate_and_wait(&mut self) -> Result<Option<ExitStatus>, TrustedRuntimeError> {
        let Some(mut child) = self.child.take() else {
            return Ok(None);
        };
        self.process_group.terminate(&mut child);
        child.wait().map(Some).map_err(child_wait_error)
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.terminate_and_wait();
    }
}

pub(crate) fn apply_base_environment(
    command: &mut Command,
    runtime: &PrivateRuntime,
    path: &OsStr,
    source: &str,
    scope_fingerprint: &str,
) {
    command
        .env_clear()
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("HOME", runtime.home())
        .env("TMPDIR", runtime.temporary())
        .env("TMP", runtime.temporary())
        .env("TEMP", runtime.temporary())
        .env("NO_COLOR", "1")
        .env("PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT", scope_fingerprint)
        .env("PRE_COMMIT_REVIEW_SOURCE", source)
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "");
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

fn validate_sha256(value: &str) -> Result<(), TrustedRuntimeError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err(TrustedRuntimeError::new(
        "trusted-runtime-digest-invalid",
        "authorized executable digest must be lowercase SHA-256",
    ))
}

fn create_private_directory(root: &Path, name: &str) -> Result<PathBuf, TrustedRuntimeError> {
    let path = root.join(name);
    fs::create_dir(&path).map_err(runtime_create_error)?;
    set_private_directory(&path)?;
    Ok(path)
}

fn runtime_executable_name(source: &Path) -> OsString {
    let mut name = OsString::from("trusted-executable");
    if let Some(extension) = source.extension() {
        name.push(".");
        name.push(extension);
    }
    name
}

fn copy_and_hash(mut input: File, destination: &Path) -> Result<String, TrustedRuntimeError> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(runtime_create_error)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            TrustedRuntimeError::new(
                "trusted-runtime-executable-invalid",
                format!("cannot read authorized executable: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(runtime_create_error)?;
    }
    output.flush().map_err(runtime_create_error)?;
    Ok(format!("{:x}", digest.finalize()))
}

fn hash_file(path: &Path) -> Result<String, TrustedRuntimeError> {
    let mut input = File::open(path).map_err(|error| {
        TrustedRuntimeError::new(
            "trusted-runtime-executable-invalid",
            format!("cannot open private executable: {error}"),
        )
    })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            TrustedRuntimeError::new(
                "trusted-runtime-executable-invalid",
                format!("cannot read private executable: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn runtime_create_error(error: std::io::Error) -> TrustedRuntimeError {
    TrustedRuntimeError::new(
        "trusted-runtime-create",
        format!("cannot create private runtime: {error}"),
    )
}

fn child_wait_error(error: std::io::Error) -> TrustedRuntimeError {
    TrustedRuntimeError::new(
        "trusted-runtime-child-wait",
        format!("cannot wait for trusted child: {error}"),
    )
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), TrustedRuntimeError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(runtime_create_error)
}

#[cfg(windows)]
fn set_private_directory(path: &Path) -> Result<(), TrustedRuntimeError> {
    crate::windows_acl::restrict_tree_private(path)
        .map_err(|error| TrustedRuntimeError::new("trusted-runtime-create", error))
}

#[cfg(unix)]
fn set_executable_permissions(path: &Path) -> Result<(), TrustedRuntimeError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o500)).map_err(runtime_create_error)
}

#[cfg(not(unix))]
fn set_executable_permissions(path: &Path) -> Result<(), TrustedRuntimeError> {
    let mut permissions = fs::metadata(path)
        .map_err(runtime_create_error)?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(runtime_create_error)
}

#[cfg(test)]
mod tests {
    use super::PrivateRuntime;
    use sha2::{Digest, Sha256};

    #[test]
    fn private_runtime_copies_and_reverifies_the_authorized_executable() {
        let source = std::env::current_exe().unwrap();
        let expected_sha256 = format!("{:x}", Sha256::digest(std::fs::read(&source).unwrap()));

        let runtime = PrivateRuntime::create(&source, &expected_sha256).unwrap();

        assert_eq!(
            std::fs::read(runtime.executable_path()).unwrap(),
            std::fs::read(source).unwrap()
        );
        runtime.verify().unwrap();
        assert!(runtime.home().is_dir());
        assert!(runtime.temporary().is_dir());
        assert!(runtime.target.is_dir());
        assert!(runtime.empty_path().is_dir());
    }

    #[test]
    fn private_runtime_creation_fits_windows_main_thread_stack() {
        const WINDOWS_MAIN_THREAD_STACK_BYTES: usize = 1024 * 1024;

        let source = std::env::current_exe().unwrap();
        let expected_sha256 = format!("{:x}", Sha256::digest(std::fs::read(&source).unwrap()));

        let worker = std::thread::Builder::new()
            .name("trusted-runtime-stack-regression".to_string())
            .stack_size(WINDOWS_MAIN_THREAD_STACK_BYTES)
            .spawn(move || PrivateRuntime::create(&source, &expected_sha256))
            .unwrap();
        let runtime = worker.join().unwrap().unwrap();
        runtime.verify().unwrap();
    }

    #[test]
    fn private_runtime_rejects_an_unauthorized_executable_digest() {
        let source = std::env::current_exe().unwrap();

        let error = PrivateRuntime::create(&source, &"0".repeat(64)).unwrap_err();

        assert_eq!(error.code, "trusted-runtime-executable-mismatch");
    }

    #[cfg(unix)]
    #[test]
    fn managed_child_drop_terminates_and_reaps_the_process() {
        use super::ManagedChild;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "exec sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let process_id = {
            let mut child = ManagedChild::spawn(command).unwrap();
            child.child_mut().id()
        };

        let deadline = Instant::now() + Duration::from_secs(2);
        while process_exists(process_id) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!process_exists(process_id));
    }

    #[cfg(unix)]
    fn process_exists(process_id: u32) -> bool {
        let process_id = i32::try_from(process_id).unwrap();
        // SAFETY: signal zero only checks whether the captured process ID exists.
        unsafe { libc::kill(process_id, 0) == 0 }
    }
}
