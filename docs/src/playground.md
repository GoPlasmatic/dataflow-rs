# Playground

Try dataflow-rs in your browser. Define workflows, supply a payload, and inspect the resulting message and run status.

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and per-step context inspection.

<div id="full-playground"></div>

## How to Use

1. **Select an Example** - Choose from the dropdown or write your own
2. **Edit Workflows** - Modify the workflow JSON on the left panel
3. **Edit Payload** - Customize the input payload on the right panel
4. **Process** - Click "Process", or press `Ctrl`/`Cmd`+`Enter` with the cursor in either editor
5. **View Results** - See how the run ended (`completed`, `completed_with_errors` or `stopped`, and why each workflow ran or did not), then the message's context, errors and audit trail

## Tips

- **Parse First** - The payload is *not* part of the evaluation context. Start every workflow with a `parse_json` task (`{"source": "payload", "target": "input"}`) and read the parsed value at `data.input.…`, as every built-in example does. The payload is sent to the engine as the JSON it holds; text that is not JSON is sent as a string
- **Refused Workflows** - A workflow the engine cannot load is reported before anything runs, with each issue's code, its path in the JSON and a message
- **JSONLogic** - Use [JSONLogic](https://jsonlogic.com/) expressions in your workflows for dynamic data access and transformation
- **Multiple Tasks** - Add multiple tasks to a workflow for sequential processing
- **Multiple Workflows** - Define multiple workflows that execute in priority order
- **Conditions** - Add conditions to tasks or workflows to control when they execute (conditions can access `data`, `metadata`, and `temp_data`)
- **Failure Policies** - A task failure stops the run unless a policy says otherwise: `"on_failure": "continue"` on a task records the failure and runs the next task; on a workflow it lets the run go on to the next workflow. A `validation` whose rules fail is a rejection, not a failure: it records its errors and the next task runs by default (`"on_rejection": "continue"`), and `"on_rejection": "halt"` ends the workflow instead
- **Audit Trail** - The output shows all changes made during processing
