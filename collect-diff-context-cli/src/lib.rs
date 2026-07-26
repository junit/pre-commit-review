mod app;
pub mod candidate;
mod git_policy;
pub mod impact_context;
mod process_group;
pub mod review_scope;
pub mod secret_scan;
pub mod static_analysis;
#[cfg(windows)]
mod windows_acl;

pub fn collect_diff_context_main() -> i32 {
    app::main_entry()
}
