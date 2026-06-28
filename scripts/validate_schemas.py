#!/usr/bin/env python3
import json
import pathlib
import sys
import jsonschema

def main():
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

if __name__ == '__main__':
    main()
