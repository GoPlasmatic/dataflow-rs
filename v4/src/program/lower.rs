//! Lowering (I47): the compile walk emits a workflow's ops as it reaches each
//! step, and this module holds the emitter, the listing and the invariants.
//!
//! The shape, from 05:
//!
//! ```text
//! [Cond workflow else->end]       the pre-phase: a loop with setup or over
//! [setup steps, fail->end]
//! LoopInit exit->end              only with a loop
//! [Cond workflow else->end]       before every sweep, or once without a loop
//! body steps, fail->LoopNext or end
//! LoopNext body->(the sweep's first op) exit->end
//! ```
//!
//! A task is `[Cond task else->(past its call)]` then `Call`, or `FanOut` with
//! a `for_each`. A group is `[Cond group else->(past the group)]`, its members,
//! then `Halt` when it is terminal. A condition that is absent, or folds to the
//! literal `true`, emits no `Cond`.

use super::compile::{CompiledWorkflow, next};
use super::ops::{Callee, CondKind, Op};
use std::fmt;

/// A target not yet known. Every one is patched before [`Lower::finish`]
/// returns.
const PENDING: u32 = u32::MAX;

/// The ops of one workflow, emitted in order as the walk reaches each step.
///
/// A forward target is recorded as a patch and filled once its index is
/// known: a `Cond`'s `else_to` when its call or group is closed, a body's
/// `fail_to` at the `LoopNext`, and every jump to the end at
/// [`finish`](Self::finish). No pass rebuilds the list.
#[derive(Default)]
pub(super) struct Lower {
    ops: Vec<Op>,
    /// The ops whose forward target is the end.
    to_end: Vec<usize>,
    /// While a loop body is lowered, the ops whose `fail_to` is its
    /// `LoopNext`.
    to_next: Option<Vec<usize>>,
}

impl Lower {
    /// The index the next op takes.
    fn here(&self) -> u32 {
        next(self.ops.len())
    }

    fn push(&mut self, op: Op) -> usize {
        self.ops.push(op);
        self.ops.len() - 1
    }

    /// Points the op at `at`, which has one forward target, to `to`.
    fn patch(&mut self, at: usize, to: u32) {
        match &mut self.ops[at] {
            Op::Cond {
                else_to: target, ..
            }
            | Op::Call {
                fail_to: target, ..
            }
            | Op::FanOut {
                fail_to: target, ..
            }
            | Op::LoopInit {
                exit_to: target, ..
            }
            | Op::LoopNext {
                exit_to: target, ..
            } => *target = to,
            Op::Halt => unreachable!("a `Halt` has no target"),
        }
    }

    /// Sends the op at `at`'s failures to the `LoopNext` in a loop body, and
    /// to the end everywhere else, setup included.
    fn fails_from(&mut self, at: usize) {
        match &mut self.to_next {
            Some(body) => body.push(at),
            None => self.to_end.push(at),
        }
    }

    /// The workflow's condition, false to the end. Nothing when `cond` is
    /// `None`: absent, or literally `true`.
    pub(super) fn workflow_cond(&mut self, cond: Option<u32>) {
        if let Some(cond) = cond {
            let at = self.push(Op::Cond {
                cond,
                kind: CondKind::Workflow,
                else_to: PENDING,
            });
            self.to_end.push(at);
        }
    }

    /// A group's or task's condition, whose `else_to` [`close`](Self::close)
    /// fills. Nothing when `cond` is `None`.
    pub(super) fn open(&mut self, cond: Option<u32>, kind: CondKind) -> Option<usize> {
        cond.map(|cond| {
            self.push(Op::Cond {
                cond,
                kind,
                else_to: PENDING,
            })
        })
    }

    /// Points the condition [`open`](Self::open) emitted past every op since.
    pub(super) fn close(&mut self, open: Option<usize>) {
        if let Some(at) = open {
            let past = self.here();
            self.patch(at, past);
        }
    }

    /// One call of call site `site`.
    pub(super) fn call(&mut self, site: u32) {
        let at = self.push(Op::Call {
            site,
            fail_to: PENDING,
        });
        self.fails_from(at);
    }

