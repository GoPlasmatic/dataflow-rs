//! The state module's tests: the splice against the owned write it stands for,
//! the write log's rules, and the run context against a model of the owned
//! tree. The seeds and case counts are the originals'.

pub(crate) mod random;
mod run_context;
mod splice_property;
mod writes;
