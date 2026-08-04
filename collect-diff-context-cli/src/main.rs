fn main() {
    let exit_code = collect_diff_context_cli::collect_diff_context_main();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}
