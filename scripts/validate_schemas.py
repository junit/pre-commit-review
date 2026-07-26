import argparse
import hashlib
import json
import pathlib
import sys

try:
    import jsonschema
    from referencing import Registry, Resource
except ModuleNotFoundError:
    print(
        "validate_schemas: Python package 'jsonschema' is required; "
        "install it with 'python3 -m pip install jsonschema'",
        file=sys.stderr,
    )
    raise SystemExit(2)


def load_control_plane_output(path):
    lines = pathlib.Path(path).read_text(encoding='utf-8').splitlines()
    try:
        marker = lines.index('## Review Control Plane JSON')
    except ValueError as exc:
        raise ValueError('missing Review Control Plane JSON section') from exc
    payload_lines = [line for line in lines[marker + 1:] if line.strip()]
    if len(payload_lines) != 1:
        raise ValueError('control-plane section must contain exactly one compact JSON value')
    return json.loads(payload_lines[0])


def load_static_evidence_output(path):
    lines = pathlib.Path(path).read_text(encoding='utf-8').splitlines()
    try:
        marker = lines.index('## Static Analysis Evidence JSON')
    except ValueError as exc:
        raise ValueError('missing Static Analysis Evidence JSON section') from exc
    payload_lines = [line for line in lines[marker + 1:] if line.strip()]
    if len(payload_lines) != 1:
        raise ValueError('static-evidence section must contain exactly one compact JSON value')
    return json.loads(payload_lines[0])


def load_static_execution_output(path):
    lines = pathlib.Path(path).read_text(encoding='utf-8').splitlines()
    try:
        marker = lines.index('## Static Analysis Execution JSON')
    except ValueError as exc:
        raise ValueError('missing Static Analysis Execution JSON section') from exc
    try:
        payload_line = next(line for line in lines[marker + 1:] if line.strip())
    except StopIteration as exc:
        raise ValueError('static-execution section has no JSON value') from exc
    return json.loads(payload_line)


def load_static_orchestration_output(path):
    lines = pathlib.Path(path).read_text(encoding='utf-8').splitlines()
    try:
        marker = lines.index('## Static Analysis Orchestration JSON')
    except ValueError as exc:
        raise ValueError('missing Static Analysis Orchestration JSON section') from exc
    try:
        payload_line = next(line for line in lines[marker + 1:] if line.strip())
    except StopIteration as exc:
        raise ValueError('static-orchestration section has no JSON value') from exc
    return json.loads(payload_line)


def load_impact_context_output(path):
    lines = pathlib.Path(path).read_text(encoding='utf-8').splitlines()
    try:
        marker = lines.index('## Impact Context JSON')
    except ValueError as exc:
        raise ValueError('missing Impact Context JSON section') from exc
    payload_lines = [line for line in lines[marker + 1:] if line.strip()]
    if len(payload_lines) != 1:
        raise ValueError('impact-context section must contain exactly one compact JSON value')
    return json.loads(payload_lines[0])


def load_schema_bundle(schema_dir):
    schemas = {}
    resources = []
    for schema_path in sorted(schema_dir.glob('*.schema.json')):
        schema = json.loads(schema_path.read_text(encoding='utf-8'))
        schemas[schema_path.name] = schema
        resources.append((schema_path.name, Resource.from_contents(schema)))
        if schema.get('$id') and schema['$id'] != schema_path.name:
            resources.append((schema['$id'], Resource.from_contents(schema)))
    return schemas, Registry().with_resources(resources)

