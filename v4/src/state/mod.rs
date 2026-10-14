//! The run context and what it is built from: the run view, the write log and
//! the checkpoint. Adopted from the phase 0 spike; the runtime uses it from
//! phase 4's Task 4.

mod run;
mod size;
mod splice;
#[cfg(test)]
mod tests;
mod writes;

pub(crate) use run::{Edit, RunContext, Stats};
pub(crate) use splice::kind;
// What only the tests name from outside `state` so far: the engine picks a
// run's tuning from Task 6. `Owner`, `Reader` and `At` are re-exported by the
// first code outside `state` that names them.
#[cfg(test)]
pub(crate) use run::Tuning;
#[cfg(test)]
pub(crate) use writes::{Lookup, Mark, Writes};
