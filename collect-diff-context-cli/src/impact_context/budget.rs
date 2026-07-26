use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactBudget {
    pub deadline: Duration,
    pub max_changed_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    pub max_nodes: usize,
    pub max_nesting_depth: usize,
    pub max_facts: usize,
    pub max_edges: usize,
    pub max_output_bytes: usize,
    pub max_query_patterns: usize,
    pub max_matches_per_pattern: usize,
}

impl ImpactBudget {
    pub fn fast_defaults() -> Self {
        Self {
            deadline: Duration::from_millis(750),
            max_changed_files: 30,
            max_file_bytes: 2 * 1024 * 1024,
            max_total_bytes: 8 * 1024 * 1024,
            max_nodes: 250_000,
            max_nesting_depth: 512,
            max_facts: 5_000,
            max_edges: 500,
            max_output_bytes: 1_048_576,
            max_query_patterns: 32,
            max_matches_per_pattern: 20,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BudgetResource {
    ChangedFiles,
    FileBytes,
    TotalBytes,
    Nodes,
    NestingDepth,
    Facts,
    Edges,
    OutputBytes,
    QueryPatterns,
    MatchesPerPattern,
}

impl BudgetResource {
    pub fn exhaustion_code(self) -> &'static str {
        match self {
            Self::ChangedFiles => "changed-file-budget-exhausted",
            Self::FileBytes => "file-byte-budget-exhausted",
            Self::TotalBytes => "total-byte-budget-exhausted",
            Self::Nodes => "node-budget-exhausted",
            Self::NestingDepth => "nesting-depth-budget-exhausted",
            Self::Facts => "fact-budget-exhausted",
            Self::Edges => "edge-budget-exhausted",
            Self::OutputBytes => "output-byte-budget-exhausted",
            Self::QueryPatterns => "query-pattern-budget-exhausted",
            Self::MatchesPerPattern => "query-match-budget-exhausted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetAmount {
    pub initial: usize,
    pub consumed: usize,
    pub remaining: usize,
    pub exhausted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetExhaustion {
    resource: Option<BudgetResource>,
    code: &'static str,
}

impl BudgetExhaustion {
    pub fn code(self) -> &'static str {
        self.code
    }

    pub fn resource(self) -> Option<BudgetResource> {
        self.resource
    }
}

impl std::fmt::Display for BudgetExhaustion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for BudgetExhaustion {}

#[derive(Debug)]
pub struct BudgetTracker {
    budget: ImpactBudget,
    started: Instant,
    consumed: BTreeMap<BudgetResource, usize>,
    exhausted: BTreeSet<BudgetResource>,
    deadline_exhausted: bool,
}

impl BudgetTracker {
    pub fn new(budget: ImpactBudget) -> Self {
        Self {
            budget,
            started: Instant::now(),
            consumed: BTreeMap::new(),
            exhausted: BTreeSet::new(),
            deadline_exhausted: false,
        }
    }

    pub fn budget(&self) -> &ImpactBudget {
        &self.budget
    }

    pub fn consume(
        &mut self,
        resource: BudgetResource,
        amount: usize,
    ) -> Result<(), BudgetExhaustion> {
        let initial = self.limit(resource);
        let consumed = self.consumed.get(&resource).copied().unwrap_or(0);
        let Some(next) = consumed.checked_add(amount) else {
            self.exhausted.insert(resource);
            return Err(resource_exhaustion(resource));
        };
        if next > initial {
            self.exhausted.insert(resource);
            return Err(resource_exhaustion(resource));
        }
        self.consumed.insert(resource, next);
        Ok(())
    }

    pub fn observe(
        &mut self,
        resource: BudgetResource,
        observed: usize,
    ) -> Result<(), BudgetExhaustion> {
        let initial = self.limit(resource);
        let previous = self.consumed.get(&resource).copied().unwrap_or(0);
        self.consumed
            .insert(resource, previous.max(observed.min(initial)));
        if observed > initial {
            self.exhausted.insert(resource);
            return Err(resource_exhaustion(resource));
        }
        Ok(())
    }

    pub fn amount(&self, resource: BudgetResource) -> BudgetAmount {
        let initial = self.limit(resource);
        let consumed = self
            .consumed
            .get(&resource)
            .copied()
            .unwrap_or(0)
            .min(initial);
        BudgetAmount {
            initial,
            consumed,
            remaining: initial.saturating_sub(consumed),
            exhausted: self.exhausted.contains(&resource),
        }
    }

    pub fn check_deadline(&mut self) -> Result<(), BudgetExhaustion> {
        if self.deadline_exhausted || self.started.elapsed() >= self.budget.deadline {
            self.deadline_exhausted = true;
            return Err(BudgetExhaustion {
                resource: None,
                code: "deadline-exhausted",
            });
        }
        Ok(())
    }

    pub fn deadline_exhausted(&self) -> bool {
        self.deadline_exhausted
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn limit(&self, resource: BudgetResource) -> usize {
        match resource {
            BudgetResource::ChangedFiles => self.budget.max_changed_files,
            BudgetResource::FileBytes => self.budget.max_file_bytes,
            BudgetResource::TotalBytes => self.budget.max_total_bytes,
            BudgetResource::Nodes => self.budget.max_nodes,
            BudgetResource::NestingDepth => self.budget.max_nesting_depth,
            BudgetResource::Facts => self.budget.max_facts,
            BudgetResource::Edges => self.budget.max_edges,
            BudgetResource::OutputBytes => self.budget.max_output_bytes,
            BudgetResource::QueryPatterns => self.budget.max_query_patterns,
            BudgetResource::MatchesPerPattern => self.budget.max_matches_per_pattern,
        }
    }
}

fn resource_exhaustion(resource: BudgetResource) -> BudgetExhaustion {
    BudgetExhaustion {
        resource: Some(resource),
        code: resource.exhaustion_code(),
    }
}