def validate_control_plane_invariants(payload):
    if not payload.get('authoritative'):
        return
    units = payload['units']
    groups = payload['groups']
    if payload['counts']['units'] != len(units):
        raise ValueError('counts.units does not match units length')
    if payload['counts']['groups'] != len(groups):
        raise ValueError('counts.groups does not match groups length')
    expected_counts = {
        'additions': sum(unit[2] for unit in units),
        'deletions': sum(unit[3] for unit in units),
        'diff_bytes': sum(unit[4] for unit in units),
        'high_risk_units': sum(unit[5] == 'high-risk' for unit in units),
        'split_required_groups': sum(group[4] == 'split-required' for group in groups),
    }
    for name, expected in expected_counts.items():
        if payload['counts'][name] != expected:
            raise ValueError(f'counts.{name} does not match compact tuples')
    fingerprint = payload['scope_fingerprint']
    if payload['collection'] != {'start': fingerprint, 'end': fingerprint}:
        raise ValueError('authoritative collection fingerprints must equal scope_fingerprint')
    group_ids = {group[0] for group in groups}
    if len(group_ids) != len(groups):
        raise ValueError('group identifiers must be unique')
    covered_indexes = []
    for group in groups:
        indexes = group[5]
        if any(index < 0 or index >= len(units) for index in indexes):
            raise ValueError(f'group {group[0]} contains an out-of-range unit index')
        if len(indexes) != len(set(indexes)):
            raise ValueError(f'group {group[0]} contains duplicate unit indexes')
        if any(units[index][6] != group[0] for index in indexes):
            raise ValueError(f'group {group[0]} points at a unit owned by another group')
        if group[3] != sum(units[index][4] for index in indexes):
            raise ValueError(f'group {group[0]} diff_bytes does not match its units')
        covered_indexes.extend(indexes)
    if sorted(covered_indexes) != list(range(len(units))):
        raise ValueError('groups must partition every unit exactly once')
    work_ids = [item[1] for item in payload['work_order']]
    if len(work_ids) != len(set(work_ids)) or set(work_ids) != group_ids:
        raise ValueError('work_order must contain every group exactly once')
    expected_work_order = []
    for group_id, risk, _, _, budget_status, _ in groups:
        if budget_status == 'split-required':
            priority, action = 1, 'split'
        elif risk == 'high':
            priority, action = 2, 'review'
        elif risk == 'consistency':
            priority, action = 3, 'review'
        else:
            priority, action = 4, 'review'
        expected_work_order.append([priority, group_id, action])
    expected_work_order.sort(key=lambda item: (item[0], item[1]))
    if payload['work_order'] != expected_work_order:
        raise ValueError('work_order priorities or ordering do not match group risk and budget')


def validate_static_evidence_invariants(payload):
    findings = payload['findings']
    counts = payload['counts']
    if payload['truncated']:
        if len(findings) >= counts['deduplicated_findings']:
            raise ValueError('truncated evidence must omit at least one deduplicated finding')
    elif len(findings) != counts['deduplicated_findings']:
        raise ValueError('untruncated evidence must emit every deduplicated finding')
    expected_visible = {
        'mapped_to_units': sum(item['manifest_unit_id'] is not None for item in findings),
        'added_line': sum(item['line_scope'] == 'added' for item in findings),
        'blocking_candidates': sum(item['disposition'] == 'blocking-candidate' for item in findings),
        'priority_candidates': sum(item['disposition'] == 'priority-candidate' for item in findings),
        'notes': sum(item['disposition'] == 'note' for item in findings),
        'outside_scope': sum(item['disposition'] == 'outside-scope' for item in findings),
    }
    if not payload['truncated']:
        for name, expected in expected_visible.items():
            if counts[name] != expected:
                raise ValueError(f'counts.{name} does not match emitted findings')
    if counts['reports'] != len(payload['reports']):
        raise ValueError('counts.reports does not match reports length')
    if counts['input_findings'] != sum(report['finding_count'] for report in payload['reports']):
        raise ValueError('counts.input_findings does not match report finding counts')
    if counts['deduplicated_findings'] > counts['input_findings']:
        raise ValueError('deduplicated findings cannot exceed input findings')
    disposition_total = (
        counts['blocking_candidates']
        + counts['priority_candidates']
        + counts['notes']
        + counts['outside_scope']
    )
    if disposition_total != counts['deduplicated_findings']:
        raise ValueError('finding disposition counts must cover every deduplicated finding')
    if counts['mapped_to_units'] + counts['outside_scope'] != counts['deduplicated_findings']:
        raise ValueError('mapped and outside-scope counts must partition deduplicated findings')
    if counts['added_line'] > counts['mapped_to_units']:
        raise ValueError('added-line findings must map to manifest units')
    report_ids = {report['report_id'] for report in payload['reports']}
    if len(report_ids) != len(payload['reports']):
        raise ValueError('report identifiers must be unique')
    finding_ids = {finding['finding_id'] for finding in findings}
    if len(finding_ids) != len(findings):
        raise ValueError('finding identifiers must be unique')
    if any(not set(item['report_ids']).issubset(report_ids) for item in findings):
        raise ValueError('finding references an unknown report identifier')
    if any(item['blocking_candidate'] != (item['disposition'] == 'blocking-candidate') for item in findings):
        raise ValueError('blocking_candidate must match blocking-candidate disposition')
    for report in payload['reports']:
        if report['trust'] == 'controlled-execution':
            if report['execution_id'] is None:
                raise ValueError('controlled execution report must carry execution_id')
            if report['scope_binding'] != 'controlled-execution':
                raise ValueError('controlled execution report must use controlled scope binding')
        else:
            if report['execution_id'] is not None:
                raise ValueError('explicit input report cannot carry execution_id')
            if report['scope_binding'] == 'controlled-execution':
                raise ValueError('explicit input report cannot use controlled scope binding')