    /// Call site `site`, once per element of fan-out site `fan`.
    pub(super) fn fan_out(&mut self, site: u32, fan: u32) {
        let at = self.push(Op::FanOut {
            site,
            fan,
            fail_to: PENDING,
        });
        self.fails_from(at);
    }

    /// The end of a terminal group.
    pub(super) fn halt(&mut self) {
        self.push(Op::Halt);
    }

    /// Loop site `lp`'s `LoopInit`, exiting to the end, which opens its body.
    /// Returns where every sweep starts.
    pub(super) fn loop_init(&mut self, lp: u32) -> u32 {
        let at = self.push(Op::LoopInit {
            lp,
            exit_to: PENDING,
        });
        self.to_end.push(at);
        self.to_next = Some(Vec::new());
        self.here()
    }

    /// Closes the body [`loop_init`](Self::loop_init) opened: its failures go
    /// to this `LoopNext`, which goes back to `sweep` or exits to the end.
    pub(super) fn loop_next(&mut self, lp: u32, sweep: u32) {
        let here = self.here();
        for at in self.to_next.take().unwrap_or_default() {
            self.patch(at, here);
        }
        let at = self.push(Op::LoopNext {
            lp,
            body_to: sweep,
            exit_to: PENDING,
        });
        self.to_end.push(at);
    }

    /// The ops, with every jump to the end filled.
    pub(super) fn finish(mut self) -> Vec<Op> {
        let end = self.here();
        for at in std::mem::take(&mut self.to_end) {
            self.patch(at, end);
        }
        self.ops
    }
}

/// An op's jump targets.
fn targets(op: Op) -> impl Iterator<Item = u32> {
    let (first, second) = match op {
        Op::Cond { else_to: to, .. }
        | Op::Call { fail_to: to, .. }
        | Op::FanOut { fail_to: to, .. }
        | Op::LoopInit { exit_to: to, .. } => (Some(to), None),
        Op::LoopNext {
            body_to, exit_to, ..
        } => (Some(body_to), Some(exit_to)),
        Op::Halt => (None, None),
    };
    first.into_iter().chain(second)
}

/// Whether `index` is a row of a table of `len`, else why not.
fn in_range(pc: usize, table: &str, index: u32, len: usize) -> Result<usize, String> {
    let index = index as usize;
    if index < len {
        Ok(index)
    } else {
        Err(format!(
            "op {pc} names {table} row {index}, and there are {len}"
        ))
    }
}

impl CompiledWorkflow {
    /// The ops, one line each, `{pc:>2} {op}`: the snapshot contract (I47). A
    /// target is its op index, or `end` when it is the end.
    ///
    /// A row the op names that does not exist prints as `?`, so a broken
    /// lowering can still be listed.
    pub(crate) fn listing(&self) -> String {
        Listing(self).to_string()
    }

