//! zerostack subagents (`task` tool) knowledge — investigated 2026-07-07.
//!
//! Unlike `memory`, there is no seeding surface here (a subagent's result
//! is returned as tool output, not written to any layout-owned path) and
//! no reliable startup/registration trace line to check for drift:
//! `src/extras/subagents/{mod,builder,prompt,task_tool}.rs` contain zero
//! `tracing::` calls of any kind (grepped directly against the zerostack
//! checkout), unlike `memory`'s `Mem::open()`, which logs `memory open:
//! root=..., project=...` on every open. The only evidence the `task` tool
//! exists and ran at all is the harness's *generic* tool-call parsing —
//! `tool_called task` / `tool_not_called task` already work with zero
//! domain code, since `transcript::parse_str` turns every
//! `tool_call`-role message's structured `tool` record into a `ToolCall`
//! regardless of name — plus a permission-checker debug line
//! (`perm check: tool=task, input_len=N`) that fires identically for every
//! tool, not task-specific.
//!
//! Confirmed empirically (`zerostack -p --yolo --pure-stdout --log-level
//! debug --no-context-files`, isolated ZS_DATA_DIR/ZS_CONFIG_DIR, a 2-file
//! project, task "Use the task tool once to look up what functions are
//! defined in this small project"):
//!   - the `task` tool's *call* summary was EMPTY — its args use a field
//!     name (`prompts`, plural, since a single call can run several
//!     investigations in parallel) that matched no key in the summary
//!     formatter's priority list, so `tool_arg_contains task ...` could
//!     never usefully match. Upstream has since given `task` its own
//!     summary branch (`ui::utils::format_task_summary` renders the
//!     prompts), so re-check this before relying on it either way.
//!   - the subagent's *own internal* tool calls (it read files, listed a
//!     directory, etc. in the test run) were indistinguishable from the
//!     main agent's own direct tool calls. Upstream PR #230 fixed this:
//!     they are now recorded as `subagent_tool_call`-role messages, which
//!     `transcript.rs` surfaces as `ToolCall { subagent: true }`.
//!
//! Net effect: there is no drift check to write. `verify` below is a
//! deliberate no-op, not a stub — the honest state of the evidence as of
//! this date. A one-line `tracing::debug!("subagent spawned: ...")` in
//! `task_tool.rs` upstream would give this module something to check;
//! until then, "did the agent delegate when it should have / skip
//! delegating when it shouldn't" relies entirely on scenario design —
//! every subagent scenario pairs a positive assert (e.g. `final_contains
//! <answer>`) alongside its `tool_called`/`tool_not_called task` assert,
//! so a broken evidence channel would show up as that positive assert
//! failing, not as a silent vacuous pass. See `scenarios/subagents/`.
//!
//! No `SeedSugar` field: this deviates from the "one new `SeedSugar`
//! field" line in `scenarios/README.md`, which describes domains that
//! seed something.
//! Subagents seed nothing, so a scenario opts in purely via
//! `domains = ["subagents"]` at the top level — the same explicit-opt-in
//! path `memory` uses for its empty-store case, just with nothing (yet)
//! behind it.

use std::path::PathBuf;

use crate::backend::RunRoots;

/// No registration/drift evidence exists in zerostack's logs as of
/// 2026-07-07 (see module doc) — always `Ok`. Kept as a real function, not
/// deleted, so `domains::verify`'s dispatch has somewhere to route
/// `domains = ["subagents"]` to, and so the day zerostack logs a
/// subagent-spawn trace line, this is the one place to wire the check in.
pub fn verify(_roots: &RunRoots, _zslogs: &[PathBuf]) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn verify_is_a_deliberate_no_op() {
        let roots = RunRoots {
            data: Path::new("/d"),
            config: Path::new("/c"),
            work: Path::new("/w"),
        };
        assert!(verify(&roots, &[]).is_ok());
        assert!(verify(&roots, &[PathBuf::from("/nonexistent.zslog")]).is_ok());
    }
}