def validate_static_execution_record(payload):
    execution = payload['execution']
    expected_execution_id_digest = hashlib.sha256()
    for value in (
        payload['scope']['fingerprint'],
        payload['profile']['sha256'],
        payload['executable']['sha256'],
        execution['stdout_sha256'],
        execution['status'],
    ):
        expected_execution_id_digest.update(str(value).encode('utf-8', errors='replace'))
        expected_execution_id_digest.update(b'\0')
    if payload['execution_id'] != expected_execution_id_digest.hexdigest()[:16]:
        raise ValueError('execution_id does not match controlled execution provenance')
    if payload['profile']['profile_id'] != payload['profile']['sha256'][:16]:
        raise ValueError('profile_id must be derived from the authorized profile SHA256')
    limits = payload['profile']['limits']
    if payload['snapshot']['files'] > limits['max_snapshot_files']:
        raise ValueError('snapshot files exceed the authorized profile limit')
    if payload['snapshot']['bytes'] > limits['max_snapshot_bytes']:
        raise ValueError('snapshot bytes exceed the authorized profile limit')
    stream_sizes = (execution['stdout_bytes'], execution['stderr_bytes'])
    if any(size > limits['max_output_bytes'] + 1 for size in stream_sizes):
        raise ValueError('captured process output exceeds the bounded limit-plus-one prefix')
    if execution['status'] == 'completed':
        if not execution['result_accepted'] or execution['failure_reason'] is not None:
            raise ValueError('completed execution must have an accepted result and no failure reason')
        if execution['exit_code'] not in payload['profile']['success_exit_codes']:
            raise ValueError('completed execution exit code is not authorized by the profile')
        if max(stream_sizes) > limits['max_output_bytes']:
            raise ValueError('completed execution exceeds the authorized output limit')
    else:
        if execution['result_accepted'] or execution['failure_reason'] is None:
            raise ValueError('incomplete execution must reject its result with a failure reason')
        expected_reason = {
            'failed': 'non-success-exit',
            'timeout': 'timeout',
            'output-limit': 'output-limit',
            'invalid-output': 'invalid-output',
        }[execution['status']]
        if execution['failure_reason'] != expected_reason:
            raise ValueError('failure_reason must match the controlled execution status')
        if execution['status'] in {'timeout', 'output-limit'} and execution['exit_code'] is not None:
            raise ValueError('timeout and output-limit execution must not claim an exit code')
        if execution['status'] == 'failed' and execution['exit_code'] in payload['profile']['success_exit_codes']:
            raise ValueError('failed execution cannot carry an authorized success exit code')
        if execution['status'] == 'output-limit' and not any(
            size == limits['max_output_bytes'] + 1 for size in stream_sizes
        ):
            raise ValueError('output-limit execution must retain exactly one sentinel byte')


def _require_sorted_unique(values, label):
    if values != sorted(values) or len(values) != len(set(values)):
        raise ValueError(f'{label} must be unique and sorted')


