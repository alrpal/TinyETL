"""Check that two TinyETL outputs (JSON array or JSON Lines) hold the same rows.

Usage: python3 -I compare.py EXPECTED ACTUAL

Rows are compared in order. Numbers are compared by value (12.5 == 12.50), since formats
such as Parquet and CSV may spell a decimal differently.
"""
import json
import sys
from decimal import Decimal


def load(path):
    with open(path, encoding="utf-8") as f:
        text = f.read().strip()
    if text.startswith("["):
        rows = json.loads(text, parse_float=Decimal)
    else:
        rows = [json.loads(line, parse_float=Decimal) for line in text.splitlines() if line.strip()]
    return [normalize(row) for row in rows]


def normalize(value):
    if isinstance(value, dict):
        return {k: normalize(v) for k, v in value.items()}
    if isinstance(value, list):
        return [normalize(v) for v in value]
    if isinstance(value, (int, Decimal)) and not isinstance(value, bool):
        return Decimal(value).normalize()
    return value


expected, actual = load(sys.argv[1]), load(sys.argv[2])
if expected != actual:
    print(f"MISMATCH: {sys.argv[2]} differs from {sys.argv[1]}")
    print(f"  expected {len(expected)} rows, got {len(actual)}")
    for i, (e, a) in enumerate(zip(expected, actual)):
        if e != a:
            print(f"  first difference at row {i + 1}:\n    expected {e}\n    got      {a}")
            break
    sys.exit(1)
