import argparse
import hashlib
import json
import pathlib
import sys

try:
    import jsonschema
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
        if any(report['status'] != 'completed' for report in evidence['reports']):
            raise ValueError('completed execution requires completed evidence reports')
        if execution['exit_code'] not in payload['profile']['success_exit_codes']:
            raise ValueError('completed execution exit code is not authorized by the profile')
        if max(execution['stdout_bytes'], execution['stderr_bytes']) > limits['max_output_bytes']:
            raise ValueError('completed execution exceeds the authorized output limit')
        if any(report['format'] != payload['profile']['output_format'] for report in evidence['reports']):
            raise ValueError('completed evidence format does not match the authorized profile')
    else:
        if execution['result_accepted'] or execution['failure_reason'] is None:
            raise ValueError('incomplete execution must reject its result with a failure reason')
        if evidence['counts']['blocking_candidates'] != 0:
            raise ValueError('incomplete execution evidence cannot contain blocking candidates')
        if any(report['status'] == 'completed' for report in evidence['reports']):
            raise ValueError('incomplete execution cannot emit completed evidence reports')
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
        profile_schema = json.loads((schema_dir / 'static-analysis-profile.schema.json').read_text())
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

if __name__ == '__main__':
    main()
