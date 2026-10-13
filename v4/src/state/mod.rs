//! The run context and what it is built from: the run view, the write log and
//! the checkpoint. Adopted from the phase 0 spike; the runtime uses it from
//! phase 4's Task 4.

mod run;
mod size;
mod splice;
#[cfg(test)]
mod tests;
mod writes;

#[expect(unused_imports, reason = "Tasks 4 and 5 import them")]
pub(crate) use run::{Edit, Owner, Reader};
#[cfg_attr(not(test), expect(unused_imports, reason = "Task 4 imports them"))]
pub(crate) use run::{RunContext, Stats, Tuning};
#[expect(unused_imports, reason = "Task 4 imports it")]
pub(crate) use writes::At;
#[cfg_attr(not(test), expect(unused_imports, reason = "Task 4 imports them"))]
pub(crate) use writes::{Lookup, Mark, Writes};
