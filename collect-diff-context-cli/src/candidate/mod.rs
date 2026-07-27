mod content;
pub mod snapshot;

pub use content::{
    decode_git_quoted_path, CandidateBytes, CandidateContent, CandidateError, CandidateFile,
    CandidateOpenLimits, CandidatePresence, ChangedRange, GitCandidateContent, RepoPath,
};
pub(crate) use content::{
    hash_unstaged_path_bounded, read_git_blobs_batch_bounded, read_unstaged_path_bounded,
    unstaged_mode, unstaged_path_size,
};
