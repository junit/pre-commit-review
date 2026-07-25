mod app;
pub mod secret_scan;

pub fn collect_diff_context_main() -> i32 {
    app::main_entry()
}
