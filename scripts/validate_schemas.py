import argparse
import json
import pathlib
import sys

import jsonschema


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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        '--control-plane-output',
        action='append',
        default=[],
        help='validate one helper output against the control-plane schema and semantic invariants',
    )
    args = parser.parse_args()
    schema_dir = pathlib.Path('collect-diff-context-cli/schemas')
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

if __name__ == '__main__':
    main()
