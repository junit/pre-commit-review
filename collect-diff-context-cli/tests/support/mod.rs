use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, AuthoritativeScope, ReviewSource, ScopeRequest,
};
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

pub struct GitRepo {
    root: TempDir,
}

impl GitRepo {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let root = TempDir::new()?;
        let repo = Self { root };
        repo.git(["init", "-q"])?;
        repo.git(["config", "user.email", "review@example.test"])?;
        repo.git(["config", "user.name", "Review Test"])?;
        Ok(repo)
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn write(&self, path: &str, bytes: impl AsRef<[u8]>) -> Result<(), Box<dyn Error>> {
        let path = self.root.path().join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
        Ok(())
    }

    pub fn git<I, S>(&self, args: I) -> Result<Output, Box<dyn Error>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.root.path())
            .output()?;
        if !output.status.success() {
            return Err(format!("git failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
        Ok(output)
    }

    pub fn commit_file(&self, path: &str, bytes: impl AsRef<[u8]>) -> Result<(), Box<dyn Error>> {
        self.write(path, bytes)?;
        self.git(["add", "--", path])?;
        self.git(["commit", "-qm", "fixture"])?;
        Ok(())
    }

    pub fn scope(&self, source: ReviewSource) -> Result<AuthoritativeScope, Box<dyn Error>> {
        Ok(open_authoritative_scope(ScopeRequest {
            repository: PathBuf::from(self.path()),
            source: Some(source),
            expected_fingerprint: None,
        })?)
    }
}
