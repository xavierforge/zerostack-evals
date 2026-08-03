# Experiments

Committed, dated snapshots of a `zseval matrix` table: a durable record of "how
did these targets do on this suite", re-readable long after the `results/`
directory that produced it is gone. Unlike `results/` (gitignored: raw
transcripts, one run's working state), a snapshot here is small (markdown plus
embedded TOML) and belongs in git; unlike `baselines/`, which is refreshed in
place as the comparison point moves, this tree is append-only.

## Never regenerate

A snapshot is a dated record, not a cache. Once
`experiments/<date>-<name>.md` is committed, it is never regenerated,
overwritten, or edited to match a later run. If the suite, targets, or judge
changed and you want an up-to-date table, produce a NEW dated snapshot instead
and let the old one stand as history.

## What a snapshot embeds

Beyond the scenario x target table itself (from `zseval matrix --markdown`:
cells, footer, SPREAD/DRIFT marks, and the per-column legend of model/target/
judge state), a snapshot embeds the provenance needed to reproduce and
interpret it later, read straight from each report.json: date, backend, trial
count, total cost, the judge file, `judge_hash`, and `judge_model` tri-state,
and every scenario's `content_hash`. Each target's full `target.toml` is
embedded verbatim from its run-directory copy (`results/<tag>/<stem>/
target.toml`), never re-read from `targets/`, which can drift after the run.
When a column has no run-directory copy available (for example, a report
reused from `baselines/` and detached from any `results/` tree), the snapshot
records that column's identity from the report and notes plainly that its
`target.toml` content is not embedded, rather than silently omitting it.

## Ritual

Produced by hand via redirect, the same manual ritual as `baselines/`
(`baselines/README.md`). Substitute your own `TAG` (a dated, descriptive tag
doubles as the snapshot's file stem) and target files.

1. Run the suite against every target you want in the table, sharing one tag,
   so they land together under `results/<tag>/`:

   ```
   TAG=2026-07-20-opus-vs-sonnet
   zseval run scenarios \
     --target targets/opus.toml --target targets/sonnet.toml \
     --tag "$TAG" --trials 3
   ```

2. Render the table to markdown and redirect it into a new dated file:

   ```
   zseval matrix results/"$TAG"/*/report.json --markdown \
     > experiments/"$TAG".md
   ```

3. Append each column's provenance and full `target.toml` (from the
   run-directory copy), degrading honestly when a column has none:

   ```
   python3 - "experiments/$TAG.md" results/"$TAG"/*/report.json <<'PY'
   import json, pathlib, sys

   out_path, *report_paths = sys.argv[1:]
   lines = ["", "## Provenance", ""]
   for rp in report_paths:
       report = json.loads(pathlib.Path(rp).read_text())
       run_dir = pathlib.Path(rp).parent
       lines += [
           f"### {report['target'] or '(no target identity)'}",
           "",
           f"- date: {report['timestamp']}",
           f"- backend: {report['backend']}",
           f"- model: {report['model']}",
           f"- trials: {report['trials']}",
           f"- total cost (usd): {report['summary']['total_cost_usd']}",
           f"- judge file: {report['judge_file'] or '(none)'}",
           f"- judge hash: {report.get('judge_hash') or '(none)'}",
           f"- judge model(s): {report.get('judge_model')}",
           "",
           "content hashes:",
           "",
       ]
       lines += [f"- `{s['id']}`: `{s['content_hash']}`" for s in report["scenarios"]]
       lines.append("")
       toml_path = run_dir / "target.toml"
       if toml_path.is_file():
           lines += ["```toml", toml_path.read_text().rstrip("\n"), "```", ""]
       else:
           lines += [
               f"_target.toml not embedded: no run-directory copy available "
               f"for `{report['target'] or rp}`._",
               "",
           ]

   with open(out_path, "a") as f:
       f.write("\n".join(lines) + "\n")
   PY
   ```

4. Commit `experiments/"$TAG".md` (only that new file — never edit an existing
   snapshot).

A composed-across-time table (reusing a `baselines/` report as one column
instead of a fresh run) skips step 1 for that column: point step 2 at
`baselines/main.json` alongside the fresh `results/` report, and step 3 will
correctly note that the baseline column's `target.toml` is not embedded, since
it has no `results/` run directory.
