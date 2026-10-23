# Playground

Experiment with dataflow-rs directly in your browser. Define workflows, supply a JSON payload, and inspect the resulting message and run report.

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for visual workflow diagrams and step-by-step trace inspection.

<div id="full-playground"></div>

## How to Use

1. **Select an Example** — Pick a starter workflow from the dropdown or write your own from scratch.
2. **Edit Workflows** — Modify the workflow JSON in the left editor.
3. **Edit Payload** — Customize the input payload in the right editor.
4. **Process** — Click **Process** (or press `Ctrl+Enter` / `Cmd+Enter` inside either editor).
5. **View Results** — Inspect the run status (`completed`, `completed_with_errors`, or `stopped`), per-workflow outcomes, and the final message context, error records, and audit trail.

## Tips

- **Parse the Payload First** — `payload` is stored separately from the JSONLogic evaluation context (`data`, `metadata`, `temp_data`). Start your workflow with a `parse_json` task (`{"source": "payload", "target": "input"}`) so subsequent tasks and conditions can read `data.input.*`.
- **Upfront Validation** — If a workflow definition has syntax or schema errors, the engine reports each issue with its code, JSON path, and description before running any message.
- **JSONLogic Expressions** — Use [JSONLogic](https://jsonlogic.com/) in conditions and mappings to read and transform fields dynamically.
- **Chaining Tasks & Workflows** — Add multiple tasks to run sequentially within a workflow, or define multiple workflows ordered by `priority`.
- **Conditional Execution** — Add a `condition` to any workflow, task, or task group to control when it runs based on `data`, `metadata`, or `temp_data`.
- **Failure & Rejection Policies** — By default, an unhandled task failure (`5xx` or error) stops the run. Set `"on_failure": "continue"` on a task to record the error and continue to the next task, or set it on a workflow to continue to the next workflow. Failed `validation` rules return `400` (a rejection), which continues to the next task by default (`"on_rejection": "continue"`)—set `"on_rejection": "halt"` on the validation task if you want to stop the workflow when validation fails.
- **Audit Trail** — Check the output's `audit_trail` to see every field modification recorded during the run.
