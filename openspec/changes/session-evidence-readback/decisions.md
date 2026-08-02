# Decisions: session-evidence-readback

Post-start rulings for this change, each dated, tagged, and sourced.
`design.md`'s Non-Goals and D1-D6 are the pre-start record and are never
edited here; this file holds what was decided after implementation began.

Created 2026-08-02, the day task 9.2's first live smoke run against the
rebuilt `ZS_BIN` produced the first ruling for this change. Nothing before
that date was reconstructed.

<!-- - <MM-DD> [caveat|reversal→pending|reversal→denied|reversal→accepted|included|tracked] <ruling> (source: <where>) -->

- 08-02 [included] Task 9.2's first live run found `session-tool-call-recorded` asserting `tool_called read`, which turned out to be a claim about which tool the model picks rather than about the evidence channel: the model answered "What is the launch code recorded in notes.txt?" with `bash cat notes.txt` instead of the `read` tool, so the scenario read as a random failure rather than a regression pin. The fix is a new name-agnostic assert, `tool_called_any` (section 10), not swapping the pinned name to `bash`: hardcoding whichever tool the model happened to pick on that run would only relocate the same defect to a different name. The live `tool` records themselves matched design D1's mirror field for field on both the `tool_call` and `tool_result` roles (`{"id":0,"name":"bash","args":{...}}` / `{"call_id":0,"name":"bash","truncated":false,"full_output_path":null}`), so the regenerated fixtures from section 1 stand unchanged (source: 9.2 first run, 2026-08-02)
