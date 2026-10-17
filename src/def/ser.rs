//! `Serialize` for the definition types: the v4 spelling, which
//! [`parse`](super::parse) reads back with the compat reader off.
//!
//! Only v4 keys are written, in the known-key table's order. A key holding
//! its default is left out (`priority` 0, `channel` `"default"`, `status`
//! active, `version` 1, the default policies, `terminal: false`, `init` 0,
//! `increment` 1, `max_concurrency` 1), and so is an empty one (`None`, an
//! empty `tags`, an empty `ext`). `ext` is written as `"ext"`, a function's
//! `input` as it was, a condition or `over` as its raw JSON, and a timestamp
//! in RFC 3339.

use super::Rollout;
use super::types::{
    FailurePolicy, ForEachDef, FunctionRef, GroupDef, Logic, LoopDef, NodeHeader, RejectionPolicy,
    Status, StepDef, TaskDef, WorkflowDef, WorkflowFailurePolicy, defaults,
};
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::convert::Infallible;

/// Where a node's present fields go. Each node lists them once, and that list
/// is run twice: counted, then written, so the map's length is exact for a
/// format that needs it up front.
trait Fields {
    type Error;
    fn field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error>;

    /// The field, when `value` is set.
    fn opt<T: Serialize>(
        &mut self,
        key: &'static str,
        value: Option<&T>,
    ) -> Result<(), Self::Error> {
        value.map_or(Ok(()), |value| self.field(key, value))
    }

    /// The field, when `present`: a key holding its default is left out.
    fn field_if<T: Serialize + ?Sized>(
        &mut self,
        present: bool,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if present {
            self.field(key, value)
        } else {
            Ok(())
        }
    }
}

struct Count(usize);

impl Fields for Count {
    type Error = Infallible;
    fn field<T: Serialize + ?Sized>(&mut self, _: &'static str, _: &T) -> Result<(), Infallible> {
        self.0 += 1;
        Ok(())
    }
}

struct Write<M>(M);

impl<M: SerializeMap> Fields for Write<M> {
    type Error = M::Error;
    fn field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), M::Error> {
        self.0.serialize_entry(key, value)
    }
}

/// A node written as a map of its present fields.
trait Node {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error>;
}

fn node<N: Node, S: Serializer>(node: &N, serializer: S) -> Result<S::Ok, S::Error> {
    let mut count = Count(0);
    let Ok(()) = node.fields(&mut count);
    let mut out = Write(serializer.serialize_map(Some(count.0))?);
    node.fields(&mut out)?;
    out.0.end()
}

/// `name` and `description`, when set.
fn names<F: Fields>(header: &NodeHeader, out: &mut F) -> Result<(), F::Error> {
    out.opt("name", header.name.as_ref())?;
    out.opt("description", header.description.as_ref())
}

/// `ext`, when it holds anything: the last key a node writes.
fn ext<F: Fields>(header: &NodeHeader, out: &mut F) -> Result<(), F::Error> {
    out.field_if(!header.ext.is_empty(), "ext", &header.ext)
}

impl Node for WorkflowDef {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error> {
        out.field("id", &self.id)?;
        names(&self.header, out)?;
        out.field_if(
            self.priority != defaults::PRIORITY,
            "priority",
            &self.priority,
        )?;
        out.opt("condition", self.condition.as_ref())?;
        out.field("tasks", &self.tasks)?;
        out.field_if(
            self.on_failure != WorkflowFailurePolicy::default(),
            "on_failure",
            &self.on_failure,
        )?;
        out.field_if(self.channel != defaults::CHANNEL, "channel", &self.channel)?;
        out.field_if(self.status != Status::default(), "status", &self.status)?;
        out.field_if(self.version != defaults::VERSION, "version", &self.version)?;
        out.opt("rollout", self.rollout.as_ref())?;
        out.opt("loop", self.r#loop.as_ref())?;
        out.field_if(!self.tags.is_empty(), "tags", &self.tags)?;
        if let Some(at) = &self.created_at {
            out.field("created_at", &at.to_rfc3339())?;
        }
        if let Some(at) = &self.updated_at {
            out.field("updated_at", &at.to_rfc3339())?;
        }
        ext(&self.header, out)
    }
}

impl Node for TaskDef {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error> {
        out.field("id", &self.id)?;
        names(&self.header, out)?;
        out.opt("condition", self.condition.as_ref())?;
        out.field("function", &self.function)?;
        out.field_if(
            self.on_failure != FailurePolicy::default(),
            "on_failure",
            &self.on_failure,
        )?;
        out.field_if(
            self.on_rejection != RejectionPolicy::default(),
            "on_rejection",
            &self.on_rejection,
        )?;
        out.field_if(self.terminal, "terminal", &true)?;
        out.opt("for_each", self.for_each.as_ref())?;
        ext(&self.header, out)
    }
}

impl Node for GroupDef {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error> {
        out.field("id", &self.id)?;
        names(&self.header, out)?;
        out.opt("condition", self.condition.as_ref())?;
        out.field_if(self.terminal, "terminal", &true)?;
        out.field("tasks", &self.tasks)?;
        ext(&self.header, out)
    }
}

impl Node for LoopDef {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error> {
        out.opt("counter", self.counter.as_ref())?;
        out.field_if(self.init != defaults::INIT, "init", &self.init)?;
        out.field_if(
            self.increment != defaults::INCREMENT,
            "increment",
            &self.increment,
        )?;
        out.field("max", &self.max)?;
        out.field_if(!self.setup.is_empty(), "setup", &self.setup)?;
        out.opt("over", self.over.as_ref())?;
        out.opt("as", self.r#as.as_ref())?;
        out.opt("scratch", self.scratch.as_ref())
    }
}

impl Node for ForEachDef {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error> {
        out.field("over", &self.over)?;
        out.field("as", &self.r#as)?;
        out.field_if(
            self.max_concurrency != defaults::MAX_CONCURRENCY,
            "max_concurrency",
            &self.max_concurrency,
        )?;
        out.opt("collect", self.collect.as_ref())?;
        out.opt("into", self.into.as_ref())
    }
}

impl Node for FunctionRef {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error> {
        out.field("name", &self.name)?;
        out.field("input", &self.input)
    }
}

impl Node for Rollout {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error> {
        out.field("bucket_start", &self.bucket_start)?;
        out.field("bucket_end", &self.bucket_end)
    }
}

impl Node for NodeHeader {
    fn fields<F: Fields>(&self, out: &mut F) -> Result<(), F::Error> {
        names(self, out)?;
        ext(self, out)
    }
}

/// `Serialize` through [`Node`], for each type that is written as a map.
macro_rules! serialize_as_node {
    ($($ty:ty),* $(,)?) => {$(
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                node(self, serializer)
            }
        }
    )*};
}

serialize_as_node!(
    WorkflowDef,
    TaskDef,
    GroupDef,
    LoopDef,
    ForEachDef,
    FunctionRef,
    Rollout,
    NodeHeader,
);

impl Serialize for StepDef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Task(task) => task.serialize(serializer),
            Self::Group(group) => group.serialize(serializer),
        }
    }
}

impl Serialize for Logic {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// `Serialize` as the authored spelling, for each policy and status.
macro_rules! serialize_as_spelling {
    ($($ty:ty),* $(,)?) => {$(
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }
    )*};
}

serialize_as_spelling!(
    Status,
    FailurePolicy,
    RejectionPolicy,
    WorkflowFailurePolicy,
);