def _reject_authority_fields(value):
    banned = {'reviewed_units', 'verdict', 'blocking_candidate'}
    if isinstance(value, dict):
        overlap = banned.intersection(value)
        if overlap:
            raise ValueError(f'impact context contains forbidden authority field: {sorted(overlap)[0]}')
        for child in value.values():
            _reject_authority_fields(child)
    elif isinstance(value, list):
        for child in value:
            _reject_authority_fields(child)


def validate_impact_context_invariants(payload):
    _reject_authority_fields(payload)
    providers = payload['providers']
    units = payload['units']
    symbols = payload['changed_symbols']
    edges = payload['impact_edges']
    summaries = payload['domain_summaries']
    limitations = payload['limitations']
    coverage = payload['coverage']
    metrics = payload['metrics']

    provider_ids = [item['provider_id'] for item in providers]
    symbol_ids = [item['symbol_id'] for item in symbols]
    edge_ids = [item['edge_id'] for item in edges]
    summary_ids = [item['summary_id'] for item in summaries]
    limitation_ids = [item['limitation_id'] for item in limitations]
    for values, label in (
        (provider_ids, 'provider ids'),
        (symbol_ids, 'symbol ids'),
        (edge_ids, 'edge ids'),
        (summary_ids, 'summary ids'),
        (limitation_ids, 'limitation ids'),
    ):
        _require_sorted_unique(values, label)

    provider_by_id = {item['provider_id']: item for item in providers}
    symbol_by_id = {item['symbol_id']: item for item in symbols}
    limitation_id_set = set(limitation_ids)
    unit_by_path = {item['path']: item for item in units}
    if len(unit_by_path) != len(units):
        raise ValueError('impact unit paths must be unique')
    manifest_ids = [item['manifest_unit_id'] for item in units]
    if len(manifest_ids) != len(set(manifest_ids)):
        raise ValueError('manifest unit identifiers must be unique')
    if any(not item.startswith('file:') for item in manifest_ids):
        raise ValueError('every impact unit must map to a changed file manifest unit')

    changed = coverage['changed_candidate_files']
    total = coverage['total_candidate_files']
    eligible = coverage['syntax_eligible_files']
    parsed = coverage['parsed_files']
    if not (len(units) == changed <= total):
        raise ValueError('candidate file coverage is not monotonic')
    if not (parsed <= eligible <= changed):
        raise ValueError('syntax coverage is not monotonic')
    if (
        coverage['clean_parse_files']
        + coverage['recovered_parse_files']
        + coverage['degraded_parse_files']
        != parsed
    ):
        raise ValueError('parse quality counts must partition parsed files')
    if (
        parsed
        + coverage['unsupported_files']
        + coverage['resource_limited_files']
        + coverage['unavailable_files']
        != changed
    ):
        raise ValueError('syntax terminal counts must partition changed files')
    if coverage['reached_graph_depth'] > coverage['requested_graph_depth']:
        raise ValueError('reached graph depth exceeds requested depth')

    for provider in providers:
        _require_sorted_unique(provider['limitation_ids'], 'provider limitation ids')
        if not set(provider['limitation_ids']).issubset(limitation_id_set):
            raise ValueError('provider references an unknown limitation')
    for unit in units:
        _require_sorted_unique(unit['provider_ids'], 'unit provider ids')
        _require_sorted_unique(unit['changed_symbol_ids'], 'unit changed symbol ids')
        _require_sorted_unique(unit['parse_affected_symbol_ids'], 'unit parse affected symbol ids')
        _require_sorted_unique(unit['limitation_ids'], 'unit limitation ids')
        if not set(unit['provider_ids']).issubset(provider_by_id):
            raise ValueError('unit references an unknown provider')
        if not set(unit['changed_symbol_ids']).issubset(symbol_by_id):
            raise ValueError('unit references an unknown changed symbol')
        if not set(unit['parse_affected_symbol_ids']).issubset(symbol_by_id):
            raise ValueError('unit references an unknown parse-affected symbol')
        if not set(unit['limitation_ids']).issubset(limitation_id_set):
            raise ValueError('unit references an unknown limitation')
        if any(symbol_by_id[symbol_id]['path'] != unit['path'] for symbol_id in unit['changed_symbol_ids']):
            raise ValueError('unit references a changed symbol from another path')

    for symbol in symbols:
        if symbol['path'] not in unit_by_path:
            raise ValueError('changed symbol path has no impact unit')
        if symbol['provider_id'] not in provider_by_id:
            raise ValueError('changed symbol references an unknown provider')
    forbidden_resolution = {'resolved-reference', 'semantic', 'polymorphic-candidate'}
    for edge in edges:
        if edge['path'] not in unit_by_path:
            raise ValueError('impact edge path has no impact unit')
        provider = provider_by_id.get(edge['provider_id'])
        if provider is None:
            raise ValueError('impact edge references an unknown provider')
        if edge['to_symbol'] is None and edge['unresolved_target'] is None:
            raise ValueError('impact edge has no target')
        if edge['to_symbol'] is not None and edge['to_symbol'] not in symbol_by_id:
            raise ValueError('impact edge references an unknown target symbol')
        if provider['provider_kind'] == 'text-adapter':
            raise ValueError('text-adapter cannot emit symbol edges')
        if provider['provider_kind'] == 'tree-sitter-rust' and edge['resolution'] in forbidden_resolution:
            raise ValueError('tree-sitter-rust cannot claim resolved semantics')
    for summary in summaries:
        if summary['path'] not in unit_by_path:
            raise ValueError('domain summary path has no impact unit')
        if summary['symbol_id'] is not None and summary['symbol_id'] not in symbol_by_id:
            raise ValueError('domain summary references an unknown symbol')
        _require_sorted_unique(summary['evidence_fact_ids'], 'summary evidence fact ids')

    output_limitations = [item for item in limitations if item['code'] == 'output-truncated']
    if coverage['output_truncated'] != bool(output_limitations):
        raise ValueError('output truncation coverage and limitation disagree')
    if not coverage['output_truncated']:
        if metrics['edges_emitted'] != len(edges):
            raise ValueError('metrics.edges_emitted does not match impact edges')
        if metrics['summaries_emitted'] != len(summaries):
            raise ValueError('metrics.summaries_emitted does not match domain summaries')


