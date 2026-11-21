"""Compare an exported CSV with the original, value by value.

Usage: compare.py original.csv exported.csv [key_column]
Numbers are compared numerically, booleans by truth value, and empty means NULL.
Column order must match exactly. Prints the differences and exits 1 if there are any.
"""
import csv
import sys
from decimal import Decimal, InvalidOperation

TRUE = {"1", "true", "t", "yes"}
FALSE = {"0", "false", "f", "no"}


def norm(value):
    v = value.strip()
    if v == "":
        return None
    low = v.lower()
    if low in TRUE:
        return True
    if low in FALSE:
        return False
    try:
        return Decimal(v).normalize()
    except InvalidOperation:
        return value


def load(path):
    with open(path, newline="", encoding="utf-8") as f:
        reader = csv.reader(f)
        header = next(reader)
        return header, [row for row in reader]


orig_path, out_path = sys.argv[1], sys.argv[2]
key = sys.argv[3] if len(sys.argv) > 3 else None
oh, orows = load(orig_path)
eh, erows = load(out_path)
problems = []
if oh != eh:
    problems.append(f"header differs:\n  expected {oh}\n  got      {eh}")
if len(orows) != len(erows):
    problems.append(f"row count differs: expected {len(orows)}, got {len(erows)}")
if key:
    k = oh.index(key)
    orows.sort(key=lambda r: norm(r[k]))
    erows.sort(key=lambda r: norm(r[eh.index(key)] if key in eh else r[k]))
for i, (a, b) in enumerate(zip(orows, erows)):
    for col, name in enumerate(oh):
        if name not in eh:
            continue
        va, vb = norm(a[col]), norm(b[eh.index(name)])
        if va != vb:
            sa, sb = repr(a[col])[:60], repr(b[eh.index(name)])[:60]
            problems.append(f"row {i + 1} column {name}: expected {sa}, got {sb}")
if problems:
    print("\n".join(problems))
    sys.exit(1)
print(f"OK: {len(orows)} rows, {len(oh)} columns match")
