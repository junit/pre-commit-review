# SQLite Storage Spike Fixture

The spike generates its graph entirely from numeric command arguments. It does
not read repository source, manifests, Git objects, or working-tree files.

Schema version: `1`.

For zero-based `index`:

- symbol id: `symbol-{index:08}`;
- symbol path: `src/module-{index % 128:03}.rs`;
- symbol range: one-based line `index + 1`;
- edge id: `edge-{index:08}`;
- edge source: symbol `index % symbols`;
- edge target: symbol `(index * 17 + 1) % symbols`.

The generation key binds the schema identifier, symbol count, edge count, and
the complete deterministic row stream. The application root uses the same row
stream with a distinct domain separator.

Hard input limits:

- symbols: `1..=2_000_000`;
- edges: `0..=5_000_000`;
- query depth: `1..=2`;
- returned query edges: `1..=10_000`.