    /// Checks the lowering's invariants, naming the first broken one:
    ///
    /// - every jump target is at most `ops.len()`;
    /// - every row an op names is in its table, as is every call site's task;
    /// - every condition is named by at least one `Cond`;
    /// - every call site and every task row is named by exactly one `Call` or
    ///   `FanOut`;
    /// - every `LoopInit` comes before its `LoopNext`.
    pub(crate) fn check_invariants(&self) -> Result<(), String> {
        let end = self.ops.len();
        let mut conds = vec![false; self.conds.len()];
        let mut calls = vec![0_usize; self.calls.len()];
        let mut tasks = vec![0_usize; self.tasks.len()];
        let mut inits = vec![None; self.loops.len()];
        let mut nexts = vec![None; self.loops.len()];
        for (pc, &op) in self.ops.iter().enumerate() {
            if let Some(to) = targets(op).find(|&to| to as usize > end) {
                return Err(format!("op {pc} jumps to {to}, past the end at {end}"));
            }
            let site = match op {
                Op::Cond { cond, .. } => {
                    let cond = in_range(pc, "condition", cond, conds.len())?;
                    conds[cond] = true;
                    None
                }
                Op::Call { site, .. } => Some(site),
                Op::FanOut { site, fan, .. } => {
                    in_range(pc, "fan-out", fan, self.fans.len())?;
                    Some(site)
                }
                Op::LoopInit { lp, .. } => {
                    let lp = in_range(pc, "loop", lp, inits.len())?;
                    inits[lp] = Some(pc);
                    None
                }
                Op::LoopNext { lp, .. } => {
                    let lp = in_range(pc, "loop", lp, nexts.len())?;
                    if inits[lp].is_none() {
                        return Err(format!(
                            "op {pc} is loop {lp}'s LoopNext, before its LoopInit"
                        ));
                    }
                    nexts[lp] = Some(pc);
                    None
                }
                Op::Halt => None,
            };
            if let Some(site) = site {
                let site = in_range(pc, "call site", site, calls.len())?;
                calls[site] += 1;
                let task = in_range(pc, "task", self.calls[site].task, tasks.len())?;
                tasks[task] += 1;
            }
        }
        if let Some(cond) = conds.iter().position(|named| !named) {
            return Err(format!("condition {cond} is named by no Cond"));
        }
        if let Some((site, n)) = calls.iter().enumerate().find(|(_, n)| **n != 1) {
            return Err(format!("call site {site} is called by {n} ops, not one"));
        }
        if let Some((task, n)) = tasks.iter().enumerate().find(|(_, n)| **n != 1) {
            return Err(format!("task {task} is called by {n} ops, not one"));
        }
        if let Some(lp) = (0..inits.len()).find(|&lp| inits[lp].is_some() && nexts[lp].is_none()) {
            return Err(format!("loop {lp}'s LoopInit has no LoopNext after it"));
        }
        Ok(())
    }
}

/// [`CompiledWorkflow::listing`]'s text.
struct Listing<'w>(&'w CompiledWorkflow);

