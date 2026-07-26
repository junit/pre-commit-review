mod content;
pub mod snapshot;

pub use content::{
    decode_git_quoted_path, CandidateBytes, CandidateContent, CandidateError, CandidateFile,
    CandidateOpenLimits, CandidatePresence, ChangedRange, GitCandidateContent, RepoPath,
};
