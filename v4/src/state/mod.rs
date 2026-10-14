//! The run context and what it is built from: the run view, the write log and
//! the checkpoint. Adopted from the phase 0 spike; the runtime uses it from
//! phase 4's Task 4.

mod run;
mod size;
pub(crate) mod splice;
#[cfg(test)]
pub(crate) mod tests;
mod writes;

pub(crate) use run::{Edit, Reader, RunContext, Stats, Tuning};
pub(crate) use splice::kind;
// `Owner` is re-exported by the first code outside `state` that names it.
pub(crate) use writes::{At, Lookup, Mark, Writes};
