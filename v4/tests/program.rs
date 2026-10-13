extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_rs::Registry;
use dataflow_rs::def::{ParseOptions, Status, parse};
use dataflow_rs::program::{CompileOptions, CompiledWorkflow, Compiler, Program, StepKind};
use serde_json::{Value, json};

const _: () = {
    fn assert<T: Send + Sync>() {}
    fn assert_clone<T: Clone>() {}
    let _ = assert::<Program>;
    let _ = assert_clone::<Program>;
    let _ = assert::<CompiledWorkflow>;
};

fn compiler() -> Compiler {
    Registry::default().compiler(CompileOptions::default())
}

fn log(id: &str) -> Value {
    json!({"id": id, "function": {"name": "log", "input": {"message": "hi"}}})
}

fn compile(c: &Compiler, json: &Value) -> CompiledWorkflow {
    let def = parse(json, ParseOptions::default())
        .def
        .expect("the definition parses");
    c.compile_one(&def)
        .unwrap_or_else(|issues| panic!("compiles: {issues:?}"))
        .workflow
}

fn link(c: &Compiler, defs: &[Value]) -> Program {
    c.link(defs.iter().map(|d| compile(c, d)))
}

fn ids(program: &Program) -> Vec<&str> {
    program.workflows().iter().map(|w| w.id.as_str()).collect()
}

#[test]
fn workflows_sort_by_priority_with_input_order_breaking_ties() {
    let c = compiler();
    let defs: Vec<Value> = [("a", 5), ("b", 1), ("c", 5), ("d", 0)]
        .into_iter()
        .map(|(id, p)| json!({"id": id, "priority": p, "tasks": [log("t")]}))
        .collect();
    let program = link(&c, &defs);
    assert_eq!(ids(&program), ["d", "b", "a", "c"]);
    for (i, w) in program.workflows().iter().enumerate() {
        assert_eq!(w.ix.index(), u32::try_from(i).unwrap());
    }
}

#[test]
fn every_link_has_its_own_id() {
    let c = compiler();
    let defs = [json!({"id": "a", "tasks": [log("t")]})];
    assert_ne!(link(&c, &defs).id(), link(&c, &defs).id());
    let program = link(&c, &defs);
    assert_eq!(program.clone().id(), program.id());
}

#[test]
fn the_task_table_is_dense_across_workflows() {
    let c = compiler();
    let defs = [
        json!({"id": "first", "priority": 0, "tasks": [log("only")]}),
        json!({"id": "second", "priority": 1,
            "loop": {"max": 2, "setup": [log("s")]},
            "tasks": [log("t1"), log("t2")]}),
    ];
    let program = link(&c, &defs);
    let rows: Vec<_> = program
        .tasks()
        .iter()
        .map(|t| (t.ix.index(), t.id.as_str(), t.kind, t.workflow.index()))
        .collect();
    assert_eq!(
        rows,
        [
            (0, "only", StepKind::Task, 0),
            (1, "s", StepKind::Setup, 1),
            (2, "t1", StepKind::Task, 1),
            (3, "t2", StepKind::Task, 1),
        ]
    );
    assert_eq!(program.tasks()[1].function, "log");
    assert_eq!(program.tasks()[2].path.to_string(), "tasks[0]");
    assert_eq!(program.tasks()[1].path.to_string(), "loop.setup[0]");
    for t in program.tasks() {
        assert_eq!(program.task(t.ix).id, t.id);
        assert_eq!(program.task(t.ix).ix, t.ix);
    }
}

#[test]
#[should_panic]
fn a_task_index_out_of_range_panics() {
    let c = compiler();
    let small = link(&c, &[json!({"id": "a", "tasks": [log("t")]})]);
    let big = link(&c, &[json!({"id": "a", "tasks": [log("t"), log("u")]})]);
    let _ = small.task(big.tasks()[1].ix);
}

#[test]
fn a_workflow_info_carries_the_facts() {
    let c = compiler();
    let def = json!({"id": "w", "channel": "orders", "priority": 7, "status": "paused",
        "version": 3, "tasks": [log("t")]});
    let fingerprint = compile(&c, &def).fingerprint();
    let program = link(&c, &[def]);
    let w = &program.workflows()[0];
    assert_eq!(
        (
            w.id.as_str(),
            w.channel.as_str(),
            w.priority,
            w.status,
            w.version
        ),
        ("w", "orders", 7, Status::Paused, 3)
    );
    assert_eq!(w.fingerprint, fingerprint);
}

#[test]
fn the_registry_is_the_compilers() {
    let c = compiler();
    let program = link(&c, &[]);
    let names = |r: &Registry| r.catalogue().map(|f| f.name.to_owned()).collect::<Vec<_>>();
    assert_eq!(names(program.registry()), names(c.registry()));
    assert!(program.workflows().is_empty() && program.tasks().is_empty());
}

#[test]
fn duplicate_ids_are_both_kept() {
    let c = compiler();
    let d = json!({"id": "same", "tasks": [log("t")]});
    let program = link(&c, &[d.clone(), d]);
    assert_eq!(ids(&program), ["same", "same"]);
    assert_eq!(program.tasks().len(), 2);
}
