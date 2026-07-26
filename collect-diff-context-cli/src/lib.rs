mod app;
pub mod candidate;
pub mod review_scope;
pub mod secret_scan;
pub mod static_analysis;

pub fn collect_diff_context_main() -> i32 {
    app::main_entry()
}
