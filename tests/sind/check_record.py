# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
"""Checks unjam journals with pyarrow (IT 6 and IT 7).

Usage: uv run --with pyarrow==21.0.0 tests/sind/check_record.py [options] FILE.arrows...
  --source S    expected unjam.source metadata: record (default) or tui
  --comm C      some process row must have this comm
  --extra       io and pids columns non-null on cgroup rows (EnableExtraControllers=io,pids);
                without it they must be null there
  --truncated   the journal may end without its end-of-stream marker; at least one complete
                batch must be readable
A FILE.parquet next to a FILE.arrows (written by the documented recipe) must have the same
row count. Prints one line per file; exits 1 on the first failed check.
"""

import argparse
import sys
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

FIXED = ["ts", "kind", "cg", "pid", "start", "ppid", "path", "comm", "cmdline"]


def fail(path, msg):
    print(f"not ok - {path}: {msg}")
    sys.exit(1)


def read(path, truncated):
    """The journal's schema and every complete batch as one table."""
    reader = pa.ipc.open_stream(path)
    batches = []
    try:
        for batch in reader:
            batches.append(batch)
    except (pa.ArrowInvalid, OSError) as e:
        if not truncated:
            fail(path, f"unreadable: {e}")
    if truncated and not batches:
        fail(path, "no complete batch")
    return pa.Table.from_batches(batches, schema=reader.schema)


def check(path, args):
    table = read(path, args.truncated)
    meta = {k.decode(): v.decode() for k, v in (table.schema.metadata or {}).items()}
    for key in ["unjam.schema", "unjam.version", "unjam.host", "unjam.interval_ms"]:
        if key not in meta:
            fail(path, f"schema metadata {key} missing")
    if meta.get("unjam.source") != args.source:
        fail(path, f"unjam.source is {meta.get('unjam.source')}, not {args.source}")
    if table.schema.names[: len(FIXED)] != FIXED:
        fail(path, f"fixed columns {table.schema.names[:len(FIXED)]}")
    metrics = [f for f in table.schema if f.name.startswith(("cg_", "p_"))]
    for f in metrics:
        fm = {k.decode(): v.decode() for k, v in (f.metadata or {}).items()}
        if not {"unjam.unit", "unjam.kind", "unjam.source"} <= fm.keys():
            fail(path, f"field metadata of {f.name}: {fm}")
    if table.num_rows == 0:
        fail(path, "no rows")
    rows = table.to_pylist()
    cgs = [r for r in rows if r["kind"] == 1]
    if args.comm and not any(r["comm"] == args.comm for r in rows if r["kind"] == 2):
        fail(path, f"no process row with comm {args.comm}")
    last = {}
    for r in cgs:
        v = r["cg_cpu_usage_usec"]
        if v is not None:
            if v < last.get(r["cg"], 0):
                fail(path, f"cg_cpu_usage_usec decreases for cgroup {r['cg']}")
            last[r["cg"]] = v
    sampled = [r for r in cgs if r["cg_cpu_usage_usec"] is not None]
    for col in ["cg_io_read_bytes", "cg_pids_current"]:
        null = [r["path"] for r in sampled if r[col] is None]
        if args.extra and null:
            fail(path, f"{col} null on {len(null)} sampled cgroup rows")
        if not args.extra and len(null) != len(sampled):
            fail(path, f"{col} set without the extra controllers")
    parquet = Path(path).with_suffix(".parquet")
    if parquet.exists() and pq.read_table(parquet).num_rows != table.num_rows:
        fail(path, f"{parquet} has {pq.read_table(parquet).num_rows} rows, not {table.num_rows}")
    print(f"ok - {path}: {table.num_rows} rows, {len(metrics)} metric columns, host {meta['unjam.host']}")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    p.add_argument("--source", default="record")
    p.add_argument("--comm")
    p.add_argument("--extra", action="store_true")
    p.add_argument("--truncated", action="store_true")
    p.add_argument("files", nargs="+")
    args = p.parse_args()
    for f in args.files:
        check(f, args)


if __name__ == "__main__":
    main()
