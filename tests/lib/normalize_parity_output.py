#!/usr/bin/env python3
import json
import sys


def normalize_json_buffer(json_buffer):
    text = "".join(json_buffer).strip()
    if not text:
        return []
    try:
        data = json.loads(text)
        return [json.dumps(data, indent=2, sort_keys=True) + "\n"]
    except Exception:
        pass

    decoder = json.JSONDecoder()
    pos = 0
    objects = []
    while pos < len(text):
        while pos < len(text) and text[pos].isspace():
            pos += 1
        if pos >= len(text):
            break
        try:
            obj, idx = decoder.raw_decode(text, pos)
            objects.append(obj)
            pos = idx
        except Exception:
            return json_buffer

    def get_sort_key(obj):
        if isinstance(obj, dict):
            if "group_id" in obj:
                return ("group_id", obj["group_id"])
            if "unit_id" in obj:
                return ("unit_id", obj["unit_id"])
        return ("str", json.dumps(obj, sort_keys=True))

    objects.sort(key=get_sort_key)
    normalized = []
    for obj in objects:
        normalized.append(json.dumps(obj, indent=2, sort_keys=True) + "\n")
    return normalized


def main():
    lines = sys.stdin.readlines()
    output = []
    in_json = False
    json_buffer = []

    for line in lines:
        is_header = line.startswith("## ")
        is_json_header = is_header and (
            line.strip().endswith("JSON")
            or line.strip().endswith("Template")
            or line.strip().endswith("JSONL")
        )
        if is_json_header:
            if json_buffer:
                output.extend(normalize_json_buffer(json_buffer))
                json_buffer = []
            output.append(line)
            in_json = True
        elif is_header and not is_json_header:
            if json_buffer:
                output.extend(normalize_json_buffer(json_buffer))
                json_buffer = []
            output.append(line)
            in_json = False
        elif in_json:
            if line.strip() == "":
                if json_buffer:
                    output.extend(normalize_json_buffer(json_buffer))
                    json_buffer = []
                output.append(line)
                in_json = False
            else:
                json_buffer.append(line)
        else:
            output.append(line)

    if json_buffer:
        output.extend(normalize_json_buffer(json_buffer))

    split_lines = "".join(output).split("\n")
    cleaned = []
    for line in split_lines:
        trimmed = line.strip()
        if trimmed == "" and cleaned and cleaned[-1].strip() == "":
            continue
        cleaned.append(line)

    sys.stdout.write("\n".join(cleaned))


if __name__ == "__main__":
    main()