def validate_static_execution_invariants(payload, evidence):
    if payload['scope'] != evidence['scope']:
        raise ValueError('execution and evidence scopes must match')
    report_ids = sorted(report['report_id'] for report in evidence['reports'])
    if sorted(payload['evidence']['report_ids']) != report_ids:
        raise ValueError('execution evidence report_ids do not match emitted reports')
    for report in evidence['reports']:
        if report['trust'] != 'controlled-execution':
            raise ValueError('execution output contains evidence without controlled trust')
        if report['scope_binding'] != 'controlled-execution':
            raise ValueError('execution output contains evidence without controlled scope binding')
        if report['execution_id'] != payload['execution_id']:
            raise ValueError('execution_id does not link every evidence report')
        if report['tool'] != payload['tool']:
            raise ValueError('execution tool identity does not match linked evidence')
    validate_static_execution_record(payload)
    execution = payload['execution']
    if execution['status'] == 'completed':
        if any(report['status'] != 'completed' for report in evidence['reports']):
            raise ValueError('completed execution requires completed evidence reports')
        if any(report['format'] != payload['profile']['output_format'] for report in evidence['reports']):
            raise ValueError('completed evidence format does not match the authorized profile')
    else:
        if evidence['counts']['blocking_candidates'] != 0:
            raise ValueError('incomplete execution evidence cannot contain blocking candidates')
        if any(report['status'] == 'completed' for report in evidence['reports']):
            raise ValueError('incomplete execution cannot emit completed evidence reports')


def validate_static_orchestration_manifest_invariants(payload):
    profile_ids = [item['profile_id'] for item in payload['profiles']]
    if len(profile_ids) != len(set(profile_ids)):
        raise ValueError('orchestration manifest profile_id values must be unique')
    path_hash_pairs = [(item['path'], item['sha256']) for item in payload['profiles']]
    if len(path_hash_pairs) != len(set(path_hash_pairs)):
        raise ValueError('orchestration manifest path/hash pairs must be unique')
    for item in payload['profiles']:
        if not pathlib.Path(item['path']).is_absolute():
            raise ValueError('orchestration manifest profile paths must be absolute')


