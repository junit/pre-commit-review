use collect_diff_context_cli::collect_diff_context_main;

#[test]
fn library_exports_collect_diff_context_entrypoint() {
    let _: fn() -> i32 = collect_diff_context_main;
}
