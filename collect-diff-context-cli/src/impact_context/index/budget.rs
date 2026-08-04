use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexBudget {
    pub deadline: Duration,
    pub max_manifest_files: usize,
    pub max_manifest_bytes: usize,
    pub max_project_model_files: usize,
    pub max_project_model_bytes: usize,
    pub max_file_bytes: usize,
    pub max_parse_bytes: usize,
    pub max_nodes: usize,
    pub max_facts: usize,
    pub max_symbols: usize,
    pub max_edges: usize,
    pub max_generation_bytes: usize,
    pub max_overlay_paths: usize,
    pub max_query_rows: usize,
    pub max_graph_depth: usize,
}

impl IndexBudget {
    pub fn fast_defaults() -> Self {
        let mut budget = Self::deep_defaults();
        budget.deadline = Duration::from_millis(750);
        budget.max_parse_bytes = 8 * 1024 * 1024;
        budget.max_nodes = 250_000;
        budget.max_facts = 50_000;
        budget.max_symbols = 50_000;
        budget.max_edges = 500;
        budget.max_generation_bytes = 64 * 1024 * 1024;
        budget.max_overlay_paths = 30;
        budget.max_query_rows = 500;
        budget.max_graph_depth = 1;
        budget
    }

    pub fn deep_defaults() -> Self {
        Self {
            deadline: Duration::from_secs(30),
            max_manifest_files: 100_000,
            max_manifest_bytes: 32 * 1024 * 1024,
            max_project_model_files: 1_000,
            max_project_model_bytes: 8 * 1024 * 1024,
            max_file_bytes: 2 * 1024 * 1024,
            max_parse_bytes: 512 * 1024 * 1024,
            max_nodes: 10_000_000,
            max_facts: 2_000_000,
            max_symbols: 1_000_000,
            max_edges: 5_000_000,
            max_generation_bytes: 2 * 1024 * 1024 * 1024,
            max_overlay_paths: 10_000,
            max_query_rows: 50_000,
            max_graph_depth: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexResource {
    ManifestFiles,
    ManifestBytes,
    ProjectModelFiles,
    ProjectModelBytes,
    FileBytes,
    ParseBytes,
    Nodes,
    Facts,
    Symbols,
    Edges,
    GenerationBytes,
    OverlayPaths,
    QueryRows,
    GraphDepth,
}

impl IndexResource {
    pub fn exhaustion_code(self) -> &'static str {
        match self {
            Self::ManifestFiles => "index-manifest-file-budget-exhausted",
            Self::ManifestBytes => "index-manifest-byte-budget-exhausted",
            Self::ProjectModelFiles => "index-project-model-file-budget-exhausted",
            Self::ProjectModelBytes => "index-project-model-byte-budget-exhausted",
            Self::FileBytes => "index-file-byte-budget-exhausted",
            Self::ParseBytes => "index-parse-byte-budget-exhausted",
            Self::Nodes => "index-node-budget-exhausted",
            Self::Facts => "index-fact-budget-exhausted",
            Self::Symbols => "index-symbol-budget-exhausted",
            Self::Edges => "index-edge-budget-exhausted",
            Self::GenerationBytes => "index-generation-byte-budget-exhausted",
            Self::OverlayPaths => "index-overlay-path-budget-exhausted",
            Self::QueryRows => "index-query-row-budget-exhausted",
            Self::GraphDepth => "index-graph-depth-budget-exhausted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexBudgetAmount {
    pub initial: usize,
    pub consumed: usize,
    pub remaining: usize,
    pub exhausted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexBudgetExhaustion {
    resource: Option<IndexResource>,
    code: &'static str,
}

impl IndexBudgetExhaustion {
    pub fn code(self) -> &'static str {
        self.code
    }

    pub fn resource(self) -> Option<IndexResource> {
        self.resource
    }
}

impl std::fmt::Display for IndexBudgetExhaustion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for IndexBudgetExhaustion {}

#[derive(Debug)]
pub struct IndexBudgetTracker {
    budget: IndexBudget,
    started: Instant,
    consumed: BTreeMap<IndexResource, usize>,
    exhausted: BTreeSet<IndexResource>,
    deadline_exhausted: bool,
}

impl IndexBudgetTracker {
    pub fn new(budget: IndexBudget) -> Self {
        Self {
            budget,
            started: Instant::now(),
            consumed: BTreeMap::new(),
            exhausted: BTreeSet::new(),
            deadline_exhausted: false,
        }
    }

    pub fn budget(&self) -> &IndexBudget {
        &self.budget
    }

    pub fn consume(
        &mut self,
        resource: IndexResource,
        amount: usize,
    ) -> Result<(), IndexBudgetExhaustion> {
        let limit = self.limit(resource);
        let consumed = self.consumed.get(&resource).copied().unwrap_or(0);
        let Some(next) = consumed.checked_add(amount) else {
            self.exhausted.insert(resource);
            return Err(index_resource_exhaustion(resource));
        };
        if next > limit {
            self.exhausted.insert(resource);
            return Err(index_resource_exhaustion(resource));
        }
        self.consumed.insert(resource, next);
        Ok(())
    }

    pub fn observe(
        &mut self,
        resource: IndexResource,
        observed: usize,
    ) -> Result<(), IndexBudgetExhaustion> {
        let limit = self.limit(resource);
        let previous = self.consumed.get(&resource).copied().unwrap_or(0);
        self.consumed
            .insert(resource, previous.max(observed.min(limit)));
        if observed > limit {
            self.exhausted.insert(resource);
            return Err(index_resource_exhaustion(resource));
        }
        Ok(())
    }

    pub fn amount(&self, resource: IndexResource) -> IndexBudgetAmount {
        let initial = self.limit(resource);
        let consumed = self
            .consumed
            .get(&resource)
            .copied()
            .unwrap_or(0)
            .min(initial);
        IndexBudgetAmount {
            initial,
            consumed,
            remaining: initial.saturating_sub(consumed),
            exhausted: self.exhausted.contains(&resource),
        }
    }

    pub fn check_deadline(&mut self) -> Result<(), IndexBudgetExhaustion> {
        if self.deadline_exhausted || self.started.elapsed() >= self.budget.deadline {
            self.deadline_exhausted = true;
            return Err(IndexBudgetExhaustion {
                resource: None,
                code: "index-deadline-exhausted",
            });
        }
        Ok(())
    }

    pub fn remaining_deadline(&self) -> Duration {
        self.budget.deadline.saturating_sub(self.started.elapsed())
    }

    fn limit(&self, resource: IndexResource) -> usize {
        match resource {
            IndexResource::ManifestFiles => self.budget.max_manifest_files,
            IndexResource::ManifestBytes => self.budget.max_manifest_bytes,
            IndexResource::ProjectModelFiles => self.budget.max_project_model_files,
            IndexResource::ProjectModelBytes => self.budget.max_project_model_bytes,
            IndexResource::FileBytes => self.budget.max_file_bytes,
            IndexResource::ParseBytes => self.budget.max_parse_bytes,
            IndexResource::Nodes => self.budget.max_nodes,
            IndexResource::Facts => self.budget.max_facts,
            IndexResource::Symbols => self.budget.max_symbols,
            IndexResource::Edges => self.budget.max_edges,
            IndexResource::GenerationBytes => self.budget.max_generation_bytes,
            IndexResource::OverlayPaths => self.budget.max_overlay_paths,
            IndexResource::QueryRows => self.budget.max_query_rows,
            IndexResource::GraphDepth => self.budget.max_graph_depth,
        }
    }
}

fn index_resource_exhaustion(resource: IndexResource) -> IndexBudgetExhaustion {
    IndexBudgetExhaustion {
        resource: Some(resource),
        code: resource.exhaustion_code(),
    }
}