def validate_budget_amount(name, amount):
    if amount['consumed'] + amount['remaining'] != amount['initial']:
        raise ValueError(f'orchestration budget {name} does not balance')


def validate_static_orchestration_invariants(payload, evidence):
    if payload['scope'] != evidence['scope']:
        raise ValueError('orchestration and evidence scopes must match')

    reports = evidence['reports']
    findings = evidence['findings']
    reports_by_id = {report['report_id']: report for report in reports}
    report_ids = set(reports_by_id)
    finding_ids = {finding['finding_id'] for finding in findings}
    if set(payload['report_ids']) != report_ids:
        raise ValueError('orchestration report_ids do not match combined evidence')
    if set(payload['finding_ids']) != finding_ids:
        raise ValueError('orchestration finding_ids do not match combined evidence')

    executed = [run for run in payload['runs'] if run['run_kind'] == 'executed']
    if executed and not reports:
        raise ValueError('executed orchestration runs require combined evidence reports')
    if not executed and reports:
        raise ValueError('orchestration without executed runs cannot contain reports')

    claimed_report_ids = set()
    incomplete_report_ids = set()
    accepted = 0
    execution_millis = 0
    captured_output_bytes = 0
    shared_snapshot = payload['snapshot']
    for run in executed:
        execution = run['execution']
        process = execution['execution']
        validate_static_execution_record(execution)
        if execution['scope'] != payload['scope']:
            raise ValueError('executed run scope does not match orchestration scope')
        for key in ('kind', 'sha256', 'files', 'bytes'):
            if execution['snapshot'][key] != shared_snapshot[key]:
                raise ValueError('executed run does not use the shared orchestration snapshot')

        run_report_ids = set(execution['evidence']['report_ids'])
        if not run_report_ids:
            raise ValueError('executed run must expose at least one evidence report id')
        if not run_report_ids.issubset(report_ids):
            raise ValueError('executed run references a report absent from combined evidence')
        if claimed_report_ids.intersection(run_report_ids):
            raise ValueError('combined evidence report is claimed by multiple executed runs')
        claimed_report_ids.update(run_report_ids)

        linked_reports = [reports_by_id[report_id] for report_id in run_report_ids]
        if any(report['execution_id'] != execution['execution_id'] for report in linked_reports):
            raise ValueError('executed run report execution_id linkage is inconsistent')
        if any(report['tool'] != execution['tool'] for report in linked_reports):
            raise ValueError('executed run report tool identity is inconsistent')
        if any(report['trust'] != 'controlled-execution' for report in linked_reports):
            raise ValueError('orchestration reports must use controlled-execution trust')
        if any(report['scope_binding'] != 'controlled-execution' for report in linked_reports):
            raise ValueError('orchestration reports must use controlled-execution scope binding')
        if any(report['status'] != process['status'] for report in linked_reports):
            raise ValueError('executed run status does not match its combined evidence reports')

        if process['status'] == 'completed':
            if not process['result_accepted']:
                raise ValueError('completed orchestration run must accept its result')
            accepted += 1
        else:
            if process['result_accepted']:
                raise ValueError('incomplete orchestration run cannot accept its result')
            incomplete_report_ids.update(run_report_ids)

        execution_millis += process['duration_ms']
        captured_output_bytes += process['stdout_bytes'] + process['stderr_bytes']

    if claimed_report_ids != report_ids:
        raise ValueError('combined evidence contains reports not owned by an executed run')
    for finding in findings:
        linked_ids = set(finding['report_ids'])
        if linked_ids.intersection(incomplete_report_ids) and finding['blocking_candidate']:
            raise ValueError('incomplete orchestration reports cannot support blocking candidates')

    expected_status = 'completed' if accepted == len(payload['runs']) else (
        'partial' if accepted else 'failed'
    )
    if payload['status'] != expected_status:
        raise ValueError('orchestration status does not match run terminal states')

    for name, amount in payload['budgets'].items():
        validate_budget_amount(name, amount)
    expected_budget_consumption = {
        'execution_millis': execution_millis,
        'captured_output_bytes': captured_output_bytes,
        'findings': evidence['counts']['deduplicated_findings'],
        'snapshot_files': shared_snapshot['files'],
        'snapshot_bytes': shared_snapshot['bytes'],
    }
    for name, consumed in expected_budget_consumption.items():
        amount = payload['budgets'][name]
        if amount['consumed'] != min(consumed, amount['initial']):
            raise ValueError(f'orchestration budget {name} consumption is inconsistent')

    if payload['manifest']['manifest_id'] != payload['manifest']['sha256'][:16]:
        raise ValueError('manifest_id must be derived from the authorized manifest SHA256')
    if shared_snapshot['snapshot_id'] != shared_snapshot['sha256'][:16]:
        raise ValueError('snapshot_id must be derived from the shared snapshot SHA256')

    orchestration_digest = hashlib.sha256()
    for value in (
        payload['scope']['fingerprint'],
        payload['manifest']['sha256'],
        shared_snapshot['sha256'],
    ):
        orchestration_digest.update(value.encode('utf-8'))
        orchestration_digest.update(b'\0')
    for run in payload['runs']:
        if run['run_kind'] == 'executed':
            terminal = 'executed'
            execution_id = run['execution']['execution_id']
        elif run['run_kind'] == 'invalidated':
            terminal = f"invalidated/{run['reason']}"
            execution_id = ''
        else:
            terminal = f"not-run/{run['reason']}"
            execution_id = ''
        for value in (run['profile_id'], terminal, execution_id):
            orchestration_digest.update(value.encode('utf-8'))
            orchestration_digest.update(b'\0')
    if payload['orchestration_id'] != orchestration_digest.hexdigest()[:16]:
        raise ValueError('orchestration_id does not match scope, authorization, and run states')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        '--control-plane-output',
        action='append',
        default=[],
        help='validate one helper output against the control-plane schema and semantic invariants',
    )
    parser.add_argument(
        '--static-evidence-output',
        action='append',
        default=[],
        help='validate one static-evidence output against its schema and semantic invariants',
    )
    parser.add_argument(
        '--static-execution-output',
        action='append',
        default=[],
        help='validate one controlled execution output and its linked static evidence',
    )
    parser.add_argument(
        '--static-profile',
        action='append',
        default=[],
        help='validate one static_analysis_profile/v1 JSON file',
    )
    parser.add_argument(
        '--static-orchestration-manifest',
        action='append',
        default=[],
        help='validate one static-analysis orchestration manifest JSON file',
    )
    parser.add_argument(
        '--static-orchestration-output',
        action='append',
        default=[],
        help='validate one orchestration output and its combined static evidence',
    )
    parser.add_argument(
        '--impact-context-output',
        action='append',
        default=[],
        help='validate one impact_context/v1 output and semantic invariants',
    )
    args = parser.parse_args()
    skill_root = pathlib.Path(__file__).resolve().parent.parent
    schema_dir = skill_root / 'collect-diff-context-cli/schemas'
    errors = 0
    schema_files = sorted(schema_dir.glob('*.schema.json'))
    for schema_file in schema_files:
        try:
            schema = json.loads(schema_file.read_text())
            jsonschema.Draft202012Validator.check_schema(schema)
            print(f'  ✅ {schema_file.name}: valid schema')
        except Exception as e:
            print(f'  ❌ {schema_file.name}: {e}', file=sys.stderr)
            errors += 1
    if errors:
        sys.exit(1)
    print(f'All {len(schema_files)} schemas validated.')
    schemas, schema_registry = load_schema_bundle(schema_dir)
    if args.control_plane_output:
        schema = json.loads((schema_dir / 'review-control-plane.schema.json').read_text())
        validator = jsonschema.Draft202012Validator(schema)
        for output_path in args.control_plane_output:
            try:
                payload = load_control_plane_output(output_path)
                validator.validate(payload)
                validate_control_plane_invariants(payload)
                print(f'  ✅ {output_path}: valid control-plane instance')
            except Exception as exc:
                print(f'  ❌ {output_path}: {exc}', file=sys.stderr)
                errors += 1
        if errors:
            sys.exit(1)
    if args.static_evidence_output:
        schema = json.loads((schema_dir / 'static-analysis-evidence.schema.json').read_text())
        validator = jsonschema.Draft202012Validator(schema)
        for output_path in args.static_evidence_output:
            try:
                payload = load_static_evidence_output(output_path)
                validator.validate(payload)
                validate_static_evidence_invariants(payload)
                print(f'  ✅ {output_path}: valid static-evidence instance')
            except Exception as exc:
                print(f'  ❌ {output_path}: {exc}', file=sys.stderr)
                errors += 1
        if errors:
            sys.exit(1)
    if args.static_execution_output:
        execution_schema = json.loads((schema_dir / 'static-analysis-execution.schema.json').read_text())
        evidence_schema = json.loads((schema_dir / 'static-analysis-evidence.schema.json').read_text())
        execution_validator = jsonschema.Draft202012Validator(execution_schema)
        evidence_validator = jsonschema.Draft202012Validator(evidence_schema)
        for output_path in args.static_execution_output:
            try:
                payload = load_static_execution_output(output_path)
                evidence = load_static_evidence_output(output_path)
                execution_validator.validate(payload)
                evidence_validator.validate(evidence)
                validate_static_evidence_invariants(evidence)
                validate_static_execution_invariants(payload, evidence)
                print(f'  ✅ {output_path}: valid static-execution instance')
            except Exception as exc:
                print(f'  ❌ {output_path}: {exc}', file=sys.stderr)
                errors += 1
        if errors:
            sys.exit(1)
    if args.static_profile:
        profile_schema = schemas['static-analysis-profile.schema.json']
        profile_validator = jsonschema.Draft202012Validator(profile_schema)
        for profile_path in args.static_profile:
            try:
                payload = json.loads(pathlib.Path(profile_path).read_text(encoding='utf-8'))
                profile_validator.validate(payload)
                print(f'  ✅ {profile_path}: valid static-analysis profile')
            except Exception as exc:
                print(f'  ❌ {profile_path}: {exc}', file=sys.stderr)
                errors += 1
        if errors:
            sys.exit(1)
    if args.static_orchestration_manifest:
        manifest_schema = schemas['static-analysis-orchestration-manifest.schema.json']
        manifest_validator = jsonschema.Draft202012Validator(manifest_schema)
        for manifest_path in args.static_orchestration_manifest:
            try:
                payload = json.loads(pathlib.Path(manifest_path).read_text(encoding='utf-8'))
                manifest_validator.validate(payload)
                validate_static_orchestration_manifest_invariants(payload)
                print(f'  ✅ {manifest_path}: valid static-analysis orchestration manifest')
            except Exception as exc:
                print(f'  ❌ {manifest_path}: {exc}', file=sys.stderr)
                errors += 1
        if errors:
            sys.exit(1)
    if args.static_orchestration_output:
        orchestration_schema = schemas['static-analysis-orchestration.schema.json']
        evidence_schema = schemas['static-analysis-evidence.schema.json']
        orchestration_validator = jsonschema.Draft202012Validator(
            orchestration_schema,
            registry=schema_registry,
        )
        evidence_validator = jsonschema.Draft202012Validator(evidence_schema)
        for output_path in args.static_orchestration_output:
            try:
                payload = load_static_orchestration_output(output_path)
                evidence = load_static_evidence_output(output_path)
                orchestration_validator.validate(payload)
                evidence_validator.validate(evidence)
                validate_static_evidence_invariants(evidence)
                validate_static_orchestration_invariants(payload, evidence)
                print(f'  ✅ {output_path}: valid static-analysis orchestration output')
            except Exception as exc:
                print(f'  ❌ {output_path}: {exc}', file=sys.stderr)
                errors += 1
        if errors:
            sys.exit(1)
    if args.impact_context_output:
        impact_schema = schemas['impact-context.schema.json']
        impact_validator = jsonschema.Draft202012Validator(impact_schema)
        for output_path in args.impact_context_output:
            try:
                payload = load_impact_context_output(output_path)
                impact_validator.validate(payload)
                validate_impact_context_invariants(payload)
                print(f'  ✅ {output_path}: valid impact-context instance')
            except Exception as exc:
                print(f'  ❌ {output_path}: {exc}', file=sys.stderr)
                errors += 1
        if errors:
            sys.exit(1)

if __name__ == '__main__':
    main()
