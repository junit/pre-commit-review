#![no_main]

use collect_diff_context_cli::candidate::ChangedRange;
use collect_diff_context_cli::impact_context::adapters::tree_sitter_rust::TreeSitterRustAdapter;
use collect_diff_context_cli::impact_context::budget::{BudgetTracker, ImpactBudget};
use collect_diff_context_cli::impact_context::contracts::SourceRange;
use libfuzzer_sys::fuzz_target;

const HEADER_BYTES: usize = 16;

fn assert_range_within_input(range: &SourceRange, input_bytes: usize) {
    assert!(range.start_line > 0);
    assert!(range.start_column > 0);
    assert!(range.end_line > 0);
    assert!(range.end_column > 0);
    assert!(range.start_byte <= range.end_byte);
    assert!(range.end_byte <= input_bytes);
}

fuzz_target!(|data: &[u8]| {
    let mut header = [0_u8; HEADER_BYTES];
    let header_len = data.len().min(HEADER_BYTES);
    header[..header_len].copy_from_slice(&data[..header_len]);
    let source = data.get(HEADER_BYTES..).unwrap_or_default();
    let line_count = source
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        .saturating_add(1)
        .min(u32::MAX as usize) as u32;
    let range_count = usize::from(header[4] % 4).saturating_add(1);
    let mut changed_ranges = Vec::with_capacity(range_count);
    for index in 0..range_count {
        let first = 1 + u32::from(header[5 + index * 2]) % line_count;
        let second = 1 + u32::from(header[6 + index * 2]) % line_count;
        changed_ranges.push(ChangedRange {
            start_line: first.min(second),
            end_line: first.max(second),
            deletion_anchor: header[13 + index % 3] & 1 == 1,
        });
    }

    let mut budget = ImpactBudget::fast_defaults();
    budget.max_nodes = usize::from(header[0] % 128).saturating_add(1);
    budget.max_nesting_depth = usize::from(header[1] % 64).saturating_add(1);
    budget.max_facts = usize::from(header[2] % 64).saturating_add(1);
    budget.max_edges = usize::from(header[3] % 16).saturating_add(1);
    let limits = budget.clone();
    let mut tracker = BudgetTracker::new(budget);

    let Ok(output) = TreeSitterRustAdapter::analyze(source, &changed_ranges, &mut tracker) else {
        return;
    };

    for range in &output.affected_ranges {
        assert_range_within_input(range, source.len());
    }
    for range in output
        .changed_symbols
        .iter()
        .map(|fact| &fact.range)
        .chain(output.imports.iter().map(|fact| &fact.range))
        .chain(output.calls.iter().map(|fact| &fact.range))
        .chain(output.macros.iter().map(|fact| &fact.range))
        .chain(output.attributes.iter().map(|fact| &fact.range))
    {
        assert_range_within_input(range, source.len());
    }
    let fact_count = output.changed_symbols.len()
        + output.imports.len()
        + output.calls.len()
        + output.macros.len()
        + output.attributes.len();
    assert!(output.nodes_visited <= limits.max_nodes);
    assert!(output.max_nesting_depth <= limits.max_nesting_depth);
    assert!(fact_count <= limits.max_facts);
    assert!(output.calls.len() <= limits.max_edges);
});
