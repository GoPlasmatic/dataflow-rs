# Playground

Try dataflow-rs directly in your browser. Define rules, create messages, and see the processing results in real-time.

> **Looking for advanced debugging?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution, breakpoints, rule visualization, and more!

<div id="full-playground"></div>

## How to Use

1. **Select an Example** - Choose from the dropdown or write your own
2. **Edit Rules** - Modify the rule JSON on the left panel
3. **Edit Payload** - Customize the input payload on the right panel
4. **Process** - Click "Process", or press `Ctrl`/`Cmd`+`Enter` with the cursor in either editor
5. **View Results** - See the processed output with data, metadata, and audit trail

## Tips

- **Parse First** - The payload is *not* part of the evaluation context. Start every rule with a `parse_json` action (`{"source": "payload", "target": "input"}`) and read the parsed value at `data.input.…` — that is what every built-in example does
- **JSONLogic** - Use [JSONLogic](https://jsonlogic.com/) expressions in your rules for dynamic data access and transformation
- **Multiple Actions** - Add multiple actions (tasks) to a rule for sequential processing
- **Multiple Rules** - Define multiple rules that execute in priority order
- **Conditions** - Add conditions to actions or rules to control when they execute (conditions can access `data`, `metadata`, and `temp_data`)
- **Audit Trail** - The output shows all changes made during processing
