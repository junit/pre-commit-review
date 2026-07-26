use collect_diff_context_cli::impact_context::budget::{
    BudgetResource, BudgetTracker, ImpactBudget,
};
use std::time::Duration;

#[test]
fn budget_file_bytes_exhaust_independently() {
    let mut budget = ImpactBudget::fast_defaults();
    budget.max_file_bytes = 4;
    budget.max_total_bytes = 100;
    let mut tracker = BudgetTracker::new(budget);

    tracker.observe(BudgetResource::FileBytes, 4).unwrap();
    let error = tracker
        .observe(BudgetResource::FileBytes, 5)
        .expect_err("oversized file must exhaust only the file-byte budget");

    assert_eq!(error.code(), "file-byte-budget-exhausted");
    assert_eq!(tracker.amount(BudgetResource::FileBytes).initial, 4);
    assert_eq!(tracker.amount(BudgetResource::FileBytes).consumed, 4);
    assert_eq!(tracker.amount(BudgetResource::FileBytes).remaining, 0);
    assert_eq!(tracker.amount(BudgetResource::TotalBytes).consumed, 0);
    assert_eq!(tracker.amount(BudgetResource::TotalBytes).remaining, 100);
}

#[test]
fn budget_fast_defaults_match_the_contract() {
    let budget = ImpactBudget::fast_defaults();
    assert_eq!(budget.deadline, Duration::from_millis(750));
    assert_eq!(budget.max_changed_files, 30);
    assert_eq!(budget.max_file_bytes, 2 * 1024 * 1024);
    assert_eq!(budget.max_total_bytes, 8 * 1024 * 1024);
    assert_eq!(budget.max_nodes, 250_000);
    assert_eq!(budget.max_nesting_depth, 512);
    assert_eq!(budget.max_facts, 5_000);
    assert_eq!(budget.max_edges, 500);
    assert_eq!(budget.max_output_bytes, 1_048_576);
    assert_eq!(budget.max_query_patterns, 32);
    assert_eq!(budget.max_matches_per_pattern, 20);
}

#[test]
fn budget_cumulative_resources_never_exceed_their_initial_amount() {
    for resource in [
        BudgetResource::ChangedFiles,
        BudgetResource::TotalBytes,
        BudgetResource::Nodes,
        BudgetResource::Facts,
        BudgetResource::Edges,
        BudgetResource::OutputBytes,
        BudgetResource::QueryPatterns,
    ] {
        let mut budget = ImpactBudget::fast_defaults();
        budget.max_changed_files = 2;
        budget.max_total_bytes = 2;
        budget.max_nodes = 2;
        budget.max_facts = 2;
        budget.max_edges = 2;
        budget.max_output_bytes = 2;
        budget.max_query_patterns = 2;
        let mut tracker = BudgetTracker::new(budget);

        tracker.consume(resource, 2).unwrap();
        let error = tracker.consume(resource, 1).unwrap_err();
        let amount = tracker.amount(resource);

        assert_eq!(error.code(), resource.exhaustion_code());
        assert_eq!(amount.initial, 2);
        assert_eq!(amount.consumed, 2);
        assert_eq!(amount.remaining, 0);
        assert!(amount.exhausted);
    }
}

#[test]
fn budget_observed_resources_use_bounded_high_water_marks() {
    for resource in [
        BudgetResource::FileBytes,
        BudgetResource::NestingDepth,
        BudgetResource::MatchesPerPattern,
    ] {
        let mut budget = ImpactBudget::fast_defaults();
        budget.max_file_bytes = 3;
        budget.max_nesting_depth = 3;
        budget.max_matches_per_pattern = 3;
        let mut tracker = BudgetTracker::new(budget);

        tracker.observe(resource, 2).unwrap();
        let error = tracker.observe(resource, usize::MAX).unwrap_err();
        let amount = tracker.amount(resource);

        assert_eq!(error.code(), resource.exhaustion_code());
        assert_eq!(amount.initial, 3);
        assert_eq!(amount.consumed, 3);
        assert_eq!(amount.remaining, 0);
        assert!(amount.exhausted);
    }
}

#[test]
fn budget_exhausted_unit_does_not_erase_previously_accepted_facts() {
    let mut budget = ImpactBudget::fast_defaults();
    budget.max_file_bytes = 4;
    budget.max_facts = 10;
    let mut tracker = BudgetTracker::new(budget);
    tracker.consume(BudgetResource::Facts, 3).unwrap();

    tracker.observe(BudgetResource::FileBytes, 5).unwrap_err();

    assert_eq!(tracker.amount(BudgetResource::Facts).consumed, 3);
    assert_eq!(tracker.amount(BudgetResource::Facts).remaining, 7);
}

#[test]
fn budget_deadline_exhaustion_is_stable_and_monotonic() {
    let mut budget = ImpactBudget::fast_defaults();
    budget.deadline = Duration::ZERO;
    let mut tracker = BudgetTracker::new(budget);

    let first = tracker.check_deadline().unwrap_err();
    let second = tracker.check_deadline().unwrap_err();

    assert_eq!(first.code(), "deadline-exhausted");
    assert_eq!(second.code(), "deadline-exhausted");
    assert!(tracker.deadline_exhausted());
}