impl fmt::Display for Listing<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let wf = self.0;
        let end = wf.ops.len();
        let to = |to: u32| {
            if to as usize == end {
                "end".to_owned()
            } else {
                to.to_string()
            }
        };
        // A call site's task id, function, kind and ` terminal` flag.
        let call = |site: u32| {
            let Some(site) = wf.calls.get(site as usize) else {
                return ("?", "?", "?", "");
            };
            let (id, function) = wf
                .tasks
                .get(site.task as usize)
                .map_or(("?", "?"), |task| (&*task.id, &*task.function));
            let kind = match site.callee {
                Callee::Builtin(_) => "builtin",
                Callee::Host(_) => "host",
            };
            (
                id,
                function,
                kind,
                if site.terminal { " terminal" } else { "" },
            )
        };
        for (pc, &op) in wf.ops.iter().enumerate() {
            write!(f, "{pc:>2} ")?;
            match op {
                Op::Cond {
                    cond,
                    kind,
                    else_to,
                } => {
                    let step = wf
                        .conds
                        .get(cond as usize)
                        .map_or(Some("?"), |c| c.step.as_deref());
                    match (kind, step) {
                        (CondKind::Workflow, _) => write!(f, "Cond workflow")?,
                        (CondKind::Group, step) => write!(f, "Cond group {}", step.unwrap_or("?"))?,
                        (CondKind::Task, step) => write!(f, "Cond task {}", step.unwrap_or("?"))?,
                    }
                    writeln!(f, " else->{}", to(else_to))?;
                }
                Op::Call { site, fail_to } => {
                    let (id, function, kind, terminal) = call(site);
                    writeln!(
                        f,
                        "Call {id} {function} {kind} fail->{}{terminal}",
                        to(fail_to)
                    )?;
                }
                Op::FanOut { site, fan, fail_to } => {
                    let (id, function, kind, terminal) = call(site);
                    let item = wf.fans.get(fan as usize).map_or_else(
                        || "?".to_owned(),
                        |fan| {
                            let item = fan.r#as.to_string();
                            item.strip_prefix("temp_data.").unwrap_or(&item).to_owned()
                        },
                    );
                    writeln!(
                        f,
                        "FanOut {id} {function} {kind} as={item} fail->{}{terminal}",
                        to(fail_to)
                    )?;
                }
                Op::LoopInit { exit_to, .. } => writeln!(f, "LoopInit exit->{}", to(exit_to))?,
                Op::LoopNext {
                    body_to, exit_to, ..
                } => writeln!(f, "LoopNext body->{} exit->{}", to(body_to), to(exit_to))?,
                Op::Halt => writeln!(f, "Halt")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::ops::Op;
    use super::*;
    use crate::base::{Result, StepKind};
    use crate::def::{ParseOptions, parse};
    use crate::expr::Expr;
    use crate::function::{Ctx, Function, FunctionConfig, Outcome, Registry};
    use crate::program::CompileOptions;
    use serde_json::{Value, json};

    #[derive(FunctionConfig)]
    struct FxRateConfig {
        from: Expr<String>,
    }

    /// A host function: a currency's rate.
    struct FxRate;

    impl Function for FxRate {
        type Config = FxRateConfig;
        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &FxRateConfig) -> Result<Outcome> {
            Ok(Outcome::Ok)
        }
    }

    #[derive(FunctionConfig)]
    struct PriceItemConfig {
        sku: Expr<String>,
    }

    /// A host function: one item's price.
    struct PriceItem;

    impl Function for PriceItem {
        type Config = PriceItemConfig;
        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &PriceItemConfig) -> Result<Outcome> {
            Ok(Outcome::Ok)
        }
    }

    #[derive(FunctionConfig)]
    struct EchoConfig {
        value: Expr<String>,
    }

    /// A host function that echoes its value.
    struct Echo;

    impl Function for Echo {
        type Config = EchoConfig;
        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &EchoConfig) -> Result<Outcome> {
            Ok(Outcome::Ok)
        }
    }

    fn compile(json: &Value) -> CompiledWorkflow {
        let mut registry = Registry::default();
        registry.register("fx_rate", FxRate);
        registry.register("price_item", PriceItem);
        registry.register("echo", Echo);
        let parsed = parse(json, ParseOptions::default());
        let def = parsed
            .def
            .unwrap_or_else(|| panic!("the definition parses: {:?}", parsed.issues));
        registry
            .compiler(CompileOptions::default())
            .compile_one(&def)
            .unwrap_or_else(|issues| panic!("the workflow compiles: {issues:?}"))
            .workflow
    }

    /// Compiles `json` and checks its listing against `lines`, and that it
    /// keeps every invariant.
    #[track_caller]
    fn snapshot(json: &Value, lines: &[&str]) -> CompiledWorkflow {
        let wf = compile(json);
        let expected: String = lines.iter().map(|line| format!("{line}\n")).collect();
        assert_eq!(wf.listing(), expected, "\n{}", wf.listing());
        assert_eq!(wf.check_invariants(), Ok(()));
        wf
    }

    /// A `map` task.
    fn map(id: &str) -> Value {
        json!({"id": id, "function": {"name": "map", "input": {
            "mappings": [{"path": "data.a", "logic": 1}]}}})
    }

    /// `map(id)` with `key` set to `value`.
    fn map_with(id: &str, extra: &[(&str, Value)]) -> Value {
        let mut task = map(id);
        for (key, value) in extra {
            task[*key] = value.clone();
        }
        task
    }

    /// A workflow condition that reads the message.
    fn kind_is_x() -> Value {
        json!({"==": [{"var": "metadata.kind"}, "x"]})
    }

    #[test]
    fn a_straight_line() {
        let log = json!({"id": "b", "function": {"name": "log", "input": {"message": "hi"}}});
        snapshot(
            &json!({"id": "w", "tasks": [map("a"), log]}),
            &[
                " 0 Call a map builtin fail->end",
                " 1 Call b log builtin fail->end",
            ],
        );
    }

    #[test]
    fn conditions_and_folding() {
        let wf = snapshot(
            &json!({"id": "w", "condition": kind_is_x(), "tasks": [
                map_with("a", &[("condition", json!({"var": "data.go"}))]),
                map_with("b", &[("condition", json!(true)), ("terminal", json!(true))]),
            ]}),
            &[
                " 0 Cond workflow else->end",
                " 1 Cond task a else->3",
                " 2 Call a map builtin fail->end",
                " 3 Call b map builtin fail->end terminal",
            ],
        );
        // A condition folding to `true` keeps no row.
        let conds: Vec<_> = wf.conds.iter().map(|c| c.path.to_string()).collect();
        assert_eq!(conds, ["condition", "tasks[0].condition"]);
    }

    #[test]
    fn a_counter_only_loop() {
        snapshot(
            &json!({"id": "w", "condition": kind_is_x(),
                    "loop": {"counter": "i", "max": 10}, "tasks": [map("a")]}),
            &[
                " 0 LoopInit exit->end",
                " 1 Cond workflow else->end",
                " 2 Call a map builtin fail->3",
                " 3 LoopNext body->1 exit->end",
            ],
        );
    }

    #[test]
    fn a_loop_over_an_array() {
        snapshot(
            &json!({"id": "w",
                    "loop": {"max": 10, "over": {"var": "data.items"}, "as": "item"},
                    "tasks": [map("a")]}),
            &[
                " 0 LoopInit exit->end",
                " 1 Call a map builtin fail->2",
                " 2 LoopNext body->1 exit->end",
            ],
        );
    }

    #[test]
    fn nested_groups() {
        snapshot(
            &json!({"id": "w", "tasks": [
                {"id": "A", "condition": {"var": "data.a"}, "tasks": [
                    map("t1"),
                    {"id": "B", "condition": {"var": "data.b"}, "terminal": true,
                     "tasks": [map("t2")]},
                    map("t3"),
                ]},
                map("t4"),
            ]}),
            &[
                " 0 Cond group A else->6",
                " 1 Call t1 map builtin fail->end",
                " 2 Cond group B else->5",
                " 3 Call t2 map builtin fail->end",
                " 4 Halt",
                " 5 Call t3 map builtin fail->end",
                " 6 Call t4 map builtin fail->end",
            ],
        );
    }

    #[test]
    fn groups_without_conditions() {
        snapshot(
            &json!({"id": "w", "tasks": [
                {"id": "G", "tasks": [map("t1")]},
                {"id": "H", "terminal": true, "tasks": [map("t2")]},
                map("t3"),
            ]}),
            &[
                " 0 Call t1 map builtin fail->end",
                " 1 Call t2 map builtin fail->end",
                " 2 Halt",
                " 3 Call t3 map builtin fail->end",
            ],
        );
    }

    /// 05's worked lowering, verbatim.
    fn price_batches() -> Value {
        json!({
          "id": "price-batches",
          "condition": {"==": [{"var": "metadata.kind"}, "batch"]},
          "loop": {
            "max": 50,
            "setup": [
              {"id": "load", "function": {"name": "parse_json", "input": {"source": "payload", "target": "data"}}}
            ],
            "over": {"var": "data.batches"},
            "as": "batch",
            "scratch": "acc"
          },
          "tasks": [
            {"id": "fx", "condition": {"!=": [{"var": "temp_data.batch.currency"}, "USD"]}, "tasks": [
              {"id": "rate", "function": {"name": "fx_rate", "input": {"from": {"var": "temp_data.batch.currency"}}}},
              {"id": "convert", "function": {"name": "map", "input": {"mappings": [
                {"path": "temp_data.acc.rate", "logic": {"var": "temp_data.fx.rate"}}
              ]}}}
            ]},
            {"id": "price", "function": {"name": "price_item", "input": {"sku": {"var": "temp_data.item.sku"}}},
             "for_each": {"over": {"var": "temp_data.batch.items"}, "as": "item",
                          "collect": "temp_data.price", "into": "temp_data.acc.prices"}},
            {"id": "sum", "function": {"name": "map", "input": {"mappings": [
              {"path": "data.total", "logic": {"reduce": [{"var": "temp_data.acc.prices"},
                {"+": [{"var": "accumulator"}, {"var": "current"}]}, {"var": "data.total"}]}}
            ]}}},
            {"id": "cap", "condition": {">": [{"var": "data.total"}, 10000]}, "terminal": true, "tasks": [
              {"id": "flag", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.capped", "logic": true}
              ]}}}
            ]}
          ]
        })
    }

    #[test]
    fn the_worked_lowering() {
        snapshot(
            &price_batches(),
            &[
                " 0 Cond workflow else->end",
                " 1 Call load parse_json builtin fail->end",
                " 2 LoopInit exit->end",
                " 3 Cond workflow else->end",
                " 4 Cond group fx else->7",
                " 5 Call rate fx_rate host fail->12",
                " 6 Call convert map builtin fail->12",
                " 7 FanOut price price_item host as=item fail->12",
                " 8 Call sum map builtin fail->12",
                " 9 Cond group cap else->12",
                "10 Call flag map builtin fail->12",
                "11 Halt",
                "12 LoopNext body->3 exit->end",
            ],
        );
    }

    #[test]
    fn the_worked_lowering_fills_its_side_tables() {
        let wf = compile(&price_batches());
        let [lp] = &wf.loops[..] else {
            panic!("one loop");
        };
        assert_eq!(lp.r#as.as_ref().unwrap().to_string(), "temp_data.batch");
        assert_eq!(lp.scratch.as_ref().unwrap().to_string(), "temp_data.acc");

        let [fan] = &wf.fans[..] else {
            panic!("one fan-out");
        };
        assert_eq!(fan.r#as.to_string(), "temp_data.item");
        assert_eq!(fan.index.to_string(), "temp_data.item_index");
        assert_eq!(fan.collect.as_ref().unwrap().to_string(), "temp_data.price");
        assert_eq!(
            fan.into.as_ref().unwrap().to_string(),
            "temp_data.acc.prices"
        );

        let rows: Vec<_> = wf
            .tasks
            .iter()
            .map(|t| (&*t.id, t.kind, t.path.to_string()))
            .collect();
        let row = |id, kind, path: &str| (id, kind, path.to_owned());
        assert_eq!(
            rows,
            [
                row("load", StepKind::Setup, "loop.setup[0]"),
                row("rate", StepKind::Task, "tasks[0].tasks[0]"),
                row("convert", StepKind::Task, "tasks[0].tasks[1]"),
                row("price", StepKind::Task, "tasks[1]"),
                row("sum", StepKind::Task, "tasks[2]"),
                row("flag", StepKind::Task, "tasks[3].tasks[0]"),
            ]
        );
    }

    #[test]
    fn setup_in_a_counter_only_loop() {
        snapshot(
            &json!({"id": "w", "condition": kind_is_x(),
                    "loop": {"max": 3, "setup": [map("s")]}, "tasks": [map("a")]}),
            &[
                " 0 Cond workflow else->end",
                " 1 Call s map builtin fail->end",
                " 2 LoopInit exit->end",
                " 3 Cond workflow else->end",
                " 4 Call a map builtin fail->5",
                " 5 LoopNext body->3 exit->end",
            ],
        );
    }

    #[test]
    fn a_conditional_fan_out() {
        snapshot(
            &json!({"id": "w", "tasks": [
                {"id": "f", "condition": {"var": "data.go"},
                 "for_each": {"over": [1, 2], "as": "x"},
                 "function": {"name": "echo", "input": {"value": "x"}}},
            ]}),
            &[
                " 0 Cond task f else->end",
                " 1 FanOut f echo host as=x fail->end",
            ],
        );
    }

    #[test]
    fn an_op_is_16_bytes() {
        assert_eq!(size_of::<Op>(), 16);
    }

    #[test]
    fn check_invariants_refuses_a_jump_past_the_end() {
        let mut wf = compile(&json!({"id": "w", "tasks": [map("a"), map("b")]}));
        wf.ops = vec![
            Op::Call {
                site: 0,
                fail_to: 3,
            },
            Op::Call {
                site: 1,
                fail_to: 2,
            },
        ];
        assert!(wf.check_invariants().is_err());
    }
}
