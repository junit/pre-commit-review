#![cfg(feature = "test-fixture")]

fn main() {
    let exit_code =
        collect_diff_context_cli::repository_context_provider::baseline_fixture::main_entry();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}
